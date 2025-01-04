# djls-template-ast Tag Specifications

Configuration files defining template tag behavior for the Django Language Server Protocol.

## Schema

```toml
[package.module.path.tag_name]  # Path where tag is registered, e.g., django.template.defaulttags
type = "block" | "tag" | "assignment" | "variable"  # Required: Type of template tag
closing = "endtag"                                  # Optional: Name of closing tag for block tags
intermediates = ["else", "elif"]                    # Optional: Allowed intermediate tags

[[package.module.path.tag_name.args]]              # Optional: Arguments specification
name = "arg_name"                                  # Name of the argument
required = true | false                            # Whether the argument is required
```

## Tag Types

- `block`: Tags that wrap content and require a closing tag

  ```django
  {% if condition %}content{% endif %}
  {% for item in items %}content{% endfor %}
  ```

- `tag`: Single tags that don't wrap content

  ```django
  {% csrf_token %}
  {% include "template.html" %}
  ```

- `assignment`: Tags that assign their output to a variable

  ```django
  {% url 'view-name' as url_var %}
  {% with total=business.employees.count %}
  ```

- `variable`: Tags that output a value directly

  ```django
  {% cycle 'odd' 'even' %}
  {% firstof var1 var2 var3 %}
  ```

## Argument Specification

Arguments can be either:

- Literal values that must match exactly (e.g., "in")
- Placeholders for variables (wrapped in curly braces, e.g., "{item}")

## Examples

```toml
[django.template.defaulttags.if]
type = "block"
closing = "endif"
intermediates = ["else", "elif"]

[[django.template.defaulttags.if.args]]
name = "condition"
required = true

[django.template.defaulttags.for]
type = "block"
closing = "endfor"
intermediates = ["empty"]

[[django.template.defaulttags.for.args]]
name = "{item}"
required = true

[[django.template.defaulttags.for.args]]
name = "in"
required = true

[[django.template.defaulttags.for.args]]
name = "{iterable}"
required = true
```
