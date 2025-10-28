# djls-ide

IDE integration layer for the Django Language Server.

## Diagnostic Rules

Diagnostic rules (error codes like S105, T100, etc.) are defined directly in the Rust error enums using attributes and doc comments. A proc macro automatically generates the necessary code, and the build script generates user-facing documentation.

### How it works

1. **Source of Truth**: Error enum variants in Rust source code with:
   - `#[derive(Diagnostic)]` on the enum
   - `#[diagnostic(code = "...", category = "...")]` attributes on each variant
   - Comprehensive doc comments with examples and fixes
   - Located in `djls-semantic/src/errors.rs` and `djls-templates/src/error.rs`

2. **Proc Macro** (`djls-macros`):
   - Generates a `diagnostic_code()` method implementation from the attributes
   - No manual match statements needed
   - Compile-time validation (missing attribute = compile error)

3. **Build Script** (`build.rs`):
   - Parses Rust source files to extract attributes and doc comments
   - **Only** generates documentation (not runtime code)
   - Creates individual markdown files for each rule in `/docs/rules/`
   - Creates an index page at `/docs/rules/index.md`

4. **Runtime Usage**: `src/diagnostics.rs` simply calls the generated `diagnostic_code()` method

### Adding a new diagnostic rule

1. Add a new enum variant with the `#[diagnostic]` attribute and doc comment:

```rust
#[derive(Error, Diagnostic, Serialize)]
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

2. Rebuild - everything is automatic:
   - Proc macro generates the `diagnostic_code()` method
   - Build script generates documentation

### Doc comment format

- **First line**: Title of the error (e.g., "Too Many Arguments")
- **Description paragraph**: Explain when this error occurs
- **`# Examples` section**: Show code that triggers the error
- **`# Fix` section**: Show how to fix it

The build script extracts these and formats them as markdown documentation.

### File locations

- **Rule definitions**: `crates/djls-semantic/src/errors.rs`, `crates/djls-templates/src/error.rs`
- **Proc macro**: `crates/djls-macros/src/lib.rs`
- **Build script**: `crates/djls-ide/build.rs` (docs only)
- **Generated docs**: `docs/rules/*.md` (checked in)
- **Runtime code**: `crates/djls-ide/src/diagnostics.rs` (calls generated methods)

### Why this approach?

This design ensures:
- **Single source of truth**: All information lives with the error definition in Rust
- **Zero duplication**: Proc macro generates code from attributes
- **Compile-time safety**: Missing attributes cause compile errors
- **Documentation at source**: Docs are right next to the code that uses them
- **Works with cargo doc**: Doc comments show up in generated Rust documentation
- **IDE support**: Developers see examples when hovering over error types
- **Build script only for docs**: No runtime code generation, just static documentation
- **Cannot drift**: No separate files to keep in sync
