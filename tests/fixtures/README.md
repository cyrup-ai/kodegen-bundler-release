# Mock Binary Fixtures

This directory contains minimal Rust binaries for testing the bundling process without requiring a full 1.5 hour compilation of the real codebase.

## Purpose

- **Fast iteration**: Test bundling logic in <1 minute vs 1.5+ hours
- **CI/CD testing**: Validate package creation without expensive builds
- **Development**: Iterate quickly on bundling configuration

## Usage

### Build mock binaries
```bash
cd packages/bundler-release/tests/fixtures
cargo build --release
```

This creates tiny (~200KB) binaries in `target/release/`:
- `mock-kodegen` - Simulates main binary
- `mock-kodegend` - Simulates daemon binary

### Test bundling with mocks
```bash
# From repository root
./scripts/test-bundling.sh
```

This will:
1. Build mock binaries (~5 seconds)
2. Run bundler with mock binaries
3. Validate package creation (.deb, .rpm, .msi, etc.)
4. Complete in <1 minute total

## What's Tested

- Package creation logic (all formats)
- File placement and permissions
- Metadata generation
- Template rendering
- Cross-platform bundling

## What's NOT Tested

- Actual binary functionality (these are no-op programs)
- Full binary size and optimization
- Real dependencies and linking
