# djls-ide

IDE integration layer for the Django Language Server.

## Diagnostic Rules

Diagnostic rules (error codes like S105, T100, etc.) are defined in `diagnostics.toml` and automatically integrated into the codebase and documentation through a build script.

### How it works

1. **Source of Truth**: `diagnostics.toml` contains all diagnostic rule definitions including:
   - Code (e.g., "S105")
   - Category (template or semantic)
   - Error type mapping (e.g., "ValidationError::TooManyArguments")
   - Name and description
   - Detailed explanation with examples

2. **Build-time Generation**: `build.rs` reads the TOML and generates:
   - A simple lookup table in `diagnostic_codes.rs` (generated in `OUT_DIR`)
   - Individual markdown files for each rule in `/docs/rules/`
   - An index page at `/docs/rules/index.md`

3. **Code Integration**: `src/diagnostics.rs` includes the lookup table and trait implementations use it to map error variants to diagnostic codes

### Adding a new diagnostic rule

1. Add a new `[[rule]]` section to `diagnostics.toml`
2. Fill in all required fields
3. Rebuild the project - the code and docs are automatically updated

Example:
```toml
[[rule]]
code = "S108"
category = "semantic"
error_type = "ValidationError::NewErrorType"
name = "New Error"
description = "Brief description of the error"
severity = "error"
explanation = """
Detailed explanation with examples.
"""
```

### Multiple error types for one code

If multiple error variants should map to the same diagnostic code, separate them with `|`:

```toml
error_type = "ValidationError::Type1|ValidationError::Type2"
```

### File locations

- **Rule definitions**: `crates/djls-ide/diagnostics.toml`
- **Build script**: `crates/djls-ide/build.rs`
- **Generated code**: `target/.../build/djls-ide-.../out/diagnostic_codes.rs` (not checked in)
- **Generated docs**: `docs/rules/*.md` (checked in)
- **Code that uses rules**: `crates/djls-ide/src/diagnostics.rs`

### Why this approach?

This design ensures:
- **Single source of truth**: No manual synchronization needed between code and docs
- **Data-driven**: The crate reads from generated lookup data rather than having logic generated
- **Maintainable**: Trait implementations remain in the crate where they can be easily read
- **Flexible**: Individual rule pages make it easy to link directly to specific error codes
- **Consistency**: All rules follow the same format and structure
- **Easy updates**: Adding or updating rules only requires editing the TOML file
