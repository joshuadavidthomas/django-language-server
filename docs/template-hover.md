# Template Hover

Django Language Server shows hover documentation for Django template symbols.

## Supported hover targets

Hover works for:

- template tag names, such as `{% block %}`, `{% include %}`, and `{% load %}`
- template filter names, such as `{{ value|default:"fallback" }}`
- template library names in `{% load %}` tags
- selectively loaded symbols in `{% load trans from i18n %}`
- template paths in `{% extends %}` and `{% include %}`

## Hover content

Tag and filter hovers identify the symbol kind, then show Django's documentation when it is available:

````markdown
```text
(tag) blocktranslate
```
---
Translate a block of text with parameters.
````

Examples from Django docstrings are shown as Django template snippets.

Symbols that require a `{% load %}` tag include the required library:

````markdown
```text
(filter) intcomma
```
---
Converts an integer to a string containing commas every three digits.
---
Requires `{% load humanize %}`.
````

Built-in tags and filters do not show a load requirement.

Template path hovers show whether the referenced template resolves. If it cannot be resolved, djls shows the paths it tried when template directories are known.

## Limitations

Hover does not yet resolve template variables to Python or ORM types. Variable hover will be added after template context inference exists.

Hover links to the exact Python function that defines a tag or filter are not yet available. That requires improving template tag/filter definition targets first.
