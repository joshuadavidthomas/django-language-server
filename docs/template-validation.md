# Template Validation

Django Language Server provides static analysis of Django templates, catching errors before you run your application. This page explains how validation works, what it can and cannot detect, and what to expect in different scenarios.

## How Validation Works

djls uses a two-layer approach to understand your Django templates:

### Runtime Inventory (Inspector)

When djls starts, it queries your Django project to discover:

- **Which template tags and filters exist** — from Django builtins, installed apps, and third-party libraries
- **Which libraries they belong to** — for `{% load %}` scoping validation
- **Where they're registered** — for documentation and jump-to-definition

This inventory is **authoritative** — Django itself reports what's available in your project. djls trusts this information completely.

### Validation Rules (Extraction)

For tags and filters in the inventory, djls extracts validation rules by analyzing the Python source code:

- **Argument requirements** — required arguments, valid options, syntax patterns
- **Block structure** — end tags, intermediate tags (like `{% else %}`)
- **Filter arity** — whether a filter requires or accepts an argument
- **Expression syntax** — valid operators and operands in `{% if %}` expressions

This extraction is **best-effort** — djls can only extract rules from patterns it recognizes in the source code. Complex or dynamic validation logic may not be captured.

## What djls Validates

| Validation | Example | Diagnostic |
|------------|---------|------------|
| Unknown tags | `{% nonexistent %}` | S108 |
| Unloaded library tags | `{% trans %}` without `{% load i18n %}` | S109 |
| Unknown filters | `{{ x\|nonexistent }}` | S111 |
| Unloaded library filters | `{{ x\|localize }}` without `{% load l10n %}` | S112 |
| Unclosed blocks | `{% if x %}` without `{% endif %}` | S100 |
| Mismatched blocks | `{% if x %}{% endfor %}` | S101 |
| Missing arguments | `{% cycle %}` (requires values) | S104 |
| Invalid arguments | `{% for x in %}` (missing iterable) | S105 |
| Expression syntax | `{% if and x %}` | S114 |
| Filter arity | `{{ x\|truncatewords }}` (requires argument) | S115, S116 |

### Filter Arity Validation

Filter arity diagnostics (S115, S116) depend on djls successfully extracting argument requirements from the filter's Python source. If extraction fails or the filter's signature is ambiguous (e.g., uses `*args`), these diagnostics are skipped rather than guessing incorrectly. This is expected behavior—if you don't see S115/S116 for a filter you expect to be validated, extraction may not support that filter's signature pattern.

## What djls Cannot Validate

djls performs **static analysis only** — it never executes your templates or Python code. This means:

### Runtime-Only Validation

- **Variable existence** — `{{ user.email }}` doesn't check if `user` exists in context
- **Type compatibility** — `{{ value|date:"Y" }}` doesn't verify `value` is a date
- **Template inheritance** — `{% extends %}` and `{% include %}` targets aren't resolved
- **Conditional logic** — Errors inside `{% if False %}` blocks are still reported

### Dynamic Tag Behavior

Some template tags perform validation at render time that djls cannot replicate:

- **Database queries** — Tags that validate against model fields
- **Request context** — Tags that check request attributes
- **Custom validation** — Tags with complex Python validation logic

If a tag's validation depends on runtime state, djls may:

- Report false positives (errors that Django wouldn't raise)
- Miss errors (issues Django would catch at render time)

## Inspector Availability

djls validation depends on the inspector being able to query your Django project.

### When Inspector is Healthy

- Full tag/filter inventory available
- Unknown tags/filters produce errors (S108, S111)
- Unloaded library tags/filters produce errors (S109, S112)
- Ambiguous symbols produce warnings (S110, S113)

### When Inspector is Unavailable

The inspector may be unavailable when:

- Django project won't initialize (settings error, missing dependency)
- Python environment not configured correctly
- `DJANGO_SETTINGS_MODULE` not set

In this state, djls **suppresses load-scoping diagnostics** (S108-S113) to avoid false positives. You'll see reduced validation coverage but no spurious errors.

To diagnose inspector issues, enable debug logging in your configuration:

```toml
# djls.toml or pyproject.toml [tool.djls]
debug = true
```

Then check your editor's LSP log output for messages about inspector initialization and Django setup.

## Ambiguous Symbols

When multiple libraries define the same tag or filter name, and you haven't loaded any of them, djls cannot determine which library you intended.

**Example**: If both `myapp` and `otherapp` define a `{% widget %}` tag:

- `{% widget %}` → S110: "Tag 'widget' requires one of: `{% load myapp %}`, `{% load otherapp %}`"
- After `{% load myapp %}` → No error (Django will use myapp's version)

## Reporting Mismatches

If djls reports an error that Django doesn't raise (or vice versa), please [report it](https://github.com/joshuadavidthomas/django-language-server/issues/new?template=template-validation-mismatch.yml) so we can improve validation accuracy.

Include:

1. The template snippet
2. Expected Django behavior
3. Actual djls diagnostic
4. Django and djls versions
5. Whether the inspector was healthy (check server logs with `debug = true`)

## See Also

- [Diagnostic Codes](configuration/index.md#available-diagnostic-codes) — Full list of validation errors
- [TagSpecs](configuration/tagspecs.md) — Define custom tag specifications
