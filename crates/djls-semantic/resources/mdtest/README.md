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
| `py` | A Python module in the fixture project. A relative path label is required. |
| `toml` | The project settings for that heading and its descendants. |
| `snapshot` | The expected rendered diagnostics. |
| `ignore` | Content that the runner skips. |

The runner rejects unknown fence languages. A section may contain at most one `toml` fence.

A `toml` fence replaces the inherited settings as one value. Omitted keys take these defaults:

| Key | Type | Default |
|---|---|---|
| `dirs` | list of strings | `["/templates"]` |
| `app-dirs` | boolean | `false` |
| `builtins` | list of module paths | `[]` |
| `libraries` | table from load name to module path | `{}` |
| `partial` | boolean | `false` |

## Inheritance

A `toml` fence applies to its heading and every nested heading until a child supplies another `toml` fence. The child settings replace the inherited value rather than merging fields.

Files stay in the section that declares them. Each scenario repeats the `py` files and templates it needs. A grouping heading may contain `toml`, but a `py` fence there is an error because child scenarios do not inherit it. Once a heading contains a template, it cannot have child headings.

A `libraries` or `builtins` entry naming a module that no `py` fence in that scenario provides makes the library unreadable and suppresses unknown-name diagnostics, so keep the `toml` fence beside the fences that supply its modules.

This example shares the settings from the title while keeping the Python module in the scenario that uses it:

````markdown
# Greeting tags

```toml
builtins = ["greeting_tags"]
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
