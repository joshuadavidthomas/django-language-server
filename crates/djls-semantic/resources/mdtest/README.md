# Markdown diagnostic snapshots

These Markdown files pair Django templates with their rendered DJLS diagnostics.

Run them with:

```bash
cargo test -p djls-semantic --test mdtest
```

Update generated snapshots with:

```bash
DJLS_UPDATE_MDTEST_SNAPSHOTS=1 cargo test -p djls-semantic --test mdtest
```

## Fences

Each fenced block belongs to the heading above it:

| Fence | Meaning |
|---|---|
| `htmldjango`, `django`, or `html` | A template. The unlabeled block is the file under test; labeled blocks are support templates. |
| `py` | A Python module in the fixture project. A relative path label is required. A block labeled `settings.py` supplies the project settings and inherits through nested headings. |
| `snapshot` | The expected rendered diagnostics. |
| `ignore` | Content that the runner skips. |

The runner rejects unknown fence languages. A section may contain at most one `settings.py` block.

A scenario with no `settings.py` in its heading ancestry uses this file:

`settings.py`:

```py
INSTALLED_APPS = []
TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates'], 'APP_DIRS': False, 'OPTIONS': {'builtins': [], 'libraries': {}}}]
```

Copy that block into a scenario or grouping heading, then edit the Django settings the scenario needs.

## Inheritance

A `settings.py` block applies to its heading and every nested heading until a child supplies another one. The child replaces the inherited file.

Other files stay in the section that declares them. Each scenario repeats the Python modules and templates it needs. A grouping heading may contain `settings.py`, but any other Python file there is an error because child scenarios do not inherit it. Once a heading contains a template, it cannot have child headings.

A `libraries` or `builtins` entry naming a module that the scenario does not provide makes the library unreadable and suppresses unknown-name diagnostics. Keep each settings block beside the scenarios whose Python fences provide its modules.

This example shares settings from the title while keeping the Python module in the scenario that uses it:

````markdown
# Greeting tags

`settings.py`:

```py
INSTALLED_APPS = []
TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates'], 'APP_DIRS': False, 'OPTIONS': {'builtins': ['greeting_tags'], 'libraries': {}}}]
```

## accepts one name

`greeting_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag
def greet(name): return f"Hello, {name}"
```

```htmldjango
{% greet "Ada" %}
```

```snapshot
✓ no diagnostics
```
````

## Template files

Use one unlabeled template block as the file under test. It gets the path `test.html`:

````markdown
```htmldjango
{% else %}
```
````

A single-template scenario may label its file to override that path:

````markdown
`templates/example.html`:

```htmldjango
{% else %}
```
````

Scenarios with several templates need one unlabeled block. Labels give the support templates their paths:

````markdown
```htmldjango
{% extends "base.html" %}
```

`base.html`:

```htmldjango
{% block content %}{% endblock %}
```
````

Put the `snapshot` fence after the template blocks. A scenario with no diagnostics uses:

```snapshot
✓ no diagnostics
```
