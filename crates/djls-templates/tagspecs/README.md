# TagSpecs

Tag Specifications (TagSpecs) define how template tags are structured, helping the language server understand template syntax for features like block completion and diagnostics.

## Schema

Tag Specifications (TagSpecs) define how tags are parsed and understood. They allow the parser to handle custom tags without hard-coding them.

```toml
[path.to.tag_name]  # Path where tag is registered, e.g., django.template.defaulttags
end = { tag = "end_tag_name", optional = false } # Optional: Defines the closing tag
intermediates = ["intermediate_tag_name", ...]   # Optional: Defines intermediate tags (like else, elif)
```

The `end` table defines the closing tag for a block tag.
- `tag`: The name of the closing tag (e.g., "endif").
- `optional`: Whether the closing tag is optional (defaults to `false`).

The `intermediates` array lists tags that can appear between the opening and closing tags (e.g., "else", "elif" for an "if" tag).

The tag name itself (e.g., `if`, `for`, `my_custom_tag`) is derived from the last segment of the TOML table path defining the spec.

## Configuration

- **Built-in TagSpecs**: The parser includes TagSpecs for Django's built-in tags and popular third-party tags. These are provided by `djls-templates` automatically; users do not need to define them. The examples below show the format, but you only need to create files for your *own* custom tags or to override built-in behavior.
- **User-defined TagSpecs**: Users can expand or override TagSpecs via `pyproject.toml` or `djls.toml` files in their project, allowing custom tags and configurations to be seamlessly integrated.

## Examples

### If Tag

```toml
[tagspecs.django.template.defaulttags.if]
end = { tag = "endif" }
intermediates = ["elif", "else"]
```

### For Tag

```toml
[tagspecs.django.template.defaulttags.for]
end = { tag = "endfor" }
intermediates = ["empty"]
```

### Autoescape Tag

```toml
[tagspecs.django.template.defaulttags.autoescape]
end = { tag = "endautoescape" }
```

### Custom Tag

```toml
[tagspecs.my_module.templatetags.my_tags.my_custom_tag]
end = { tag = "endmycustomtag", optional = true }
intermediates = ["myintermediate"]
```
