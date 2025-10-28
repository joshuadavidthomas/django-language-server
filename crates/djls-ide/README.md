# djls-ide

IDE integration layer for the Django Language Server.

## Diagnostic Rules

Diagnostic rules (error codes like S105, T100, etc.) are defined directly in the Rust error enums using attributes and doc comments. The build script extracts this information and generates both code and documentation.

### How it works

1. **Source of Truth**: Error enum variants in Rust source code with:
   - `#[diagnostic(code = "...", category = "...")]` attributes
   - Comprehensive doc comments with examples and fixes
   - Located in `djls-semantic/src/errors.rs` and `djls-templates/src/error.rs`

2. **Build-time Generation**: `build.rs` parses the Rust source and generates:
   - A lookup table in `diagnostic_codes.rs` (in `OUT_DIR`)
   - Individual markdown files for each rule in `/docs/rules/`
   - An index page at `/docs/rules/index.md`

3. **Code Integration**: `src/diagnostics.rs` includes the lookup table and trait implementations use it to map error variants to diagnostic codes

### Adding a new diagnostic rule

1. Add a new enum variant with the `#[diagnostic]` attribute and doc comment:

```rust
/// Too Many Arguments
///
/// This error occurs when a template tag is called with more arguments
/// than it accepts.
///
/// # Examples
///
/// ```django
/// {% csrf_token extra_arg %}  {# csrf_token takes no arguments #}
/// ```
///
/// # Fix
///
/// ```django
/// {% csrf_token %}
/// ```
#[diagnostic(code = "S105", category = "semantic")]
#[error("Tag '{tag}' accepts at most {max} argument{}")]
TooManyArguments { tag: String, max: usize, span: Span },
```

2. Add the variant to the match statement in `src/diagnostics.rs`

3. Rebuild - the docs are automatically generated

### Doc comment format

- **First line**: Title of the error (e.g., "Too Many Arguments")
- **Description paragraph**: Explain when this error occurs
- **`# Examples` section**: Show code that triggers the error
- **`# Fix` section**: Show how to fix it

The build script extracts these and formats them as markdown documentation.

### File locations

- **Rule definitions**: `crates/djls-semantic/src/errors.rs`, `crates/djls-templates/src/error.rs`
- **Build script**: `crates/djls-ide/build.rs`
- **Generated code**: `target/.../build/djls-ide-.../out/diagnostic_codes.rs` (not checked in)
- **Generated docs**: `docs/rules/*.md` (checked in)
- **Code that uses rules**: `crates/djls-ide/src/diagnostics.rs`

### Why this approach?

This design ensures:
- **Single source of truth**: All information lives with the error definition in Rust
- **Documentation at source**: Docs are right next to the code that uses them
- **Works with cargo doc**: Doc comments show up in generated Rust documentation
- **IDE support**: Developers see examples when hovering over error types
- **Data-driven**: The crate reads from generated lookup data
- **Flexible**: Individual rule pages make it easy to link to specific errors
- **Cannot drift**: No separate file to keep in sync
