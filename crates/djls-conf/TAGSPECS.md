# TagSpecs

Tag Specifications (TagSpecs) define how template tags are structured, helping the language server understand template syntax for features like block completion and diagnostics.

> **⚠️ DEPRECATED FORMAT (v0.4.0)**
>
> If you're using the old flat `[[tagspecs]]` format, please migrate to the new v0.5.0 hierarchical format.
> The old format still works but will be **removed in v5.2.7**.
>
> 👉 See [Migration from v0.4.0](#migration-from-v040) below for the migration guide.

## Schema (v0.5.0)

TagSpecs v0.5.0 uses a hierarchical structure that groups tags by their template library module.

```toml
[tagspecs]
version = "0.5.0"  # Specification version
engine = "django"  # Template engine (default: "django")

[[tagspecs.libraries]]
module = "django.template.defaulttags"  # Dotted Python import path

[[tagspecs.libraries.tags]]
name = "if"                    # Tag name
type = "block"                 # Tag type: "block", "standalone", or "loader"

[tagspecs.libraries.tags.end]
name = "endif"                 # End tag name
required = true                # Whether end tag must appear (default: true)

[[tagspecs.libraries.tags.intermediates]]
name = "elif"                  # Intermediate tag name

[[tagspecs.libraries.tags.args]]
name = "condition"             # Argument name
kind = "any"                   # Argument kind (semantic role)
required = true                # Whether argument is required (default: true)
```

### Root Fields

- **`version`** (default: `"0.5.0"`): Specification version
- **`engine`** (default: `"django"`): Template engine name
- **`requires_engine`** (optional): Engine version constraint (PEP 440 for Django)
- **`extends`** (optional): References to parent documents for overlay composition
- **`libraries`**: Array of tag library definitions

### Library Fields

Each library groups tags by their Python module:

- **`module`**: Dotted Python import path (e.g., `"django.template.defaulttags"`)
- **`requires_engine`** (optional): Engine version constraint for this library
- **`tags`**: Array of tag definitions
- **`extra`** (optional): Extra metadata for extensibility

### Tag Fields

- **`name`**: Tag name (e.g., `"if"`, `"for"`, `"my_custom_tag"`)
- **`type`**: Tag type classification:
  - `"block"`: Block tag with opening/closing tags (e.g., `{% if %}...{% endif %}`)
  - `"standalone"`: Standalone tag with no closing tag (e.g., `{% csrf_token %}`)
  - `"loader"`: Loader tag that may optionally behave as block (e.g., `{% extends %}`)
- **`end`** (optional): End tag specification (auto-synthesized for block tags if omitted)
- **`intermediates`** (optional): Array of intermediate tag definitions
- **`args`** (optional): Array of argument definitions
- **`extra`** (optional): Extra metadata

### End Tag Fields

- **`name`**: End tag name (e.g., `"endif"`)
- **`required`** (default: `true`): Whether the end tag must appear explicitly
- **`args`** (optional): End tag arguments
- **`extra`** (optional): Extra metadata

### Intermediate Tag Fields

- **`name`**: Intermediate tag name (e.g., `"elif"`, `"else"`, `"empty"`)
- **`args`** (optional): Intermediate tag arguments
- **`min`** (optional): Minimum occurrence count
- **`max`** (optional): Maximum occurrence count
- **`position`** (default: `"any"`): Positioning constraint (`"any"` or `"last"`)
- **`extra`** (optional): Extra metadata

### Argument Fields

- **`name`**: Argument name (used as placeholder text in LSP snippets)
- **`kind`**: Argument kind (semantic classification):
  - `"literal"`: Literal token (e.g., `"on"`, `"off"`)
  - `"variable"`: Template variable or filter expression
  - `"any"`: Any template expression or literal
  - `"syntax"`: Mandatory syntactic token (e.g., `"in"`, `"as"`)
  - `"assignment"`: Variable assignment (e.g., `"as varname"`)
  - `"modifier"`: Boolean modifier (e.g., `"reversed"`)
  - `"choice"`: Choice from specific literals (requires `extra.choices`)
- **`required`** (default: `true`): Whether the argument is required
- **`type`** (default: `"both"`): Argument type (`"both"`, `"positional"`, `"keyword"`)
- **`count`** (optional): Exact token count (null means variable)
- **`extra`** (optional): Extra metadata (e.g., `{ choices = ["on", "off"] }` for choice kind)

## Configuration

- **Built-in TagSpecs**: The parser includes TagSpecs for Django's built-in tags and popular third-party tags. These are provided by `djls-templates` automatically; users do not need to define them. The examples below show the format, but you only need to create files for your *own* custom tags or to override built-in behavior.
- **User-defined TagSpecs**: Users can expand or override TagSpecs via `pyproject.toml` or `djls.toml` files in their project, allowing custom tags and configurations to be seamlessly integrated.

## Examples

### Block Tag with Intermediates (if/elif/else)

```toml
[tagspecs]
version = "0.5.0"

[[tagspecs.libraries]]
module = "django.template.defaulttags"

[[tagspecs.libraries.tags]]
name = "if"
type = "block"

[tagspecs.libraries.tags.end]
name = "endif"

[[tagspecs.libraries.tags.intermediates]]
name = "elif"

[[tagspecs.libraries.tags.intermediates]]
name = "else"

[[tagspecs.libraries.tags.args]]
name = "condition"
kind = "any"
```

### Block Tag with Syntax and Modifiers (for)

```toml
[[tagspecs.libraries]]
module = "django.template.defaulttags"

[[tagspecs.libraries.tags]]
name = "for"
type = "block"

[tagspecs.libraries.tags.end]
name = "endfor"

[[tagspecs.libraries.tags.intermediates]]
name = "empty"

[[tagspecs.libraries.tags.args]]
name = "item"
kind = "variable"

[[tagspecs.libraries.tags.args]]
name = "in"
kind = "syntax"

[[tagspecs.libraries.tags.args]]
name = "items"
kind = "variable"

[[tagspecs.libraries.tags.args]]
name = "reversed"
kind = "modifier"
required = false
```

### Choice Arguments (autoescape)

```toml
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

### Standalone Tag (csrf_token)

```toml
[[tagspecs.libraries]]
module = "django.template.defaulttags"

[[tagspecs.libraries.tags]]
name = "csrf_token"
type = "standalone"
```

### Custom Tag in Your Project

```toml
[tool.djls.tagspecs]  # In pyproject.toml
version = "0.5.0"

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

[[tool.djls.tagspecs.libraries.tags.args]]
name = "linenos"
kind = "modifier"
required = false
```

## Migration from v0.4.0

The v0.5.0 format introduces a hierarchical structure that better represents how Django organizes template tags into libraries. Here are the key changes:

### Key Changes

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

### Migration Examples

#### Example 1: Simple Block Tag

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

**New format (v0.5.0):**
```toml
[tagspecs]
version = "0.5.0"

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

#### Example 2: Multiple Tags from Same Module

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

**New format (v0.5.0):**
```toml
[tagspecs]
version = "0.5.0"

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

#### Example 3: Choice Arguments

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

**New format (v0.5.0):**
```toml
[tagspecs]
version = "0.5.0"

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

#### Example 4: Argument Type Mapping

**Old argument types → New argument kinds:**

| Old `type` | New `kind` | Notes |
|------------|------------|-------|
| `"literal"` | `"literal"` or `"syntax"` | Use `"syntax"` for mandatory tokens like `"in"`, `"as"` |
| `"variable"` | `"variable"` | No change |
| `"string"` | `"variable"` | Strings are just variables in v0.5.0 |
| `"expression"` | `"any"` | Renamed for clarity |
| `"assignment"` | `"assignment"` | No change |
| `"varargs"` | `"any"` | Use count or omit for variable-length |
| `{ choice = [...] }` | `"choice"` | Choices moved to `extra.choices` |

### Deprecation Timeline

- **v5.2.5** (current): Old format supported with deprecation warnings
- **v5.2.6**: Old format still supported with deprecation warnings
- **v5.2.7**: Old format **removed** - you must migrate to v0.5.0

### Need Help?

If you encounter issues during migration, please [open an issue](https://github.com/joshuadavidthomas/django-language-server/issues) with your tagspec configuration.
