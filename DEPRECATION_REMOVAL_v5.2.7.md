# TagSpecs v0.4.0 Deprecation Removal Checklist

**Target Version:** v5.2.7
**Timeline:** Remove after v5.2.5 and v5.2.6 releases

This document provides a step-by-step checklist for removing the deprecated TagSpecs v0.4.0 format support.

## Removal Checklist

### 1. Delete Legacy Module

- [ ] Delete file: `crates/djls-conf/src/tagspecs/legacy.rs`

### 2. Update `tagspecs.rs`

- [ ] Remove line: `pub mod legacy;` from `crates/djls-conf/src/tagspecs.rs`
- [ ] Remove the deprecation comment above it

### 3. Update `lib.rs`

- [ ] Remove the `deserialize_tagspecs` function from `crates/djls-conf/src/lib.rs`
- [ ] Remove the deprecation comment above it
- [ ] Change `Settings` struct field from:
  ```rust
  #[serde(default, deserialize_with = "deserialize_tagspecs")]
  tagspecs: TagSpecDef,
  ```
  to:
  ```rust
  #[serde(default)]
  tagspecs: TagSpecDef,
  ```

### 4. Update Dependencies

- [ ] Consider if `tracing` is still needed in `crates/djls-conf/Cargo.toml`
  - If only used for deprecation warning, remove it
  - If used elsewhere, keep it

### 5. Remove Tests

- [ ] Remove the entire `mod legacy_format` test section from `crates/djls-conf/src/lib.rs`
  - Look for the comment: `// DEPRECATION TESTS: Remove in v5.2.7`
  - Delete everything in the `mod legacy_format` module

### 6. Update Documentation

- [ ] Remove deprecation warning from `crates/djls-conf/TAGSPECS.md`
  - Delete the "DEPRECATED FORMAT" callout at the top
  - Delete the entire "Migration from v0.4.0" section at the end

- [ ] Remove deprecation warning from `docs/configuration.md`
  - Delete the "DEPRECATED FORMAT" warning in the `tagspecs` section

### 7. Update CHANGELOG

- [ ] Add removal notice to CHANGELOG.md under v5.2.7:
  ```markdown
  ## [5.2.7]

  ### Removed

  - TagSpecs v0.4.0 flat format support (deprecated in v5.2.5)
  ```

- [ ] Move the deprecation notice from "Deprecated" section to "Removed" section

### 8. Verification

- [ ] Run all tests: `cargo test`
- [ ] Build project: `cargo build`
- [ ] Try using old format in config (should fail with clear error)
- [ ] Verify new format still works

### 9. Cleanup

- [ ] Delete this checklist file: `DEPRECATION_REMOVAL_v5.2.7.md`

## Files Affected

Summary of all files that will be modified:

1. **Deleted:**
   - `crates/djls-conf/src/tagspecs/legacy.rs`
   - `DEPRECATION_REMOVAL_v5.2.7.md` (this file)

2. **Modified:**
   - `crates/djls-conf/src/tagspecs.rs`
   - `crates/djls-conf/src/lib.rs`
   - `crates/djls-conf/Cargo.toml` (maybe)
   - `crates/djls-conf/TAGSPECS.md`
   - `docs/configuration.md`
   - `CHANGELOG.md`

## Notes

- All legacy code is isolated in clearly marked sections
- Removing these sections should not affect any other functionality
- The new v0.5.0 format implementation remains unchanged
