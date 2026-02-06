# Template Validation

Django Language Server validates your Django templates as you write them, catching errors before you run your application. This page explains how validation works, what it can detect, and its limitations.

## How It Works

Template validation relies on two systems working together:

### Inspector

The **inspector** is a Python process that introspects your running Django project to discover:

- Which template tags and filters exist
- Which libraries they belong to (builtins vs third-party)
- Which library load-name maps to which module

This gives djls an accurate picture of your project's template tag ecosystem — the same tags Django would see at runtime.

### Extraction

The **extraction engine** analyzes Python source code (using static AST analysis) to derive validation rules from template tag and filter implementations:

- **Argument constraints** — how many arguments a tag expects, required keywords
- **Block structure** — which tags need closing tags, what intermediate tags are allowed
- **Filter arity** — whether a filter expects an argument (e.g., `{{ value|default:"nothing" }}`)
- **Expression syntax** — valid operator usage in `{% if %}` / `{% elif %}` expressions

Together, the inspector tells djls *what exists* and extraction tells djls *how to validate usage*.

## What djls Validates

### Block Structure (S100–S103)

Validates that block tags are properly opened, closed, and nested:

- **S100** — Unclosed tag (missing end tag, e.g., `{% block %}` without `{% endblock %}`)
- **S101** — Unbalanced structure (mismatched block tags)
- **S102** — Orphaned tag (intermediate tag like `{% else %}` without a parent `{% if %}`)
- **S103** — Unmatched block name (e.g., `{% endblock foo %}` doesn't match `{% block bar %}`)

### Tag Scoping (S108–S110)

Validates that template tags are available at their point of use, respecting `{% load %}` statements:

- **S108** — Unknown tag (not defined in any known library)
- **S109** — Unloaded tag (defined in a library that hasn't been loaded via `{% load %}`)
- **S110** — Ambiguous unloaded tag (defined in multiple libraries — load the correct one to resolve)

### Filter Scoping (S111–S113)

Validates that template filters are available at their point of use, with the same load-scoping rules as tags:

- **S111** — Unknown filter (not defined in any known library)
- **S112** — Unloaded filter (defined in a library that hasn't been loaded)
- **S113** — Ambiguous unloaded filter (defined in multiple libraries)

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

## Inspector Availability

The inspector requires a working Django environment (correct `DJANGO_SETTINGS_MODULE`, virtual environment with Django installed, no import errors during Django setup).

When the inspector is **healthy**:

- Full validation is active — all S108–S116 diagnostics are emitted
- Completions are scoped to loaded libraries at cursor position

When the inspector is **unavailable** (Django init failed, Python not configured, etc.):

- **S108–S113 are suppressed** — Without knowing which tags/filters exist, djls cannot determine if something is unknown or just unloaded
- **S115–S116 are suppressed** — Filter arity rules come from extraction, which depends on knowing the source modules
- **S117 is suppressed** — Tag argument rules come from extraction, which depends on the inspector discovering tag source modules
- **S100–S103 still work** — Block structure validation uses built-in tag specs
- **S114 still works** — Expression syntax validation is purely structural
- **Completions show all known tags** — Without library scoping, all tags are offered as a fallback

This design avoids false positives when the Python environment isn't available.

## Configuring Diagnostic Severity

All diagnostics default to error severity. You can adjust or disable them in your configuration:

```toml
[diagnostics.severity]
# Disable all scoping diagnostics
"S108" = "off"
"S109" = "off"
"S110" = "off"

# Downgrade filter arity checks to warnings
"S115" = "warning"
"S116" = "warning"

# Or use prefix matching to control groups
"S11" = "warning"   # All S110-S119 as warnings
```

See the [Configuration](./configuration/index.md#diagnostics) page for full details on severity configuration.

## Reporting Validation Mismatches

If djls reports an error for a template that works correctly in Django (or misses an error that Django would catch), please [open an issue](https://github.com/joshuadavidthomas/django-language-server/issues/new?template=template-validation-mismatch.yml) with:

- Your djls and Django versions
- A minimal template snippet reproducing the issue
- What Django does vs what djls reports
- Your `djls.toml` configuration (if any)

As a workaround, you can [disable specific diagnostics](./configuration/index.md#diagnostics) via severity configuration (e.g., `S117 = "off"` to suppress tag argument validation errors).
