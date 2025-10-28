# djls-ide

IDE integration layer for the Django Language Server.

## Diagnostic Rules

Diagnostic rules (error codes like S105, T100, etc.) are defined using `#[diagnostic]` attributes on error enum variants. The build script extracts these to generate user-facing documentation.

### How it works

1. **Source of Truth**: Error enum variants with:
   - `#[diagnostic(code = "...", category = "...")]` attributes
   - Comprehensive doc comments with examples and fixes
   - Located in `djls-semantic/src/errors.rs` and `djls-templates/src/error.rs`

2. **Build Script** (`build.rs`):
   - Parses Rust source files to extract attributes and doc comments
   - Generates individual markdown files for each rule in `/docs/rules/`
   - Creates an index page at `/docs/rules/index.md`

3. **Runtime Code**: Manual trait implementations in `src/diagnostics.rs` map error variants to their diagnostic codes

### Adding a new diagnostic rule

1. Add a new enum variant with the `#[diagnostic]` attribute and doc comment:

```rust
pub enum ValidationError {
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
}
```

2. Add the variant to the match statement in `src/diagnostics.rs`:

```rust
fn diagnostic_code(&self) -> &'static str {
    match self {
        ValidationError::TooManyArguments { .. } => "S105",
        // ...
    }
}
```

3. Rebuild - the build script generates documentation

### Doc comment format

- **First line**: Title of the error (e.g., "Too Many Arguments")
- **Description paragraph**: Explain when this error occurs
- **`# Examples` section**: Show code that triggers the error
- **`# Fix` section**: Show how to fix it

The build script extracts these and formats them as markdown documentation.

### File locations

- **Rule definitions**: `crates/djls-semantic/src/errors.rs`, `crates/djls-templates/src/error.rs`
- **Build script**: `crates/djls-ide/build.rs` (docs generation only)
- **Generated docs**: `docs/rules/*.md` (checked in)
- **Runtime code**: `crates/djls-ide/src/diagnostics.rs` (manual trait implementations)

### Why this approach?

This design ensures:
- **Single source of truth**: Attributes define codes, doc comments define documentation
- **Simple and clear**: Manual trait implementations are easy to understand
- **Documentation at source**: Docs are right next to the error definitions
- **Works with cargo doc**: Doc comments show up in generated Rust documentation
- **IDE support**: Developers see examples when hovering over error types
- **Build script only for user docs**: No runtime code generation
