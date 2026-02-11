# TagSpecs

Configure custom template tag specifications as a fallback when automatic extraction can't fully infer a tag's structure or arguments.

## Overview

Django Language Server primarily derives tag structure and argument rules automatically from Python source code via static AST analysis.

For edge cases (dynamic tags, unusual registration patterns, complex parsing, proprietary code), you can provide **TagSpecs** as a manual fallback. When present, TagSpecs can improve:

- Block tag matching and nesting validation
- Validation and diagnostics for tag arguments
- Argument autocompletion and context-aware snippets

!!! important "Precedence"

    Automatic extraction always wins. TagSpecs are only used to fill missing information.

## Configuration

TagSpecs can be configured in your project's `djls.toml`, `.djls.toml`, or `pyproject.toml` file.

=== "`djls.toml`"

    ```toml
    [tagspecs]
    version = "0.6.0"

    [[tagspecs.libraries]]
    module = "myapp.templatetags.custom"

    [[tagspecs.libraries.tags]]
    name = "highlight"
    type = "block"

    [tagspecs.libraries.tags.end]
    name = "endhighlight"

    [[tagspecs.libraries.tags.args]]
    name = "language"
    kind = "variable"
    ```

=== "`pyproject.toml`"

    ```toml
    [tool.djls.tagspecs]
    version = "0.6.0"

    [[tool.djls.tagspecs.libraries]]
    module = "myapp.templatetags.custom"

    [[tool.djls.tagspecs.libraries.tags]]
    name = "highlight"
    type = "block"

    [tool.djls.tagspecs.libraries.tags.end]
    name = "endhighlight"

    [[tool.djls.tagspecs.libraries.tags.args]]
    name = "language"
    kind = "variable"
    ```

    In `pyproject.toml`, prefix all tables with `tool.djls.`.

### Tag types

- `"block"` - Block tag with opening and closing tags (e.g., `{% mytag %}...{% endmytag %}`)
- `"standalone"` - Single tag with no closing tag (e.g., `{% mytag %}`)
- `"loader"` - Loader tag that may optionally behave as block (e.g., `{% extends %}`)

### Argument kinds

- `"literal"` - Exact literal token (e.g., `"reversed"`)
- `"syntax"` - Mandatory syntactic keyword (e.g., `"in"`, `"as"`)
- `"variable"` - Template variable or filter expression
- `"any"` - Any template expression or literal
- `"assignment"` - Variable assignment pattern
- `"modifier"` - Boolean modifier flag
- `"choice"` - Choice from specific literals (requires `extra.choices`)

## Migration from v0.4.0

The legacy v0.4.0 flat TagSpecs format is still supported for compatibility, but is deprecated.

- **v6.0.0**: legacy format deprecated (warnings)
- **v6.0.2**: legacy format removed

If you are still using the legacy format, migrate to v0.6.0.
