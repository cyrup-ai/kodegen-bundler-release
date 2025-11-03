# kodegen-bundler-release

**Production-quality release management and multi-platform bundling for Rust workspaces.**

[![Crates.io](https://img.shields.io/crates/v/kodegen_bundler_release.svg)](https://crates.io/crates/kodegen_bundler_release)
[![License](https://img.shields.io/badge/license-Apache%202.0%20OR%20MIT-blue.svg)](LICENSE.md)
[![Rust](https://img.shields.io/badge/rust-nightly--2024--10--20-orange.svg)](https://rust-lang.github.io/rustup/)

## Features

- 🚀 **Atomic Release Operations** - All-or-nothing releases with automatic rollback on failure
- 📦 **Multi-Platform Bundling** - Create native installers for Linux, macOS, and Windows
- 🔄 **Version Synchronization** - Automatically update internal workspace dependencies
- 🌳 **Pure Rust Git** - No `git` CLI dependency, uses [gix](https://github.com/Byron/gitoxide)
- 📊 **Dependency-Ordered Publishing** - Publishes packages in correct topological order
- 🛡️ **Isolated Workflow** - Operates in temporary clones to protect your working directory
- ⏸️ **Resume & Rollback** - Continue interrupted releases or undo failed ones
- 🎯 **GitHub Integration** - Automated release creation and artifact uploads
- 🔐 **Code Signing** - macOS Developer ID and Windows Authenticode support

## Quick Start

### Installation

```bash
# Install from crates.io
cargo install kodegen_bundler_release

# OR build from source
git clone https://github.com/cyrup-ai/kodegen-bundler-release
cd kodegen-bundler-release
cargo install --path .
```

### Basic Usage

```bash
# Release with patch version bump (0.1.0 → 0.1.1)
kodegen_bundler_release release patch

# Preview changes without making modifications
kodegen_bundler_release release minor --dry-run

# Create platform-specific bundles
kodegen_bundler_release bundle --platform deb

# Resume an interrupted release
kodegen_bundler_release resume

# Rollback a failed release
kodegen_bundler_release rollback
```

## What It Does

### Release Workflow

When you run a release command, the tool:

1. **Validates** workspace structure and dependencies
2. **Clones** your repository to `/tmp/kodegen-release-{timestamp}/` (isolated environment)
3. **Updates** version numbers in all `Cargo.toml` files
4. **Synchronizes** internal workspace dependency versions
5. **Commits** changes with formatted commit message
6. **Tags** the release (e.g., `v0.1.0`)
7. **Signs** artifacts (macOS only, optional)
8. **Bundles** platform packages (optional, enabled by default)
9. **Pushes** to remote repository
10. **Creates** GitHub release with notes
11. **Uploads** signed artifacts and bundles
12. **Publishes** packages to crates.io in dependency order
13. **Cleans up** temporary clone

**Your working directory is never modified.** All operations happen in an isolated temporary clone.

### Supported Package Formats

#### Linux Packages
- **Debian (.deb)** - Ubuntu, Debian, and derivatives
- **RPM (.rpm)** - Fedora, RHEL, CentOS, openSUSE
- **AppImage (.AppImage)** - Portable, self-contained executables

#### macOS Packages
- **App Bundle (.app)** - macOS application bundle
- **DMG (.dmg)** - macOS disk image installer

#### Windows Packages
- **MSI (.msi)** - Windows Installer via WiX Toolset
- **NSIS (.exe)** - Lightweight installer via NSIS

## Usage Examples

### Release Commands

```bash
# Standard release workflow
kodegen_bundler_release release patch

# Bump minor version (0.1.x → 0.2.0)
kodegen_bundler_release release minor

# Bump major version (0.x.y → 1.0.0)
kodegen_bundler_release release major

# Dry run (preview without changes)
kodegen_bundler_release release patch --dry-run

# Release without pushing to remote
kodegen_bundler_release release patch --no-push

# Release without GitHub release
kodegen_bundler_release release patch --no-github-release

# Release without creating bundles
kodegen_bundler_release release patch --no-bundles

# Keep temp clone for debugging
kodegen_bundler_release release patch --keep-temp
```

### Bundle Commands

```bash
# Bundle for current platform
kodegen_bundler_release bundle

# Bundle specific platform
kodegen_bundler_release bundle --platform deb
kodegen_bundler_release bundle --platform dmg
kodegen_bundler_release bundle --platform msi

# Bundle without rebuilding binaries
kodegen_bundler_release bundle --no-build

# Bundle and upload to GitHub release
kodegen_bundler_release bundle --upload --github-repo owner/repo

# Bundle for specific architecture
kodegen_bundler_release bundle --target x86_64-apple-darwin
```

### State Management Commands

```bash
# Resume interrupted release
kodegen_bundler_release resume

# Check current release status
kodegen_bundler_release status

# Rollback failed release
kodegen_bundler_release rollback

# Force rollback (even for completed releases)
kodegen_bundler_release rollback --force

# Clean up state without rollback
kodegen_bundler_release cleanup
```

### Validation Commands

```bash
# Validate workspace structure
kodegen_bundler_release validate

# Verbose validation output
kodegen_bundler_release validate --verbose
```

## Configuration

### Environment Variables

#### Required for Publishing

```bash
# crates.io API token
export CARGO_REGISTRY_TOKEN=cio_xxxx

# GitHub API token (for GitHub releases)
export GITHUB_TOKEN=ghp_xxxx
# OR
export GH_TOKEN=ghp_xxxx
```

#### macOS Code Signing (Optional)

```bash
# Add to ~/.zshrc (loaded automatically on startup)
export APPLE_CERTIFICATE=<base64-encoded-p12>
export APPLE_CERTIFICATE_PASSWORD=<password>
export APPLE_TEAM_ID=<team-id>

# Optional: App Store Connect API (for notarization)
export APPLE_API_KEY_CONTENT=<base64-key>
export APPLE_API_KEY_ID=<key-id>
export APPLE_API_ISSUER_ID=<issuer-id>
```

### Cargo.toml Metadata

Configure bundling behavior in your workspace `Cargo.toml`:

```toml
[package.metadata.bundle]
identifier = "com.example.myapp"
publisher = "Example Inc."
icon = ["assets/icon.png"]
category = "Developer Tool"
short_description = "My awesome application"

[package.metadata.bundle.linux.deb]
depends = ["libc6"]

[package.metadata.bundle.linux.rpm]
requires = ["glibc"]
```

## Building Locally

### Prerequisites

- **Rust nightly** (edition 2024): `rustup install nightly && rustup default nightly`
- **Git**: For version control operations
- **Platform-specific tools**:
  - **Linux**: dpkg-dev, rpm, fakeroot
  - **macOS**: Xcode Command Line Tools
  - **Windows**: WiX Toolset, NSIS

### Build Commands

```bash
# Build release binary
cargo build --release

# Run tests
cargo test

# Format code
cargo fmt

# Lint
cargo clippy -- -D warnings
```

### Cross-Platform Bundling via Docker

For creating Linux/Windows bundles from macOS (or vice versa), use Docker:

```bash
# Build Docker image (includes Wine, WiX, NSIS)
kodegen_bundler_release bundle --platform msi --rebuild-image

# Create Windows MSI from Linux/macOS
kodegen_bundler_release bundle --platform msi

# Create Linux packages from macOS
kodegen_bundler_release bundle --platform deb
```

The tool automatically detects when cross-platform bundling is needed and uses Docker containers with appropriate toolchains.

## Architecture Highlights

### Isolated Release Strategy

All release operations execute in **temporary clones** to ensure your working directory remains untouched:

- Clone created at `/tmp/kodegen-release-{timestamp}/`
- Active temp path saved to `~/.kodegen-temp-release` for resume support
- Automatic cleanup after completion (unless `--keep-temp`)
- Resume capability across sessions

### Dependency-Ordered Publishing

The tool analyzes your workspace dependency graph and publishes packages in the correct order:

```
Tier 0: [utils, schema]           ← No dependencies
Tier 1: [mcp-tool, mcp-client]    ← Depends on Tier 0
Tier 2: [tools-git, tools-fs]     ← Depends on Tier 1
Tier 3: [kodegen]                 ← Depends on Tier 2
```

Packages within the same tier publish in parallel (configurable concurrency), while tiers execute sequentially.

### State-Based Resume

Release progress is tracked in `.cyrup_release_state.json` with phases:

1. Validation
2. VersionUpdate
3. GitOperations
4. GitHubRelease
5. Publishing
6. Completed

If a release is interrupted, `resume` continues from the last successful checkpoint.

### Format-Preserving TOML Editing

Version updates preserve your `Cargo.toml` formatting using `toml_edit`:

- Comments preserved
- Custom formatting maintained
- Whitespace unchanged
- Only version fields modified

## Troubleshooting

### "Binary not found in target/release"

**Solution**: Build binaries before bundling:

```bash
cargo build --release --workspace
# OR let bundler build automatically
kodegen_bundler_release bundle  # No --no-build flag
```

### "Another release is in progress"

**Solution**: Resume or clean up the existing release:

```bash
kodegen_bundler_release resume
# OR
kodegen_bundler_release cleanup
```

### "GitHub token not found"

**Solution**: Set the required environment variable:

```bash
export GITHUB_TOKEN=ghp_your_token_here
# OR add to ~/.bashrc or ~/.zshrc
```

### macOS Code Signing Fails

**Solution**: Verify credentials are loaded:

```bash
echo $APPLE_TEAM_ID
echo $APPLE_CERTIFICATE | base64 -d | openssl pkcs12 -info -nodes -passin pass:$APPLE_CERTIFICATE_PASSWORD
```

### Docker Build Failures

**Solution**: Increase Docker memory limit:

```bash
kodegen_bundler_release bundle --platform msi --docker-memory 4096
```

## Development

### Project Structure

```
kodegen-bundler-release/
├── src/
│   ├── bundler/         # Platform-specific bundling logic
│   ├── cli/             # Command parsing and orchestration
│   ├── error/           # Error types and handling
│   ├── git/             # Git operations (via gix)
│   ├── github/          # GitHub API integration
│   ├── publish/         # crates.io publishing logic
│   ├── state/           # Release state persistence
│   ├── version/         # Version bumping and TOML editing
│   └── workspace/       # Workspace analysis and graphs
├── Cargo.toml
└── README.md
```

### Running Tests

```bash
# All tests
cargo test

# Specific module
cargo test --lib workspace::tests

# With output
cargo test -- --nocapture
```

### Debug Logging

```bash
RUST_LOG=debug kodegen_bundler_release release patch --dry-run
```

## Contributing

Contributions are welcome! Please:

1. Fork the repository
2. Create a feature branch: `git checkout -b feature/my-feature`
3. Make your changes with tests
4. Format code: `cargo fmt`
5. Lint: `cargo clippy -- -D warnings`
6. Submit a pull request

### Code Standards

- Edition 2024 Rust
- `#![deny(unsafe_code)]` except for audited FFI calls
- Comprehensive error messages with recovery suggestions
- All public APIs documented

## License

Dual-licensed under **Apache-2.0 OR MIT**.

See [LICENSE.md](LICENSE.md) for details.

## Credits

Part of the [KODEGEN.ᴀɪ](https://kodegen.ai) project - blazing-fast MCP tools for AI-powered code generation.

### Key Dependencies

- [gix](https://github.com/Byron/gitoxide) - Pure Rust Git implementation
- [clap](https://github.com/clap-rs/clap) - Command-line argument parsing
- [toml_edit](https://github.com/toml-rs/toml) - Format-preserving TOML editing
- [petgraph](https://github.com/petgraph/petgraph) - Graph algorithms for dependency ordering
- [reqwest](https://github.com/seanmonstar/reqwest) - HTTP client for GitHub API

## Support

- **Issues**: [GitHub Issues](https://github.com/cyrup-ai/kodegen-bundler-release/issues)
- **Documentation**: [docs.rs](https://docs.rs/kodegen_bundler_release)
- **Website**: [kodegen.ai](https://kodegen.ai)
