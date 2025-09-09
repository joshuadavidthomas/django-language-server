# TagSpecs

Tag Specifications (TagSpecs) define how template tags are structured, helping the language server understand template syntax for features like block completion and diagnostics.

## Schema

Tag Specifications (TagSpecs) define how tags are parsed and understood. They allow the parser to handle custom tags without hard-coding them.

```toml
[[path.to.module]]  # Array of tables for the module, e.g., tagspecs.django.template.defaulttags
name = "tag_name"   # The tag name (e.g., "if", "for", "my_custom_tag")
end_tag = { name = "end_tag_name", optional = false }  # Optional: Defines the closing tag
intermediate_tags = [{ name = "tag_name" }, ...]       # Optional: Defines intermediate tags
args = { min = 1, max = 3 }                            # Optional: Argument constraints
```

The `name` field specifies the tag name (e.g., "if", "for", "my_custom_tag").

The `end_tag` table defines the closing tag for a block tag.
- `name`: The name of the closing tag (e.g., "endif").
- `optional`: Whether the closing tag is optional (defaults to `false`).
- `args`: Optional argument constraints for the end tag.

The `intermediate_tags` array lists tags that can appear between the opening and closing tags. Each intermediate tag is an object with:
- `name`: The name of the intermediate tag (e.g., "else", "elif").

The `args` table defines argument constraints:
- `min`: Minimum number of arguments required.
- `max`: Maximum number of arguments allowed.

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
args = { min = 1 }  # condition
```

### For Tag

```toml
[[tagspecs.django.template.defaulttags]]
name = "for"
end_tag = { name = "endfor" }
intermediate_tags = [{ name = "empty" }]
args = { min = 3 }  # item in items (at minimum)
```

### Autoescape Tag

```toml
[[tagspecs.django.template.defaulttags]]
name = "autoescape"
end_tag = { name = "endautoescape" }
args = { min = 1, max = 1 }  # on or off
```

### Custom Tag

```toml
[[tagspecs.my_module.templatetags.my_tags]]
name = "my_custom_tag"
end_tag = { name = "endmycustomtag", optional = true }
intermediate_tags = [{ name = "myintermediate" }]
```

### Standalone Tags (no end tag)

```toml
[[tagspecs.django.template.defaulttags]]
name = "csrf_token"
args = { min = 0, max = 0 }  # no arguments

[[tagspecs.django.template.defaulttags]]
name = "load"
args = { min = 1 }  # library name(s)
```
