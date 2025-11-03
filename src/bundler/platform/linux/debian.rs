//! Debian package (.deb) bundler.
//!
//! Creates .deb packages as ar archives with proper Debian structure.
//!
//! A .deb file is an ar archive containing:
//! - debian-binary: Format version (2.0)
//! - control.tar.gz: Package metadata (control, md5sums, scripts)
//! - data.tar.gz: Files to install

use crate::bundler::{
    error::{Context, ErrorExt, Result, Error},
    settings::{Arch, Settings},
    utils::fs::{copy_custom_files, copy_file},
};
use flate2::{Compression, write::GzEncoder};
use std::{
    fs::{self as std_fs, File},
    io::{self, Write},
    path::{Path, PathBuf},
};
use tar::HeaderMode;
use walkdir::WalkDir;

/// Bundle project as Debian package.
/// Returns vector with path to created .deb file.
pub async fn bundle_project(settings: &Settings) -> Result<Vec<PathBuf>> {
    // Map architecture
    let arch = arch_to_debian(settings.binary_arch())?;

    // Create package name: {product}_{version}_{arch}.deb
    let package_base_name = format!(
        "{}_{}_{}",
        settings.product_name(),
        settings.version_string(),
        arch
    );
    let package_name = format!("{}.deb", package_base_name);

    // Setup directories
    let base_dir = settings.project_out_directory().join("bundle/deb");
    let package_dir = base_dir.join(&package_base_name);

    // Remove old package directory if it exists
    if package_dir.exists() {
        tokio::fs::remove_dir_all(&package_dir).await
            .fs_context("removing old package directory", &package_dir)?;
    }

    let package_path = base_dir.join(&package_name);

    log::info!("Bundling {} ({})", package_name, package_path.display());

    // Generate data directory (binaries, resources, desktop file)
    let data_dir =
        generate_data(settings, &package_dir).await.context("failed to generate data directory")?;

    // Copy custom files if specified
    copy_custom_files(&settings.bundle_settings().deb.files, &data_dir).await
        .context("failed to copy custom files")?;

    // Generate control directory
    let control_dir = package_dir.join("control");
    generate_control_file(settings, arch, &control_dir, &data_dir).await
        .context("failed to generate control file")?;
    generate_scripts(settings, &control_dir).await.context("failed to generate control scripts")?;
    generate_md5sums(&control_dir, &data_dir).await.context("failed to generate md5sums file")?;

    // Create debian-binary file with format version
    let debian_binary_path = package_dir.join("debian-binary");
    tokio::fs::write(&debian_binary_path, "2.0\n").await
        .fs_context("creating debian-binary file", &debian_binary_path)?;

    // Create tar.gz archives
    let control_tar_gz =
        tar_and_gzip_dir(control_dir).await.context("failed to tar/gzip control directory")?;
    let data_tar_gz = tar_and_gzip_dir(data_dir).await.context("failed to tar/gzip data directory")?;

    // Create final ar archive
    create_ar_archive(
        vec![debian_binary_path, control_tar_gz, data_tar_gz],
        &package_path,
    ).await
    .context("failed to create ar archive")?;

    Ok(vec![package_path])
}

/// Generate data directory with all files to be installed.
async fn generate_data(settings: &Settings, package_dir: &Path) -> Result<PathBuf> {
    let data_dir = package_dir.join("data");
    let bin_dir = data_dir.join("usr/bin");

    // Copy all binaries
    for bin in settings.binaries() {
        let bin_path = settings.binary_path(bin);
        let dest = bin_dir.join(bin.name());
        copy_file(&bin_path, &dest).await
            .with_context(|| format!("failed to copy binary {:?}", bin_path))?;

        // Set executable permission on Unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755)).await
                .fs_context("setting executable permission", &dest)?;
        }
    }

    // Generate desktop file
    generate_desktop_file(settings, &data_dir).await?;

    // Generate compressed changelog if provided
    generate_changelog(settings, &data_dir).await?;

    Ok(data_dir)
}

/// Generate freedesktop.org desktop file at usr/share/applications/<name>.desktop
async fn generate_desktop_file(settings: &Settings, data_dir: &Path) -> Result<()> {
    let desktop_path = data_dir
        .join("usr/share/applications")
        .join(format!("{}.desktop", settings.product_name()));

    // Clone necessary data for the blocking task
    let product_name = settings.product_name().to_string();
    let short_description = settings.bundle_settings().short_description.clone();
    let category = settings.bundle_settings().category.clone();

    tokio::task::spawn_blocking(move || -> Result<()> {
        if let Some(parent) = desktop_path.parent() {
            std_fs::create_dir_all(parent)
                .fs_context("creating desktop file directory", parent)?;
        }
        let mut file = File::create(&desktop_path)
            .fs_context("creating desktop file", &desktop_path)?;

        writeln!(file, "[Desktop Entry]")?;
        writeln!(file, "Type=Application")?;
        writeln!(file, "Name={}", product_name)?;
        writeln!(file, "Exec={}", product_name)?;
        writeln!(file, "Terminal=false")?;

        // Optional fields from settings
        if let Some(desc) = short_description.as_ref() {
            writeln!(file, "Comment={}", desc)?;
        }
        if let Some(category) = category.as_ref() {
            writeln!(file, "Categories={}", category)?;
        }

        file.flush()?;
        Ok(())
    })
    .await
    .map_err(|e| Error::GenericError(format!("Desktop file generation task failed: {}", e)))??;
    
    Ok(())
}

/// Generate compressed changelog at usr/share/doc/<name>/changelog.gz
async fn generate_changelog(settings: &Settings, data_dir: &Path) -> Result<()> {
    if let Some(changelog_path) = &settings.bundle_settings().deb.changelog {
        let dest = data_dir.join(format!(
            "usr/share/doc/{}/changelog.gz",
            settings.product_name()
        ));

        let src_path = changelog_path.clone();
        let dest_path = dest.clone();
        
        tokio::task::spawn_blocking(move || -> Result<()> {
            let mut src = File::open(&src_path)
                .fs_context("opening changelog file", &src_path)?;
            
            if let Some(parent) = dest_path.parent() {
                std_fs::create_dir_all(parent)
                    .fs_context("creating changelog directory", parent)?;
            }
            let dest_file = File::create(&dest_path)
                .fs_context("creating changelog destination", &dest_path)?;
            
            let mut encoder = GzEncoder::new(dest_file, Compression::new(9));
            io::copy(&mut src, &mut encoder)?;
            let mut finished = encoder.finish()?;
            finished.flush()?;
            Ok(())
        })
        .await
        .map_err(|e| Error::GenericError(format!("Changelog generation task failed: {}", e)))??;
    }
    
    Ok(())
}

/// Generate control file with package metadata.
async fn generate_control_file(
    settings: &Settings,
    arch: &str,
    control_dir: &Path,
    data_dir: &Path,
) -> Result<()> {
    let control_path = control_dir.join("control");
    
    // Clone all data needed for blocking task
    let package = settings.product_name().to_lowercase().replace(' ', "-");
    let version = settings.version_string().to_string();
    let arch = arch.to_string();
    let size_kb = calculate_dir_size(data_dir).await? / 1024;
    let maintainer = settings
        .authors()
        .map(|a| a.join(", "))
        .or_else(|| settings.bundle_settings().publisher.clone())
        .unwrap_or_else(|| "Unknown".to_string());
    let section = settings.bundle_settings().deb.section.clone();
    let priority = settings.bundle_settings().deb.priority.clone();
    let homepage = settings.homepage().map(|s| s.to_string());
    let depends = settings.bundle_settings().deb.depends.clone();
    let recommends = settings.bundle_settings().deb.recommends.clone();
    let provides = settings.bundle_settings().deb.provides.clone();
    let conflicts = settings.bundle_settings().deb.conflicts.clone();
    let replaces = settings.bundle_settings().deb.replaces.clone();
    let short_description = settings.bundle_settings().short_description.clone();
    let long_description = settings.bundle_settings().long_description.clone();

    tokio::task::spawn_blocking(move || -> Result<()> {
        if let Some(parent) = control_path.parent() {
            std_fs::create_dir_all(parent)
                .fs_context("creating control directory", parent)?;
        }
        let mut file = File::create(&control_path)
            .fs_context("creating control file", &control_path)?;

        writeln!(file, "Package: {}", package)?;
        writeln!(file, "Version: {}", version)?;
        writeln!(file, "Architecture: {}", arch)?;
        writeln!(file, "Installed-Size: {}", size_kb)?;
        writeln!(file, "Maintainer: {}", maintainer)?;

        if let Some(section) = section {
            writeln!(file, "Section: {}", section)?;
        }

        if let Some(priority) = priority {
            writeln!(file, "Priority: {}", priority)?;
        } else {
            writeln!(file, "Priority: optional")?;
        }

        if let Some(homepage) = homepage {
            writeln!(file, "Homepage: {}", homepage)?;
        }

        if let Some(depends) = depends {
            writeln!(file, "Depends: {}", depends.join(", "))?;
        }

        if let Some(recommends) = recommends {
            writeln!(file, "Recommends: {}", recommends.join(", "))?;
        }

        if let Some(provides) = provides {
            writeln!(file, "Provides: {}", provides.join(", "))?;
        }

        if let Some(conflicts) = conflicts {
            writeln!(file, "Conflicts: {}", conflicts.join(", "))?;
        }

        if let Some(replaces) = replaces {
            writeln!(file, "Replaces: {}", replaces.join(", "))?;
        }

        let short = short_description
            .as_deref()
            .unwrap_or("(no description)");
        writeln!(file, "Description: {}", short)?;

        if let Some(long) = long_description {
            for line in long.lines() {
                if line.trim().is_empty() {
                    writeln!(file, " .")?;
                } else {
                    writeln!(file, " {}", line.trim())?;
                }
            }
        }

        file.flush()?;
        Ok(())
    })
    .await
    .map_err(|e| Error::GenericError(format!("Control file generation task failed: {}", e)))??;
    
    Ok(())
}

/// Generate MD5 checksums for all files in data directory.
async fn generate_md5sums(control_dir: &Path, data_dir: &Path) -> Result<()> {
    let md5sums_path = control_dir.join("md5sums");
    let data_dir = data_dir.to_path_buf();
    
    tokio::task::spawn_blocking(move || -> Result<()> {
        
        if let Some(parent) = md5sums_path.parent() {
            std_fs::create_dir_all(parent)
                .fs_context("creating md5sums directory", parent)?;
        }
        let mut file = File::create(&md5sums_path)
            .fs_context("creating md5sums file", &md5sums_path)?;

        for entry in WalkDir::new(&data_dir) {
            let entry = entry?;
            if !entry.file_type().is_file() {
                continue;
            }

            // Calculate MD5 hash
            let mut src = File::open(entry.path())
                .fs_context("opening file for MD5", entry.path())?;
            let mut context = md5::Context::new();
            io::copy(&mut src, &mut context)?;
            let digest = context.finalize();

            // Write in format: "hex_digest  relative_path"
            for byte in digest.iter() {
                write!(file, "{:02x}", byte)?;
            }

            let rel_path = entry.path().strip_prefix(&data_dir)?;
            writeln!(file, "  {}", rel_path.display())?;
        }

        file.flush()?;
        Ok(())
    })
    .await
    .map_err(|e| Error::GenericError(format!("MD5sums generation task failed: {}", e)))??;
    
    Ok(())
}

/// Generate maintainer scripts (preinst, postinst, prerm, postrm).
async fn generate_scripts(settings: &Settings, control_dir: &Path) -> Result<()> {
    let scripts = [
        (
            &settings.bundle_settings().deb.pre_install_script,
            "preinst",
        ),
        (
            &settings.bundle_settings().deb.post_install_script,
            "postinst",
        ),
        (&settings.bundle_settings().deb.pre_remove_script, "prerm"),
        (&settings.bundle_settings().deb.post_remove_script, "postrm"),
    ];

    for (script_opt, name) in scripts {
        if let Some(script_path) = script_opt {
            let dest = control_dir.join(name);
            let mut src = tokio::fs::File::open(script_path).await.fs_context("opening script file", script_path)?;

            // Create with executable permissions
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                let mut dest_file = tokio::fs::OpenOptions::new()
                    .create(true)
                    .write(true)
                    .truncate(true)
                    .mode(0o755)
                    .open(&dest)
                    .await
                    .fs_context("creating script file", &dest)?;

                tokio::io::copy(&mut src, &mut dest_file).await?;
            }

            #[cfg(not(unix))]
            {
                let mut dest_file =
                    tokio::fs::File::create(&dest).await.fs_context("creating script file", &dest)?;
                tokio::io::copy(&mut src, &mut dest_file).await?;
            }
        }
    }

    Ok(())
}

/// Create tar.gz archive from directory.
async fn tar_and_gzip_dir(src_dir: PathBuf) -> Result<PathBuf> {
    let dest_path = src_dir.with_extension("tar.gz");
    let tar_gz = tokio::fs::File::create(&dest_path).await.fs_context("creating tar.gz file", &dest_path)?;
    let std_file = tar_gz.into_std().await;
    
    tokio::task::spawn_blocking(move || {
        let enc = GzEncoder::new(std_file, Compression::default());
        let mut tar = tar::Builder::new(enc);

        for entry in WalkDir::new(&src_dir) {
            let entry = entry?;
            let path = entry.path();

            if path == src_dir {
                continue;
            }

            let rel_path = path.strip_prefix(&src_dir)?;
            let metadata = std::fs::metadata(path)?; // Use blocking fs in spawn_blocking

            let mut header = tar::Header::new_gnu();
            header.set_metadata_in_mode(&metadata, HeaderMode::Deterministic);

            if entry.file_type().is_dir() {
                tar.append_data(&mut header, rel_path, &mut io::empty())?;
            } else {
                let mut file = std::fs::File::open(path)?; // Use blocking fs
                tar.append_data(&mut header, rel_path, &mut file)?;
            }
        }

        let enc = tar.into_inner()?;
        let mut finished = enc.finish()?;
        finished.flush()?;
        Ok(dest_path)
    }).await.map_err(|e| Error::GenericError(format!("Join error: {}", e)))?
}

/// Create ar archive (final .deb package).
async fn create_ar_archive(files: Vec<PathBuf>, dest: &Path) -> Result<()> {
    let tokio_file = tokio::fs::File::create(dest).await.fs_context("creating .deb archive", dest)?;
    let dest_file = tokio_file.into_std().await;
    
    tokio::task::spawn_blocking(move || {
        let mut builder = ar::Builder::new(dest_file);
        
        for path in &files {
            builder.append_path(path)?;
        }

        let finished = builder.into_inner()?;
        finished.sync_all()?;
        Ok(())
    }).await.map_err(|e| Error::GenericError(format!("Join error: {}", e)))?
}

/// Map Rust architecture to Debian architecture string.
fn arch_to_debian(arch: Arch) -> Result<&'static str> {
    match arch {
        Arch::X86_64 => Ok("amd64"),
        Arch::X86 => Ok("i386"),
        Arch::AArch64 => Ok("arm64"),
        Arch::Armhf => Ok("armhf"),
        Arch::Armel => Ok("armel"),
        Arch::Riscv64 => Ok("riscv64"),
        _ => Err(crate::bundler::error::Error::ArchError(format!(
            "Unsupported architecture for Debian: {:?}",
            arch
        ))),
    }
}

/// Calculate total size of directory in bytes.
async fn calculate_dir_size(dir: &Path) -> Result<u64> {
    let dir = dir.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let mut total = 0u64;
        for entry in WalkDir::new(&dir) {
            let entry = entry?;
            if entry.file_type().is_file() {
                total += entry.metadata()?.len();
            }
        }
        Ok(total)
    }).await.map_err(|e| Error::GenericError(format!("Join error: {}", e)))?
}
