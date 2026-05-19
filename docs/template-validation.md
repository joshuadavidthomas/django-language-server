# Template Validation

Django Language Server validates your Django templates as you write them, catching errors before you run your application. This page explains how validation works, what it can detect, and its limitations.

## How It Works

Template validation relies on three systems working together:

### Project Facts

**Project facts** describe the Django facts djls needs for templates:

- Which template tags and filters exist
- Which libraries they belong to (builtins vs third-party)
- Which library load-name maps to which module
- Which template directories Django searches

By default, `django_discovery = "source"` builds these facts from your workspace, `django_settings_module`, `pythonpath`, and available source files. When source-derived facts are available, djls can provide scoped completions and diagnostics without spawning Python or calling `django.setup()`.

You can choose the discovery path with [`django_discovery`](configuration/index.md#django_discovery):

- `"source"` — source-based Django discovery, no inspector subprocess
- `"runtime"` — legacy runtime inspector

Project facts track confidence. Known facts enable scoped availability diagnostics. Partial or unknown facts still allow diagnostics backed by positive facts, but suppress absence-based diagnostics.

### Inspector

The **inspector** is the legacy Python process that introspects your running Django project. It remains available through `django_discovery = "runtime"` during the source-based Django discovery rollout.

### Environment Scanner

The **environment scanner** examines your Python environment (site-packages and `sys.path`) to discover all template tag libraries that are *installed* — regardless of whether their Django app is in `INSTALLED_APPS`. It does this by:

1. Finding all `templatetags/*.py` files across your Python path
2. Parsing each file with static AST analysis to identify tag and filter registrations

This allows djls to distinguish between tags that are truly unknown (package not installed) and tags that exist in your environment but aren't activated in your Django configuration.

### Extraction

The **extraction engine** analyzes Python source code (using static AST analysis) to derive validation rules from template tag and filter implementations:

- **Argument constraints** — how many arguments a tag expects, required keywords
- **Block structure** — which tags need closing tags, what intermediate tags are allowed
- **Filter arity** — whether a filter expects an argument (e.g., `{{ value|default:"nothing" }}`)
- **Expression syntax** — valid operator usage in `{% if %}` / `{% elif %}` expressions

Together, project facts tell djls *what's active in your project*, the environment scanner tells djls *what's installed*, and extraction tells djls *how to validate usage*.

## Three-Layer Resolution

A template tag or filter must pass through three layers before it's available in a template:

```
Python Environment  →  Django Configuration  →  Template Load  →  Available
(pip install)          (INSTALLED_APPS)          ({% load %})
```

Each layer has a different failure mode and a different fix:

| Layer | Failure | Diagnostic | Fix |
|---|---|---|---|
| Not in environment | Package not installed | S108/S111 (Unknown) | `pip install <package>` |
| In environment, not in `INSTALLED_APPS` | App not activated | S118/S119 (Not in INSTALLED_APPS) | Add app to `INSTALLED_APPS` |
| In `INSTALLED_APPS`, not loaded | No `{% load %}` | S109/S112 (Unloaded) | Add `{% load <library> %}` |

The same three-layer model applies to `{% load %}` library names themselves:

| Layer | Failure | Diagnostic | Fix |
|---|---|---|---|
| Library not in environment | Package not installed | S120 (Unknown library) | `pip install <package>` |
| Library in environment, not in `INSTALLED_APPS` | App not activated | S121 (Library not in INSTALLED_APPS) | Add app to `INSTALLED_APPS` |
| Library in `INSTALLED_APPS` | Valid load | No diagnostic | — |

This layered approach gives you actionable diagnostics — instead of a generic "unknown tag" error, djls tells you exactly what to do to fix it.

## What djls Validates

### Block Structure (S100–S103)

Validates that block tags are properly opened, closed, and nested:

- **S100** — Unclosed tag (missing end tag, e.g., `{% block %}` without `{% endblock %}`)
- **S101** — Unbalanced structure (mismatched block tags)
- **S102** — Orphaned tag (intermediate tag like `{% else %}` without a parent `{% if %}`)
- **S103** — Unmatched block name (e.g., `{% endblock foo %}` doesn't match `{% block bar %}`)

### Tag Scoping (S108–S110, S118)

Validates that template tags are available at their point of use, using [three-layer resolution](#three-layer-resolution):

- **S108** — Unknown tag (not found in any known library or in the Python environment)
- **S109** — Unloaded tag (defined in a library that hasn't been loaded via `{% load %}`)
- **S110** — Ambiguous unloaded tag (defined in multiple libraries — load the correct one to resolve)
- **S118** — Tag not in `INSTALLED_APPS` (the tag exists in an installed package, but its Django app isn't activated)

### Filter Scoping (S111–S113, S119)

Validates that template filters are available at their point of use, with the same three-layer resolution as tags:

- **S111** — Unknown filter (not found in any known library or in the Python environment)
- **S112** — Unloaded filter (defined in a library that hasn't been loaded)
- **S113** — Ambiguous unloaded filter (defined in multiple libraries)
- **S119** — Filter not in `INSTALLED_APPS` (the filter exists in an installed package, but its Django app isn't activated)

### Library Validation (S120–S121)

Validates that `{% load %}` library names refer to known template tag libraries:

- **S120** — Unknown library (not found in the project facts or the Python environment)
- **S121** — Library not in `INSTALLED_APPS` (the library exists in an installed package, but its Django app isn't activated)

### Extends Validation (S122–S123)

Validates structural rules for `{% extends %}`:

- **S122** — `{% extends %}` must be the first tag in the template. Text and `{# comments #}` are allowed before it, but no other tags (`{% load %}`, etc.) or variables (`{{ foo }}`) may appear first. This matches Django's parse-time enforcement.
- **S123** — `{% extends %}` cannot appear more than once in a template.

### Expression Syntax (S114)

Validates operator usage in `{% if %}` and `{% elif %}` expressions:

- **S114** — Expression syntax error (e.g., `{% if and x %}`, `{% if x == %}`, `{% if x y %}`)

### Filter Arity (S115–S116)

Validates that filters are called with the correct number of arguments:

- **S115** — Filter requires an argument but none was provided (e.g., `{{ value|default }}` instead of `{{ value|default:"fallback" }}`)
- **S116** — Filter does not accept an argument but one was provided (e.g., `{{ value|title:"arg" }}`)

### Tag Argument Validation (S117)

Validates that template tags are called with the correct arguments, based on rules extracted from Python source code:

- **S117** — Tag argument rule violation (e.g., `{% for item %}` missing `in` keyword, `{% cycle %}` with no arguments)

These rules are derived automatically by analyzing Django's template tag implementations via static AST analysis. The extraction engine reads `split_contents()` guard conditions, function signatures, and keyword position checks directly from Python source code — no manual configuration needed.

## What djls Cannot Validate

Django templates are deeply dynamic — many things can only be checked at runtime:

- **Variable resolution** — Whether a variable exists in the template context
- **Type coercion** — Whether filter arguments have the correct type at runtime
- **Template inheritance** — `{% extends %}` and `{% include %}` resolution across files
- **Dynamic tag behavior** — Tags whose validation depends on runtime state
- **Format strings** — Whether date/time format strings are valid

## Project Facts Availability

The source-based Django discovery needs enough source files and configuration to find your Django settings, installed apps, template directories, and templatetag modules. For typical literal or split settings, it can work without a configured Python runtime.

When project facts are **known**:

- Scoped tag, filter, and library availability diagnostics can run from project facts
- Completions are scoped to loaded libraries at cursor position
- Argument and arity diagnostics run when extraction or built-in specs provide rules

When project facts are **partial or unknown**:

- **Absence-based diagnostics are suppressed when unsafe** — Without complete project facts, djls avoids claiming that a tag, filter, or library is unknown or missing from `INSTALLED_APPS` (S108, S111, S118, S119, S120, S121)
- **Positive-fact diagnostics can still run** — Known but unloaded or ambiguous tags and filters can still produce S109, S110, S112, and S113
- **Known argument rules can still run** — Tag argument diagnostics (S117) run when extraction or built-in specs provide the needed rules
- **Known arity rules can still run with positive project facts** — Filter arity diagnostics (S115–S116) run when active project facts include known filters and extraction or built-in specs provide the needed rules
- **S100–S103 still work** — Block structure validation uses built-in tag specs
- **S114 still works** — Expression syntax validation is purely structural
- **Completions show all known tags** — Without library scoping, all tags are offered as a fallback

This design avoids false positives when the Python environment isn't available.

In `django_discovery = "source"`, djls does not fall back to the inspector for partial or unknown source-derived facts. Use `django_discovery = "runtime"` when you need the legacy runtime path for a dynamic project.

## Configuring Diagnostic Severity

All diagnostics default to error severity. You can adjust or disable them in your configuration:

```toml
[diagnostics.severity]
# Disable all scoping diagnostics (tags, filters, libraries)
"S108" = "off"
"S109" = "off"
"S110" = "off"
"S118" = "off"

# Downgrade filter arity checks to warnings
"S115" = "warning"
"S116" = "warning"

# Or use prefix matching to control groups
"S11" = "warning"   # All S110-S119 as warnings
"S12" = "warning"   # All S120-S121 as warnings
```

See the [Configuration](./configuration/index.md#diagnostics) page for full details on severity configuration.

## Reporting Validation Mismatches

If djls reports an error for a template that works correctly in Django (or misses an error that Django would catch), please [open an issue](https://github.com/joshuadavidthomas/django-language-server/issues/new) with:

- Your djls and Django versions
- A minimal template snippet reproducing the issue
- What Django does vs what djls reports
- Your `djls.toml` configuration (if any)

As a workaround, you can [disable specific diagnostics](./configuration/index.md#diagnostics) via severity configuration (e.g., `S117 = "off"` to suppress tag argument validation errors).
