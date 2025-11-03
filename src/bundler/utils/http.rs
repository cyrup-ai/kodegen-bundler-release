//! HTTP utilities for downloading bundler tools.
//!
//! Provides functions for downloading files with hash verification
//! and extracting ZIP archives.

#[cfg(any(target_os = "linux", target_os = "windows"))]
use crate::bundler::error::Result;

#[cfg(any(target_os = "windows", target_os = "linux"))]
use crate::bundler::error::Error;

#[cfg(any(target_os = "windows", target_os = "linux"))]
use std::path::Path;

/// Hash algorithm for verification.
#[derive(Debug, Clone, Copy)]
#[cfg(any(target_os = "windows", target_os = "linux"))]
pub enum HashAlgorithm {
    /// SHA-1 hashing algorithm
    Sha1,
    /// SHA-256 hashing algorithm
    Sha256,
}

/// Downloads a file from a URL.
///
/// Returns the file contents as a byte vector.
///
/// Used by:
/// - Linux: AppImage bundler (downloads linuxdeploy tool)
/// - Windows: MSI/NSIS bundlers (via download_and_verify for WiX/NSIS downloads)
#[cfg(any(target_os = "linux", target_os = "windows"))]
pub async fn download(url: &str) -> Result<Vec<u8>> {
    log::info!("Downloading {}", url);

    let response = reqwest::get(url)
        .await
        .map_err(|e| Error::GenericError(format!("Download failed: {}", e)))?;

    let bytes = response
        .bytes()
        .await
        .map_err(|e| Error::GenericError(format!("Failed to read response: {}", e)))?;

    Ok(bytes.to_vec())
}

/// Downloads a file and verifies its hash.
///
/// Returns the file contents if the hash matches, otherwise returns an error.
#[cfg(any(target_os = "windows", target_os = "linux"))]
pub async fn download_and_verify(
    url: &str,
    expected_hash: &str,
    algorithm: HashAlgorithm,
) -> Result<Vec<u8>> {
    let data = download(url).await?;
    log::info!("validating hash");
    verify_hash(&data, expected_hash, algorithm).await?;
    Ok(data)
}

/// Verifies that data matches the expected hash.
///
/// Compares the hash case-insensitively. Returns an error if the hashes don't match.
/// Uses spawn_blocking to prevent blocking the async runtime during CPU-bound hashing.
#[cfg(any(target_os = "windows", target_os = "linux"))]
pub async fn verify_hash(data: &[u8], expected_hash: &str, algorithm: HashAlgorithm) -> Result<()> {
    use sha1::Digest as _;
    use sha2::Digest as _;

    // Clone data for moving into spawn_blocking
    let data = data.to_vec();
    let expected_hash = expected_hash.to_string();

    tokio::task::spawn_blocking(move || {
        let actual_hash = match algorithm {
            HashAlgorithm::Sha1 => {
                let mut hasher = sha1::Sha1::new();
                hasher.update(&data);
                hex::encode(hasher.finalize())
            }
            HashAlgorithm::Sha256 => {
                let mut hasher = sha2::Sha256::new();
                hasher.update(&data);
                hex::encode(hasher.finalize())
            }
        };

        if actual_hash.eq_ignore_ascii_case(&expected_hash) {
            Ok(())
        } else {
            Err(Error::HashMismatch {
                expected: expected_hash,
                actual: actual_hash,
            })
        }
    })
    .await
    .map_err(|e| Error::GenericError(format!("Hash verification task failed: {}", e)))?
}

/// Extracts a ZIP archive from memory into a destination directory.
///
/// Creates parent directories as needed and handles both files and directories in the archive.
/// 
/// **Security:** Validates paths to prevent traversal attacks. Only extracts files within the
/// destination directory, rejecting entries with `..` or absolute paths.
///
/// Used by:
/// - Windows: MSI bundler (extracts WiX toolset)
/// - Windows: NSIS bundler (extracts NSIS toolset)
#[cfg(any(target_os = "windows", target_os = "linux"))]
pub async fn extract_zip(data: &[u8], dest: &Path) -> Result<()> {
    use async_zip::base::read::mem::ZipFileReader;
    use futures_lite::io::AsyncReadExt as _;

    let reader = ZipFileReader::new(data.to_vec()).await
        .map_err(|e| Error::GenericError(format!("Failed to read ZIP archive: {}", e)))?;

    for i in 0..reader.file().entries().len() {
        let entry = reader.file().entries().get(i)
            .ok_or_else(|| Error::GenericError(format!("Failed to get ZIP entry {}", i)))?;
        
        let filename = entry.filename().as_str()
            .map_err(|e| Error::GenericError(format!("Invalid filename in ZIP: {}", e)))?;
        
        // SECURITY: Validate path to prevent directory traversal
        if filename.contains("..") || filename.starts_with('/') || filename.starts_with('\\') {
            return Err(Error::GenericError(format!(
                "Invalid ZIP entry path (potential traversal attack): {}", 
                filename
            )));
        }
        
        if entry.dir()
            .map_err(|e| Error::GenericError(format!("Failed to check if entry is directory: {}", e)))? 
        {
            // Directory entry
            let dir_path = dest.join(filename);
            tokio::fs::create_dir_all(&dir_path).await?;
            continue;
        }

        // File entry
        let file_path = dest.join(filename);
        
        if let Some(parent) = file_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        // Read file content asynchronously
        let mut entry_reader = reader.reader_with_entry(i).await
            .map_err(|e| Error::GenericError(format!("Failed to read ZIP entry: {}", e)))?;
        let mut content = Vec::new();
        entry_reader.read_to_end(&mut content).await?;

        // Write file asynchronously
        tokio::fs::write(&file_path, content).await?;
    }

    Ok(())
}
