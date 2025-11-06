# Task 01: Fix Binary Discovery in Metadata Module

## Problem

`kodegen-bundler-release/src/metadata.rs` has a broken/simplified implementation that doesn't handle `[[bin]]` sections in Cargo.toml.

**Current code** (line 49-50):
```rust
// Binary name defaults to package name (simplified - doesn't handle [[bin]] sections)
let binary_name = name.clone();
```

This causes bundler to look for binaries with the wrong name, resulting in:
```
Error: Binary not found at .../target/release/kodegen_tools_filesystem
```

## Solution

Replace with the CORRECT implementation from `kodegen-bundler-bundle/src/metadata/mod.rs` (lines 122-142):

```rust
// Try [[bin]] section first
let binary_name = toml_value
    .get("bin")
    .and_then(|v| v.as_array())
    .and_then(|arr| arr.first())
    .and_then(|first| first.get("name"))
    .and_then(|v| v.as_str())
    .map(String::from)
    .or_else(|| {
        // Fallback to package name
        package
            .get("name")
            .and_then(|v| v.as_str())
            .map(String::from)
    })
    .ok_or_else(|| {
        BundlerError::Cli(CliError::InvalidArguments {
            reason: "No binary found in Cargo.toml".to_string(),
        })
    })?;
```

## Implementation Steps

1. **Read the working implementation**:
   - File: `kodegen-bundler-bundle/src/metadata/mod.rs`
   - Function: `load_manifest()` (lines 38-148)
   - This version correctly parses TOML once and extracts both metadata AND binary name

2. **Replace broken implementation**:
   - File: `kodegen-bundler-release/src/metadata.rs`
   - Function: `load_manifest()` (lines 23-59)
   - Replace with working version from bundler-bundle

3. **Update struct definitions** if needed:
   - Ensure `Manifest` struct matches between the two files
   - Ensure `PackageMetadata` struct matches

4. **Verify error types are compatible**:
   - Both use `BundlerError::Cli(CliError::InvalidArguments)`
   - Should be compatible

5. **Test with actual package**:
   - Run release workflow on kodegen-tools-filesystem
   - Verify binary is discovered correctly
   - Verify bundling succeeds

## Files to Modify

- `/Users/davidmaple/kodegen-workspace/kodegen-bundler-release/src/metadata.rs`

## Dependencies

- None - this is a self-contained fix

## Testing

```bash
cd /Users/davidmaple/kodegen-workspace/kodegen-bundler-release
cargo run -- /Users/davidmaple/kodegen-workspace/kodegen-tools-filesystem
```

Should succeed without "Binary not found" error.

## Success Criteria

- [ ] Binary discovery works for packages with [[bin]] sections
- [ ] Binary discovery still works for simple packages (fallback to package name)
- [ ] Release workflow completes bundling phase successfully
- [ ] No compilation errors or warnings
