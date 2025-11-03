//! AppImage bundler - portable Linux applications.

use crate::{
    bail,
    bundler::{
        error::{Context, ErrorExt, Result},
        settings::Settings,
        utils::http,
    },
};
use std::{
    path::{Path, PathBuf},
};
use tokio::io::AsyncWriteExt;

const LINUXDEPLOY_BASE_URL: &str =
    "https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous";

/// Bundle project as AppImage.
///
/// Creates a portable, self-contained AppImage executable that runs on any Linux distribution.
///
/// # Process
///
/// 1. Downloads linuxdeploy tool (cached in .tools/)
/// 2. Creates AppDir structure (usr/bin, usr/lib)
/// 3. Copies binaries and resources
/// 4. Generates .desktop file
/// 5. Invokes linuxdeploy to create AppImage
///
/// # Returns
///
/// Vector containing the path to the generated .AppImage file.
pub async fn bundle_project(settings: &Settings) -> Result<Vec<PathBuf>> {
    // 1. Map architecture
    let arch = match settings.binary_arch() {
        crate::bundler::settings::Arch::X86_64 => "x86_64",
        crate::bundler::settings::Arch::X86 => "i386",
        crate::bundler::settings::Arch::AArch64 => "aarch64",
        _ => bail!(
            "Unsupported architecture for AppImage: {:?}",
            settings.binary_arch()
        ),
    };

    log::info!("Building AppImage for {}", settings.product_name());
    log::debug!("Using architecture: {}", arch);

    // 2. Setup directories
    let output_dir = settings.project_out_directory().join("bundle/appimage");
    let tools_dir = output_dir.join(".tools");

    tokio::fs::create_dir_all(&tools_dir).await.fs_context("creating tools directory", &tools_dir)?;

    // 3. Download linuxdeploy
    let linuxdeploy =
        download_linuxdeploy(&tools_dir, arch).await.context("failed to download linuxdeploy tool")?;

    // 4. Create AppDir structure
    let app_dir = output_dir.join(format!("{}.AppDir", settings.product_name()));

    // Clean any existing AppDir
    if app_dir.exists() {
        tokio::fs::remove_dir_all(&app_dir).await.fs_context("removing old AppDir", &app_dir)?;
    }

    // Create directory structure
    let usr_dir = app_dir.join("usr");
    let bin_dir = usr_dir.join("bin");
    let lib_dir = usr_dir.join("lib");

    for dir in [&usr_dir, &bin_dir, &lib_dir] {
        tokio::fs::create_dir_all(dir).await.fs_context("creating AppDir structure", dir)?;
    }

    // 5. Copy binaries
    for binary in settings.binaries() {
        let src = settings.binary_path(binary);
        let dst = bin_dir.join(binary.name());

        tokio::fs::copy(&src, &dst).await.fs_context("copying binary", &dst)?;

        // Ensure executable permissions
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(&dst, std::fs::Permissions::from_mode(0o755)).await?;
        }
    }

    // 6. Create desktop file
    create_desktop_file(settings, &app_dir).await?;

    // 7. Copy icon (if available)
    if let Some(icon_paths) = &settings.bundle_settings().icon {
        // Find first PNG icon (AppImage requires PNG)
        if let Some(icon_path) = icon_paths
            .iter()
            .find(|p| p.extension().and_then(|e| e.to_str()) == Some("png"))
        {
            let icon_name = format!("{}.png", settings.product_name());
            let dst_icon = app_dir.join(&icon_name);

            tokio::fs::copy(icon_path, &dst_icon).await.fs_context("copying icon", &dst_icon)?;

            // Create .DirIcon symlink (required by AppImage spec)
            #[cfg(unix)]
            {
                let diricon_path = app_dir.join(".DirIcon");
                tokio::fs::symlink(&icon_name, &diricon_path).await?;
            }
        }
    }

    // 8. Invoke linuxdeploy
    let appimage_path = output_dir.join(format!(
        "{}-{}-{}.AppImage",
        settings.product_name(),
        settings.version_string(),
        arch
    ));

    let app_dir_str = app_dir
        .to_str()
        .context("AppDir path contains invalid UTF-8")?;

    let status = tokio::process::Command::new(&linuxdeploy)
        .env("OUTPUT", &appimage_path)
        .env("ARCH", arch)
        .args(["--appdir", app_dir_str, "--output", "appimage"])
        .status()
        .await
        .map_err(|e| crate::bundler::Error::GenericError(format!("Failed to execute linuxdeploy: {}", e)))?;

    if !status.success() {
        bail!("linuxdeploy failed with exit code: {:?}", status.code());
    }

    // 9. Set final permissions
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(&appimage_path, std::fs::Permissions::from_mode(0o755)).await?;
    }

    log::info!("✓ Created AppImage: {}", appimage_path.display());

    Ok(vec![appimage_path])
}

/// Download linuxdeploy tool.
///
/// Downloads the linuxdeploy AppImage from GitHub and caches it locally.
/// Returns early if the tool is already cached.
async fn download_linuxdeploy(tools_dir: &Path, arch: &str) -> Result<PathBuf> {
    let tool_name = format!("linuxdeploy-{}.AppImage", arch);
    let tool_path = tools_dir.join(&tool_name);

    // Return early if already downloaded
    if tool_path.exists() {
        log::debug!("linuxdeploy already cached at {:?}", tool_path);
        return Ok(tool_path);
    }

    log::info!("Downloading linuxdeploy for {}...", arch);

    let url = format!("{}/{}", LINUXDEPLOY_BASE_URL, tool_name);
    let data = http::download(&url).await?;

    tokio::fs::write(&tool_path, data).await.fs_context("writing linuxdeploy tool", &tool_path)?;

    // Make executable on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(&tool_path, std::fs::Permissions::from_mode(0o755)).await?;
    }

    Ok(tool_path)
}

/// Create .desktop file for the AppImage.
///
/// Generates a freedesktop.org compliant desktop entry with application metadata.
async fn create_desktop_file(settings: &Settings, app_dir: &Path) -> Result<()> {
    let desktop_file = app_dir.join(format!("{}.desktop", settings.product_name()));
    let mut file = tokio::fs::File::create(&desktop_file).await.fs_context("creating desktop file", &desktop_file)?;

    file.write_all(b"[Desktop Entry]\n").await?;
    file.write_all(b"Type=Application\n").await?;
    file.write_all(format!("Name={}\n", settings.product_name()).as_bytes()).await?;

    // Find main binary name
    let main_binary = settings
        .binaries()
        .iter()
        .find(|b| b.main())
        .context("no main binary found")?;

    file.write_all(format!("Exec={}\n", main_binary.name()).as_bytes()).await?;
    file.write_all(format!("Icon={}\n", settings.product_name()).as_bytes()).await?;

    // Optional fields from bundle settings
    let bundle = settings.bundle_settings();

    if !settings.description().is_empty() {
        file.write_all(format!("Comment={}\n", settings.description()).as_bytes()).await?;
    }

    if let Some(category) = &bundle.category {
        file.write_all(format!("Categories={}\n", category).as_bytes()).await?;
    }

    file.write_all(b"Terminal=false\n").await?;
    Ok(())
}
