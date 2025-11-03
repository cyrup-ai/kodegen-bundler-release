//! Platform detection and classification for Docker-based builds.
//!
//! Determines which platforms can be built natively vs which require Docker containers.

use crate::bundler::PackageType;

/// Splits package types into native (run locally) vs containerized (run in Docker).
///
/// Based on the current host OS, determines which platforms can be built natively
/// and which require a Docker container.
///
/// # Platform Support
///
/// - **macOS**: Native=[MacOsBundle, Dmg], Container=[Deb, Rpm, AppImage, Nsis]
/// - **Linux**: Native=[Deb, Rpm, AppImage, Nsis], Container=[]
/// - **Windows**: Native=[Nsis], Container=[Deb, Rpm, AppImage]
///
/// Note: macOS packages cannot be built in containers due to Apple licensing restrictions.
///
/// # Arguments
///
/// * `platforms` - Requested package types
///
/// # Returns
///
/// * `(native, containerized)` - Tuple of (platforms to build locally, platforms to build in Docker)
pub fn split_platforms_by_host(platforms: &[PackageType]) -> (Vec<PackageType>, Vec<PackageType>) {
    let mut native = Vec::new();
    let mut containerized = Vec::new();

    for &platform in platforms {
        if is_native_platform(platform) {
            native.push(platform);
        } else {
            containerized.push(platform);
        }
    }

    (native, containerized)
}

/// Checks if a platform can be built natively on the current host OS.
///
/// Uses runtime OS detection via `std::env::consts::OS` instead of compile-time
/// cfg attributes.
///
/// # Platform Support
///
/// - **macOS**: MacOsBundle, Dmg (native only, cannot be built in containers)
/// - **Linux**: Deb, Rpm, AppImage, Nsis (always native)
/// - **Windows**: Nsis (native)
/// - **All others**: Require Docker container
///
/// # Returns
///
/// - `true` - Platform can be built natively on current OS
/// - `false` - Platform requires Docker container
fn is_native_platform(platform: PackageType) -> bool {
    use PackageType::*;

    match (std::env::consts::OS, platform) {
        // macOS native packages (cannot be built in Linux containers)
        ("macos", MacOsBundle | Dmg) => true,

        // Linux native packages (NSIS works natively via makensis)
        ("linux", Deb | Rpm | AppImage | Nsis) => true,

        // Windows native packages
        ("windows", Nsis) => true,

        // Everything else needs Docker
        _ => false,
    }
}

/// Converts PackageType to string for CLI arguments.
pub fn platform_type_to_string(platform: PackageType) -> &'static str {
    match platform {
        PackageType::Deb => "deb",
        PackageType::Rpm => "rpm",
        PackageType::AppImage => "appimage",
        PackageType::MacOsBundle => "app",
        PackageType::Dmg => "dmg",
        PackageType::Nsis => "nsis",
    }
}

/// Returns emoji for platform type (for pretty output).
pub fn platform_emoji(platform: PackageType) -> &'static str {
    match platform {
        PackageType::Deb | PackageType::Rpm | PackageType::AppImage => "🐧",
        PackageType::MacOsBundle | PackageType::Dmg => "🍎",
        PackageType::Nsis => "🪟",
    }
}
