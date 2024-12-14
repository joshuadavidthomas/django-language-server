# djls-ast Tag Specifications

Configuration files defining template tag behavior for the Django Language Server Protocol.

## Schema

```toml
[package.module.path]  # Path where tag is registered, e.g., django.template.defaulttags
tag_name = {
    type = "block" | "tag" | "assignment" | "variable",  # Required
    closing = "endtag",                                  # Optional: closing tag name
    intermediates = ["else", "elif"],                    # Optional: intermediate tags
    introduces_vars = ["varname", "{placeholder}"],      # Optional: variables introduced
    valid_args = ["required", "?optional", "*"],         # Optional: argument patterns
}
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

## Field Details

- `closing`: Optional closing tag name
- `intermediates`: Optional list of allowed intermediate tags
- `introduces_vars`: Optional list of variables made available
- `valid_args`: Optional list of argument patterns
  - `*` matches any argument
  - `?` prefix marks optional argument
  - Empty list means no arguments allowed

## Examples

```toml
# django.template.defaulttags.toml
[django.template.defaulttags]
if = {
    type = "block",
    closing = "endif",
    intermediates = ["else", "elif"],
    valid_args = ["*"]
}

csrf_token = {
    type = "tag",
    valid_args = []
}

for = {
    type = "block",
    closing = "endfor",
    intermediates = ["empty"],
    introduces_vars = ["forloop", "{loop_var}"],
    valid_args = ["* in *"]
}

# django-crispy-forms.toml
[crispy_forms.templatetags.crispy_forms_filters]
formhelper = {
    type = "block",
    closing = "endformhelper",
    valid_args = ["form_variable"]
}

field = {
    type = "block",
    closing = "endfield",
    valid_args = ["field_name", "?label_text"]
}
```

## User Configuration

Users can add or override tag specifications in their `pyproject.toml`:

```toml
[tool.djls.templatetags]
"myapp.templatetags.custom_tags" = {
    customtag = {
        type = "block",
        closing = "endcustomtag",
        intermediates = ["middle"],
        valid_args = ["arg1", "?arg2"]
    }
}
```
