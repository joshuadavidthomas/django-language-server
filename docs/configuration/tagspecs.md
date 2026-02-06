# TagSpecs

Configure custom template tag specifications to extend Django Language Server's understanding of your custom template tags.

## Overview

[TagSpecs](https://github.com/joshuadavidthomas/djtagspecs) (Tag Specifications) define the structure and behavior of Django template tags, enabling the language server to provide:

- Autocompletion with context-aware snippets
- Validation and diagnostics for tag arguments
- Block tag matching and nesting validation
- Custom tag documentation on hover

Django Language Server includes built-in TagSpecs for Django's standard template tags and popular third-party libraries. You only need to define TagSpecs for your custom template tags.

!!! note "Specification Reference"

    Django Language Server implements the [TagSpecs v0.6.0 specification](https://github.com/joshuadavidthomas/djtagspecs/tree/v0.6.0). See the specification repository for complete schema documentation.

## Configuration

TagSpecs can be configured in your project's `djls.toml`, `.djls.toml`, or `pyproject.toml` file:

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

    In `pyproject.toml`, prefix all tables with `tool.djls.` - otherwise the structure is identical.

### Tag types

- `"block"` - Block tag with opening and closing tags (e.g., `{% mytag %}...{% endmytag %}`)
- `"standalone"` - Single tag with no closing tag (e.g., `{% mytag %}`)
- `"loader"` - Loader tag that may optionally behave as block (e.g., `{% extends %}`)

### Argument kinds

The `kind` field defines the semantic role of an argument:

- `"literal"` - Exact literal token (e.g., `"reversed"`)
- `"syntax"` - Mandatory syntactic keyword (e.g., `"in"`, `"as"`)
- `"variable"` - Template variable or filter expression
- `"any"` - Any template expression or literal
- `"assignment"` - Variable assignment pattern
- `"modifier"` - Boolean modifier flag
- `"choice"` - Choice from specific literals (requires `extra.choices`)

## Examples

!!! note

    All examples below use `djls.toml` format. For `pyproject.toml`, prefix all tables with `tool.djls.`

### Block tag with intermediates

```toml
[[tagspecs.libraries]]
module = "myapp.templatetags.custom"

[[tagspecs.libraries.tags]]
name = "switch"
type = "block"

[tagspecs.libraries.tags.end]
name = "endswitch"

[[tagspecs.libraries.tags.intermediates]]
name = "case"

[[tagspecs.libraries.tags.intermediates]]
name = "default"

[[tagspecs.libraries.tags.args]]
name = "value"
kind = "variable"
```

### Tag with syntax keywords

```toml
[[tagspecs.libraries.tags]]
name = "assign"
type = "standalone"

[[tagspecs.libraries.tags.args]]
name = "value"
kind = "any"

[[tagspecs.libraries.tags.args]]
name = "as"
kind = "syntax"

[[tagspecs.libraries.tags.args]]
name = "varname"
kind = "variable"
```

### Tag with choice arguments

```toml
[[tagspecs.libraries.tags]]
name = "cache"
type = "block"

[tagspecs.libraries.tags.end]
name = "endcache"

[[tagspecs.libraries.tags.args]]
name = "timeout"
kind = "variable"

[[tagspecs.libraries.tags.args]]
name = "mode"
kind = "choice"

[tagspecs.libraries.tags.args.extra]
choices = ["public", "private"]
```

### Standalone tag

```toml
[[tagspecs.libraries.tags]]
name = "render_widget"
type = "standalone"

[[tagspecs.libraries.tags.args]]
name = "widget_name"
kind = "variable"

[[tagspecs.libraries.tags.args]]
name = "options"
kind = "any"
required = false
```

## Migration from v0.4.0

The v0.6.0 format introduces a hierarchical structure that better represents how Django organizes template tags into libraries.

The migration to the new version will follow the [breaking changes policy](../versioning.md#breaking-changes), with this deprecation timeline:

- **v6.0.0** (current): Old format supported with deprecation warnings
- **v6.1.0**: Old format still supported with deprecation warnings
- **v6.2.0**: Old format **removed** - you must migrate to v0.6.0

!!! info "Version Timeline Note"

    The deprecation was introduced in v5.2.5 (unreleased). When Django 6.0 was officially released, the language server bumped from v5.2.4 directly to v6.0.0 per [DjangoVer versioning](../versioning.md). The two-release deprecation policy continues uninterrupted across this version boundary: v6.0.0 and v6.1.0 serve as the two warning releases before removal in v6.2.0.

Here are the key changes:

1. **Hierarchical Structure**: Tags are now grouped by library module
    - Old: `[[tagspecs]]` (flat array of tags, each with `module` field)
    - New: `[[tagspecs.libraries]]` containing `[[tagspecs.libraries.tags]]`
2. **Tag Type Classification**: Tags now have an explicit `type` field
    - Old: Implicitly determined by presence of `end_tag`
    - New: Explicit `type = "block"`, `"standalone"`, or `"loader"`
3. **Argument Kind vs Type**: Semantic role separated from positional/keyword designation
    - Old: `args = [{ name = "foo", type = "variable" }]`
    - New: `args = [{ name = "foo", kind = "variable" }]`
    - The `type` field now means positional vs keyword (`"both"`, `"positional"`, `"keyword"`)
4. **End Tag Optional → Required**: Inverted boolean for clarity
    - Old: `end_tag = { name = "endif", optional = false }`
    - New: `end = { name = "endif", required = true }`
5. **Renamed Fields**:
    - `end_tag` → `end`
    - `intermediate_tags` → `intermediates`
6. **Choice Arguments**: Moved to extra metadata
    - Old: `type = { choice = ["on", "off"] }`
    - New: `kind = "choice"` with `extra.choices = ["on", "off"]`

If you encounter issues during migration, please [open an issue](https://github.com/joshuadavidthomas/django-language-server/issues) with your tagspec configuration.

If you believe djls template validation is incorrect compared to Django runtime behavior (false positives or false negatives), please use the [Template Validation Mismatch](https://github.com/joshuadavidthomas/django-language-server/issues/new?template=template-validation-mismatch.yml) form.

### Argument type mapping


| Old `type` | New `kind` | Notes |
|------------|------------|-------|
| `"literal"` | `"literal"` or `"syntax"` | Use `"syntax"` for mandatory tokens like `"in"`, `"as"` |
| `"variable"` | `"variable"` | No change |
| `"string"` | `"variable"` | Strings are just variables in v0.6.0 |
| `"expression"` | `"any"` | Renamed for clarity |
| `"assignment"` | `"assignment"` | No change |
| `"varargs"` | `"any"` | Use count or omit for variable-length |
| `{ choice = [...] }` | `"choice"` | Choices moved to `extra.choices` |

### Examples

#### Simple block tag

**Old format (v0.4.0) - DEPRECATED:**
```toml
[[tagspecs]]
name = "block"
module = "django.template.defaulttags"
end_tag = { name = "endblock", optional = false }
args = [
    { name = "name", type = "variable" }
]
```

**New format (v0.6.0):**
```toml
[tagspecs]
version = "0.6.0"

[[tagspecs.libraries]]
module = "django.template.defaulttags"

[[tagspecs.libraries.tags]]
name = "block"
type = "block"

[tagspecs.libraries.tags.end]
name = "endblock"
required = true

[[tagspecs.libraries.tags.args]]
name = "name"
kind = "variable"
```

#### Multiple tags from same module

**Old format (v0.4.0) - DEPRECATED:**
```toml
[[tagspecs]]
name = "tag1"
module = "myapp.tags"
args = [{ name = "arg1", type = "variable" }]

[[tagspecs]]
name = "tag2"
module = "myapp.tags"
args = [{ name = "arg2", type = "literal" }]
```

**New format (v0.6.0):**
```toml
[tagspecs]
version = "0.6.0"

[[tagspecs.libraries]]
module = "myapp.tags"

[[tagspecs.libraries.tags]]
name = "tag1"
type = "standalone"

[[tagspecs.libraries.tags.args]]
name = "arg1"
kind = "variable"

[[tagspecs.libraries.tags]]
name = "tag2"
type = "standalone"

[[tagspecs.libraries.tags.args]]
name = "arg2"
kind = "literal"
```

#### Choice arguments

**Old format (v0.4.0) - DEPRECATED:**
```toml
[[tagspecs]]
name = "autoescape"
module = "django.template.defaulttags"
end_tag = { name = "endautoescape" }
args = [
    { name = "mode", type = { choice = ["on", "off"] } }
]
```

**New format (v0.6.0):**
```toml
[tagspecs]
version = "0.6.0"

[[tagspecs.libraries]]
module = "django.template.defaulttags"

[[tagspecs.libraries.tags]]
name = "autoescape"
type = "block"

[tagspecs.libraries.tags.end]
name = "endautoescape"

[[tagspecs.libraries.tags.args]]
name = "mode"
kind = "choice"

[tagspecs.libraries.tags.args.extra]
choices = ["on", "off"]
```
