# TagSpecs

Tag Specifications (TagSpecs) define how template tags are structured, helping the language server understand template syntax for features like block completion and diagnostics.

## Schema

Tag Specifications (TagSpecs) define how tags are parsed and understood. They allow the parser to handle custom tags without hard-coding them and provide rich autocompletion with LSP snippets.

```toml
[[path.to.module]]  # Array of tables for the module, e.g., tagspecs.django.template.defaulttags
name = "tag_name"   # The tag name (e.g., "if", "for", "my_custom_tag")
end_tag = { name = "end_tag_name", optional = false }  # Optional: Defines the closing tag
intermediate_tags = [{ name = "tag_name" }, ...]       # Optional: Defines intermediate tags
args = [                                                # Defines tag arguments for validation and snippets
    { name = "arg_name", type = "arg_type", required = true }
]
```

### Core Fields

The `name` field specifies the tag name (e.g., "if", "for", "my_custom_tag").

The `end_tag` table defines the closing tag for a block tag:
- `name`: The name of the closing tag (e.g., "endif")
- `optional`: Whether the closing tag is optional (defaults to `false`)
- `args`: Optional array of arguments for the end tag (e.g., endblock can take a name)

The `intermediate_tags` array lists tags that can appear between the opening and closing tags. Each intermediate tag is an object with:
- `name`: The name of the intermediate tag (e.g., "else", "elif")

### Argument Specification

The `args` array defines the expected arguments for a tag. Each argument has:
- `name`: The argument name (used as placeholder text in LSP snippets)
- `type`: The argument type (see below)
- `required`: Whether the argument is required (defaults to `true`)

#### Argument Types

- `"literal"`: A literal keyword that must appear exactly (e.g., "in", "as", "by")
- `"variable"`: A template variable name
- `"string"`: A string literal (will be wrapped in quotes in snippets)
- `"expression"`: A template expression or condition
- `"assignment"`: A variable assignment (e.g., "var=value")
- `"varargs"`: Variable number of arguments
- `{ choice = ["option1", "option2"] }`: A choice from specific options (generates choice snippets)

## Configuration

- **Built-in TagSpecs**: The parser includes TagSpecs for Django's built-in tags and popular third-party tags. These are provided by `djls-templates` automatically; users do not need to define them. The examples below show the format, but you only need to create files for your *own* custom tags or to override built-in behavior.
- **User-defined TagSpecs**: Users can expand or override TagSpecs via `pyproject.toml` or `djls.toml` files in their project, allowing custom tags and configurations to be seamlessly integrated.

## Examples

### If Tag

```toml
[[tagspecs.django.template.defaulttags]]
name = "if"
end_tag = { name = "endif" }
intermediate_tags = [{ name = "elif" }, { name = "else" }]
args = [
    { name = "condition", type = "expression" }
]
# Generates snippet: if ${1:condition}
```

### For Tag

```toml
[[tagspecs.django.template.defaulttags]]
name = "for"
end_tag = { name = "endfor" }
intermediate_tags = [{ name = "empty" }]
args = [
    { name = "item", type = "variable" },
    { name = "in", type = "literal" },
    { name = "items", type = "variable" },
    { name = "reversed", required = false, type = "literal" }
]
# Generates snippet: for ${1:item} in ${2:items} ${3:reversed}
```

### Autoescape Tag

```toml
[[tagspecs.django.template.defaulttags]]
name = "autoescape"
end_tag = { name = "endautoescape" }
args = [
    { name = "mode", type = { choice = ["on", "off"] } }
]
# Generates snippet: autoescape ${1|on,off|}
```

### URL Tag with Optional Arguments

```toml
[[tagspecs.django.template.defaulttags]]
name = "url"
args = [
    { name = "view_name", type = "string" },
    { name = "args", required = false, type = "varargs" },
    { name = "as", required = false, type = "literal" },
    { name = "varname", required = false, type = "variable" }
]
# Generates snippet: url "${1:view_name}" ${2:args} ${3:as} ${4:varname}
```

### Custom Tag

```toml
[[tagspecs.my_module.templatetags.my_tags]]
name = "my_custom_tag"
end_tag = { name = "endmycustomtag", optional = true }
intermediate_tags = [{ name = "myintermediate" }]
args = [
    { name = "param1", type = "variable" },
    { name = "with", type = "literal" },
    { name = "param2", type = "string" }
]
# Generates snippet: my_custom_tag ${1:param1} with "${2:param2}"
```

### Standalone Tags

```toml
[[tagspecs.django.template.defaulttags]]
name = "csrf_token"
args = []  # No arguments
# Generates snippet: csrf_token

[[tagspecs.django.template.defaulttags]]
name = "load"
args = [
    { name = "libraries", type = "varargs" }
]
# Generates snippet: load ${1:libraries}
```
