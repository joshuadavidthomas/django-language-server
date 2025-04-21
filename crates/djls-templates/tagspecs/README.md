# TagSpecs

## Schema

Tag Specifications (TagSpecs) define how tags are parsed and understood. They allow the parser to handle custom tags without hard-coding them.

```toml
[package.module.path.tag_name]  # Path where tag is registered, e.g., django.template.defaulttags
type = "container" | "inclusion" | "single"
closing = "closing_tag_name"        # For block tags that require a closing tag
branches = ["branch_tag_name", ...] # For block tags that support branches

# Arguments can be positional (matched by order) or keyword (matched by name)
args = [
    # Positional argument (position inferred from array index)
    { name = "setting", required = true, allowed_values = ["on", "off"] },
    # Keyword argument
    { name = "key", required = false, is_kwarg = true }
]
```

The `name` field in args should match the internal name used in Django's node implementation. For example, the `autoescape` tag's argument is stored as `setting` in Django's `AutoEscapeControlNode`.

## Tag Types

- `container`: Tags that wrap content and require a closing tag

  ```django
  {% if condition %}content{% endif %}
  {% for item in items %}content{% endfor %}
  ```

- `inclusion`: Tags that include or extend templates.

  ```django
  {% extends "base.html" %}
  {% include "partial.html" %}
  ```

- `single`: Single tags that don't wrap content

  ```django
  {% csrf_token %}
  ```

## Configuration

- **Built-in TagSpecs**: The parser includes TagSpecs for Django's built-in tags and popular third-party tags.
- **User-defined TagSpecs**: Users can expand or override TagSpecs via `pyproject.toml` or `djls.toml` files in their project, allowing custom tags and configurations to be seamlessly integrated.

## Examples

### If Tag

```toml
[django.template.defaulttags.if]
type = "container"
closing = "endif"
branches = ["elif", "else"]
args = [{ name = "condition", required = true }]
```

### Include Tag

```toml
[django.template.defaulttags.includes]
type = "inclusion"
args = [{ name = "template_name", required = true }]
```

### Autoescape Tag

```toml
[django.template.defaulttags.autoescape]
type = "container"
closing = "endautoescape"
args = [{ name = "setting", required = true, allowed_values = ["on", "off"] }]
```

### Custom Tag with Kwargs

```toml
[my_module.templatetags.my_tags.my_custom_tag]
type = "single"
args = [
    { name = "arg1", required = true },
    { name = "kwarg1", required = false, is_kwarg = true }
]
```
