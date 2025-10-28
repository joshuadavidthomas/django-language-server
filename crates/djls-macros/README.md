# djls-macros

Proc macros for the Django Language Server.

## `#[derive(Diagnostic)]`

This derive macro automatically generates a `diagnostic_code()` method for error enums based on `#[diagnostic]` attributes.

### Usage

```rust
use djls_macros::Diagnostic;

#[derive(Diagnostic)]
pub enum MyError {
    #[diagnostic(code = "E001", category = "semantic")]
    FirstError { message: String },

    #[diagnostic(code = "E002", category = "template")]
    SecondError,
}
```

This generates:

```rust
impl MyError {
    pub fn diagnostic_code(&self) -> &'static str {
        match self {
            MyError::FirstError { .. } => "E001",
            MyError::SecondError => "E002",
        }
    }
}
```

### Attributes

Each enum variant must have a `#[diagnostic]` attribute with:
- `code`: The diagnostic code (e.g., "S105", "T100")
- `category`: The category (e.g., "semantic", "template")

The `category` field is parsed but not used by the macro itself - it's available for build scripts and documentation generation.

### Error Handling

The macro will emit a compile error if:
- Applied to anything other than an enum
- A variant is missing the `#[diagnostic]` attribute
- The `code` field is missing from a `#[diagnostic]` attribute

This ensures all error variants have diagnostic codes at compile time.
