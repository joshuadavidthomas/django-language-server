# Template Validation

Django Language Server validates your Django templates as you write them, catching errors before you run your application. This page explains how validation works, what it can detect, and its limitations.

## How It Works

Template validation relies on three systems working together:

### Static project discovery

DJLS statically reads your Django settings and Python search paths to discover:

- Which apps are listed in `INSTALLED_APPS`
- Which template tag libraries are active for this project
- Which template tag libraries exist on the search paths but are not active
- Which tags and filters each library registers

This gives djls an evidence-backed picture of your project's template tag ecosystem without starting Django. The active inventory reflects `INSTALLED_APPS`, so tags and filters from packages outside the active app list are reported separately.

### Extraction

The **extraction engine** analyzes Python source code (using static AST analysis) to derive validation rules from template tag and filter implementations:

- **Argument constraints** — how many arguments a tag expects, required keywords
- **Block structure** — which tags need closing tags, what intermediate tags are allowed
- **Filter arity** — whether a filter expects an argument (e.g., `{{ value|default:"nothing" }}`)
- **Expression syntax** — valid operator usage in `{% if %}` / `{% elif %}` expressions

Together, static project discovery tells djls *what's active in your project*, and extraction tells djls *how to validate usage*.

## Template Symbol Resolution

A template tag or filter must be known to the active Django project and loaded before it's available in a template:

```
Django Configuration  →  Template Load  →  Available
(INSTALLED_APPS)          ({% load %})
```

Each layer has a different failure mode and a different fix:

| Layer | Failure | Diagnostic | Fix |
|---|---|---|---|
| Not found in active or inactive libraries | Unknown package, library, tag, or filter | S108/S111/S120 (Unknown) | Check the name or install the package |
| Found on the Python search paths, not active in this project | App missing from `INSTALLED_APPS` | S118/S119/S121 (Not in `INSTALLED_APPS`) | Add the app to `INSTALLED_APPS` |
| Active, not loaded | No `{% load %}` | S109/S112 (Unloaded) | Use the quick fix or add `{% load <library> %}` |

`{% load %}` library names are checked against the active static inventory first. If the library exists on the project's Python search paths but its app is not in `INSTALLED_APPS`, djls reports S121. If no inactive-library evidence exists, djls reports S120.

The same inventory powers editor navigation. Resolved `{% load %}` library names become document links and go-to-definition targets for their Python source files. Selective-load symbols and available Tag and Filter names jump to definite local Python declarations. Dynamic, imported, member, and ambiguous callables are skipped rather than guessed.

Template directory discovery also powers go to definition for literal `{% extends %}` and `{% include %}` names. An overridden `{% block %}` name resolves to the nearest definite parent block; a root block resolves to itself. Find references returns the root block and its definite overrides. Editors that support definition links receive exact origin and declaration ranges.

This gives you diagnostics based on the same template tag inventory Django would use at runtime, while distinguishing "not installed or misspelled" from "installed but not activated".

## Code Actions

DJLS exposes quick fixes for selected template validation diagnostics. In editors that support LSP code actions, open the quick-fix menu on the diagnostic range to apply them.

| Diagnostic | Quick fix |
|---|---|
| S109/S112 — unloaded tag or filter with one matching library | Add a standalone `{% load <library> %}` line |
| S110/S113 — unloaded tag or filter found in multiple libraries | Choose one `{% load <library> %}` quick fix per candidate library |
| S103 — mismatched `{% endblock %}` name | Rename only the closing block name to match the opening `{% block %}` |

Load quick fixes insert a new `{% load ... %}` line after the leading template import run: after `{% extends %}` and existing top-of-file `{% load %}` tags when present, or at the beginning of the template otherwise. They do not rewrite existing `{% load %}` tags.

Quick fixes are derived from active diagnostics. If you disable a diagnostic with `diagnostics.severity`, its quick fix is disabled too.

## What djls Validates

### Block Structure (S100–S103)

Validates that block tags are properly opened, closed, and nested:

- **S100** — Unclosed tag (missing end tag, e.g., `{% block %}` without `{% endblock %}`)
- **S101** — Unbalanced structure (mismatched block tags)
- **S102** — Orphaned tag (intermediate tag like `{% else %}` without a parent `{% if %}`)
- **S103** — Unmatched block name (e.g., `{% endblock foo %}` doesn't match `{% block bar %}`)

### Tag Scoping (S108–S110, S118)

Validates that template tags are available at their point of use, using [template symbol resolution](#template-symbol-resolution):

- **S108** — Unknown tag (not found in any active or inactive library)
- **S109** — Unloaded tag (defined in an active library that hasn't been loaded via `{% load %}`)
- **S110** — Ambiguous unloaded tag (defined in multiple active libraries — load the correct one to resolve)
- **S118** — Tag exists in a library whose app is not in `INSTALLED_APPS`

### Filter Scoping (S111–S113, S119)

Validates that template filters are available at their point of use, with the same resolution layers as tags:

- **S111** — Unknown filter (not found in any active or inactive library)
- **S112** — Unloaded filter (defined in an active library that hasn't been loaded)
- **S113** — Ambiguous unloaded filter (defined in multiple active libraries)
- **S119** — Filter exists in a library whose app is not in `INSTALLED_APPS`

### Library Validation (S120–S121)

Validates that `{% load %}` library names refer to known template tag libraries:

- **S120** — Unknown library (not found in active or inactive libraries)
- **S121** — Library exists on the project's Python search paths, but its app is not in `INSTALLED_APPS`

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
- **Template existence diagnostics and inheritance semantics** — djls resolves static `{% extends %}` / `{% include %}` targets for navigation, but it does not report missing targets as validation errors or verify inherited block behavior
- **Dynamic tag behavior** — Tags whose validation depends on runtime state
- **Format strings** — Whether date/time format strings are valid

## Template Inventory Completeness

The template inventory can be complete, incomplete, or unavailable. It is complete only when djls can resolve the active settings, installed apps, template backends, and relevant Python modules.

When the template inventory is **complete**:

- Full validation is active — all current validation diagnostics are emitted
- Completions are scoped to loaded libraries at cursor position

When the template inventory is **incomplete or unavailable**:

- **Absence-claim diagnostics are suppressed** — S108, S111, S118, S119, S120, and S121 require a complete active set, so djls does not emit them from incomplete evidence
- **Known unloaded diagnostics can still appear with an incomplete inventory** — S109, S110, S112, and S113 are based on positive active-library evidence
- **S115–S116 are suppressed when filter arity rules are unavailable** — Filter arity rules come from extraction, which depends on knowing source modules
- **S117 is suppressed when tag argument rules are unavailable** — Tag argument rules come from extraction, which depends on discovering tag source modules
- **S100–S103 still work** — Block structure validation uses built-in tag specs
- **S114 still works** — Expression syntax validation is purely structural
- **Project-backed completions and `{% load %}` document links are unavailable** when the active inventory is unknown; syntax-only completions may still appear

This design avoids false positives when djls cannot prove whether a tag, filter, or library is absent from the active project.

## Configuring Diagnostic Severity

All diagnostics default to error severity. You can adjust or disable them in your configuration:

```toml
[diagnostics.severity]
# Disable all scoping diagnostics (tags, filters, libraries)
"S108" = "off"
"S109" = "off"
"S110" = "off"

# Downgrade filter arity checks to warnings
"S115" = "warning"
"S116" = "warning"

# Or use prefix matching to control groups
"S11" = "warning"   # All S110-S117 as warnings
"S12" = "warning"   # S120 and future S12x diagnostics as warnings
```

See the [Configuration](./configuration/index.md#diagnostics) page for full details on severity configuration.

## Reporting Validation Mismatches

If djls reports an error for a template that works correctly in Django (or misses an error that Django would catch), please [open an issue](https://github.com/joshuadavidthomas/django-language-server/issues/new) with:

- Your djls and Django versions
- A minimal template snippet reproducing the issue
- What Django does vs what djls reports
- Your `djls.toml` configuration (if any)

As a workaround, you can [disable specific diagnostics](./configuration/index.md#diagnostics) via severity configuration (e.g., `S117 = "off"` to suppress tag argument validation errors).
