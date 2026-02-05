# M7: Documentation + Issue Template Implementation Plan

## Overview

After the template validation port (M1-M6) is complete, update documentation to reflect the new validation behavior and add a high-signal issue template for reporting mismatches between djls static validation and Django runtime behavior.

## Current State Analysis

### Documentation Structure

| File | Current Content |
|------|-----------------|
| `docs/configuration/index.md` | Diagnostic codes S100-S107, T100, T900-T901 |
| `docs/configuration/tagspecs.md` | Custom tagspec definitions, generic "open an issue" text |
| `.mkdocs.yml` | Navigation structure (no template validation section) |

### Missing After M1-M6

1. **New diagnostic codes**: S108-S116 not documented
2. **Validation architecture**: No explanation of inspector vs extraction
3. **Static-analysis limits**: No documentation of what djls can/cannot validate
4. **Issue reporting path**: No structured way to report validation mismatches

### GitHub Templates

- `.github/` directory exists (contains `workflows/`, `dependabot.yml`, `zizmor.yml`)
- No `.github/ISSUE_TEMPLATE/` directory exists

## Desired End State

After M7:

1. **New documentation page** (`docs/template-validation.md`) explaining:
   - What djls validates vs what Django validates at runtime
   - Inspector inventory vs Rust extraction (high-level)
   - "Inspector unavailable" behavior and its effects
   
2. **Updated diagnostic codes** in `docs/configuration/index.md`:
   - S100-S116 grouped by milestone bands
   - Each code with meaning, typical fix, and suppression rules
   - Link to new template validation page

3. **Navigation updated** in `.mkdocs.yml`:
   - "Template Validation" page under Configuration section

4. **Issue template** at `.github/ISSUE_TEMPLATE/template-validation-mismatch.yml`:
   - High-signal form for validation mismatches
   - Required debug information fields

## What We're NOT Doing

- General bug report form (future work)
- Feature request template (future work)
- Detailed internals documentation (charter/RFC level detail)
- Per-milestone incremental doc updates (all codes documented at once)
- Changing existing TagSpecs issue link to point at validation form

---

## Phase 1: Create Template Validation Documentation Page

### Overview

Add a new page explaining template validation architecture and limits for end users.

### Changes Required

#### 1. Create New Documentation Page

**File**: `docs/template-validation.md`

```markdown
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
- **Type compatibility** — `{{ value\|date:"Y" }}` doesn't verify `value` is a date
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
```

#### 2. Update Navigation

**File**: `.mkdocs.yml`

**Old text**:
```yaml
nav:
  - Home: index.md
  - Installation: installation.md
  - Configuration:
      - configuration/index.md
      - TagSpecs: configuration/tagspecs.md
  - Clients:
```

**New text**:
```yaml
nav:
  - Home: index.md
  - Installation: installation.md
  - Configuration:
      - configuration/index.md
      - TagSpecs: configuration/tagspecs.md
      - Template Validation: template-validation.md
  - Clients:
```

### Success Criteria

#### Automated Verification:
- [ ] Documentation builds without errors: `just docs build`
- [ ] No broken links in new page
- [ ] YAML syntax valid in `.mkdocs.yml`

#### Manual Verification:
- [ ] New page renders correctly with proper formatting
- [ ] Navigation shows "Template Validation" under Configuration
- [ ] Internal links work (to configuration, tagspecs)
- [ ] Content is accurate and matches M1-M6 implementation

---

## Phase 2: Update Diagnostic Codes Documentation

### Overview

Expand the diagnostic codes section in `docs/configuration/index.md` to include S108-S116, grouped by milestone bands with meaning, typical fix, and suppression rules. Add a link to the new template validation page.

### Changes Required

#### 1. Add Link to Template Validation Page

**File**: `docs/configuration/index.md`

**Location**: After the opening paragraph, before "## Options"

**Add**:
```markdown
!!! tip "Understanding Template Validation"

    For details on how djls validates templates, what it can and cannot detect, and how inspector availability affects diagnostics, see [Template Validation](../template-validation.md).
```

#### 2. Update Diagnostic Codes Section

**File**: `docs/configuration/index.md`

**Find and replace** the "Available diagnostic codes" section:

**Old text** (current section):

```markdown
#### Available diagnostic codes

**Template Errors (T-series):**
- `T100` - Parser errors (syntax issues in templates)
- `T900` - IO errors (file read/write issues)
- `T901` - Configuration errors (invalid tagspecs)

**Semantic Validation Errors (S-series):**
- `S100` - Unclosed tag (missing end tag)
- `S101` - Unbalanced structure (mismatched block tags)
- `S102` - Orphaned tag (intermediate tag without parent)
- `S103` - Unmatched block name (e.g., `{% endblock foo %}` doesn't match `{% block bar %}`)
- `S104` - Missing required arguments
- `S105` - Too many arguments
- `S106` - Invalid literal argument
- `S107` - Invalid argument choice
```

**New text**:

```markdown
#### Available diagnostic codes

**Template Errors (T-series):**

| Code | Error | Description |
|------|-------|-------------|
| `T100` | Parser error | Syntax issues in templates (unclosed tags, malformed expressions) |
| `T900` | IO error | File read/write issues |
| `T901` | Configuration error | Invalid tagspecs or configuration |

**Semantic Validation Errors (S-series):**

Semantic errors are grouped by validation category. Some errors depend on [inspector availability](../template-validation.md#inspector-availability) and may be suppressed when the inspector cannot query your Django project.

##### Block Structure (S100-S107)

These errors detect structural issues in template block tags.

| Code | Error | Description | Typical Fix |
|------|-------|-------------|-------------|
| `S100` | Unclosed tag | Block tag missing its end tag | Add `{% endif %}`, `{% endfor %}`, etc. |
| `S101` | Unbalanced structure | Mismatched block tags | Fix tag nesting order |
| `S102` | Orphaned tag | Intermediate tag without parent block | Move `{% else %}` inside `{% if %}` block |
| `S103` | Unmatched block name | End tag name doesn't match opening | Fix `{% endblock name %}` to match `{% block name %}` |
| `S104` | Missing required arguments | Tag requires arguments not provided | Add required arguments per tag documentation |
| `S105` | Too many arguments | Tag given more arguments than expected | Remove extra arguments |
| `S106` | Invalid literal argument | Argument value not recognized | Use valid literal value |
| `S107` | Invalid argument choice | Argument not in allowed choices | Use one of the allowed values |

##### Tag Scoping (S108-S110)

These errors validate `{% load %}` requirements for template tags. They depend on inspector availability.

| Code | Error | Description | Typical Fix | Suppression |
|------|-------|-------------|-------------|-------------|
| `S108` | Unknown tag | Tag not in Django's registry | Check spelling, install library, or define [TagSpec](tagspecs.md) | Suppressed when inspector unavailable |
| `S109` | Unloaded library tag | Tag requires `{% load %}` | Add `{% load library_name %}` before usage | Suppressed when inspector unavailable |
| `S110` | Ambiguous unloaded tag | Tag exists in multiple libraries | Load one of the listed libraries | Suppressed when inspector unavailable |

##### Filter Scoping (S111-S113)

These errors validate `{% load %}` requirements for template filters. They depend on inspector availability.

| Code | Error | Description | Typical Fix | Suppression |
|------|-------|-------------|-------------|-------------|
| `S111` | Unknown filter | Filter not in Django's registry | Check spelling, install library | Suppressed when inspector unavailable |
| `S112` | Unloaded library filter | Filter requires `{% load %}` | Add `{% load library_name %}` before usage | Suppressed when inspector unavailable |
| `S113` | Ambiguous unloaded filter | Filter exists in multiple libraries | Load one of the listed libraries | Suppressed when inspector unavailable |

##### Expression & Filter Arity (S114-S116)

These errors validate expression syntax and filter argument requirements.

| Code | Error | Description | Typical Fix | Suppression |
|------|-------|-------------|-------------|-------------|
| `S114` | Expression syntax error | Invalid `{% if %}` expression | Fix operator/operand syntax | Never suppressed |
| `S115` | Filter missing argument | Filter requires an argument | Add argument: `{{ x\|filter:arg }}` | Suppressed when inspector unavailable or arity unknown |
| `S116` | Filter unexpected argument | Filter doesn't accept arguments | Remove argument: `{{ x\|filter }}` | Suppressed when inspector unavailable or arity unknown |

!!! note "Filter Arity Extraction"

    S115 and S116 depend on djls extracting filter arity (argument requirements) from Python source. If extraction fails or the filter's signature is ambiguous, these diagnostics are skipped rather than guessing. This is expected behavior, not a bug.
```

### Success Criteria

#### Automated Verification:
- [ ] Documentation builds without errors
- [ ] Tables render correctly (no markdown syntax issues)
- [ ] Links to `template-validation.md` and `tagspecs.md` resolve

#### Manual Verification:
- [ ] All S100-S116 codes documented
- [ ] Grouping by bands is clear and logical
- [ ] Suppression rules accurately reflect M1-M6 implementation
- [ ] "Typical Fix" guidance is actionable
- [ ] Tip box with link to template validation page is visible

---

## Phase 3: Create GitHub Issue Template Directory

### Overview

Create the `.github/ISSUE_TEMPLATE/` directory structure required for issue forms.

### Changes Required

#### 1. Create Directory and Config

**File**: `.github/ISSUE_TEMPLATE/config.yml`

```yaml
blank_issues_enabled: true
contact_links:
  - name: Documentation
    url: https://djls.joshthomas.dev
    about: Check the documentation for configuration and troubleshooting guides.
  - name: Discussions
    url: https://github.com/joshuadavidthomas/django-language-server/discussions
    about: Ask questions and discuss ideas.
```

### Success Criteria

#### Automated Verification:
- [ ] YAML syntax valid: `python -c "import yaml; yaml.safe_load(open('.github/ISSUE_TEMPLATE/config.yml'))"`

#### Manual Verification:
- [ ] Config allows blank issues (for edge cases and general issues)
- [ ] Contact links point to correct URLs

---

## Phase 4: Create Template Validation Mismatch Issue Form

### Overview

Create a high-signal issue form specifically for reporting mismatches between djls static validation and Django runtime behavior.

### Changes Required

#### 1. Create Issue Form

**File**: `.github/ISSUE_TEMPLATE/template-validation-mismatch.yml`

```yaml
name: Template Validation Mismatch
description: Report a difference between djls validation and Django runtime behavior
title: "[Validation] "
labels: ["validation", "needs-triage"]
body:
  - type: markdown
    attributes:
      value: |
        ## Template Validation Mismatch Report

        Use this form to report cases where djls produces a diagnostic that Django doesn't raise at runtime (false positive), or where Django raises an error that djls doesn't catch (false negative).

        **Before submitting**, please:
        - Check inspector health: enable `debug = true` in config and review server startup logs
        - Verify the issue reproduces with the latest djls version
        - Search existing issues for duplicates

  - type: dropdown
    id: mismatch-type
    attributes:
      label: Mismatch Type
      description: What kind of validation mismatch are you reporting?
      options:
        - "False Positive: djls reports error, Django renders fine"
        - "False Negative: Django raises error, djls reports nothing"
        - "Wrong Diagnostic: djls reports wrong error code or message"
    validations:
      required: true

  - type: input
    id: template-path
    attributes:
      label: Template File Path
      description: |
        Path to the template file (relative to project root), if relevant.
        Helps triage in multi-app or monorepo projects.
      placeholder: "myapp/templates/myapp/widget.html"
    validations:
      required: false

  - type: textarea
    id: template-snippet
    attributes:
      label: Template Snippet
      description: |
        Minimal template code that reproduces the issue.
        Please reduce to the smallest snippet that demonstrates the problem.
      placeholder: |
        {% load i18n %}
        {% trans "Hello" as greeting %}
        {{ greeting }}
      render: django
    validations:
      required: true

  - type: textarea
    id: expected-behavior
    attributes:
      label: Expected Behavior
      description: What should happen? Include Django's actual behavior if relevant.
      placeholder: |
        Django renders this template successfully.
        The `{% trans ... as var %}` syntax is valid and assigns to `greeting`.
    validations:
      required: true

  - type: textarea
    id: djls-diagnostic
    attributes:
      label: djls Diagnostic
      description: |
        What diagnostic does djls produce? Include the full message and code.
        If djls produces no diagnostic but should, describe what's missing.
      placeholder: |
        S105: Too many arguments for tag 'trans'
        
        Or: No diagnostic produced (expected S109 for unloaded library)
    validations:
      required: true

  - type: input
    id: djls-version
    attributes:
      label: djls Version
      description: Output of `djls --version`
      placeholder: "djls 6.0.0"
    validations:
      required: true

  - type: input
    id: django-version
    attributes:
      label: Django Version
      description: Output of `python -c "import django; print(django.VERSION)"`
      placeholder: "(5, 2, 0, 'final', 0)"
    validations:
      required: true

  - type: input
    id: python-version
    attributes:
      label: Python Version
      description: Output of `python --version`
      placeholder: "Python 3.12.0"
    validations:
      required: true

  - type: dropdown
    id: os
    attributes:
      label: Operating System
      options:
        - Linux
        - macOS
        - Windows
        - Other
    validations:
      required: true

  - type: dropdown
    id: inspector-health
    attributes:
      label: Inspector Health
      description: |
        Check server startup logs (with `debug = true` in config) for inspector status.
        Inspector health affects which diagnostics are produced.
      options:
        - "Healthy: Inspector initialized successfully (tags/filters loaded)"
        - "Unhealthy: Inspector failed (include logs below)"
        - "Unknown: Haven't checked logs"
    validations:
      required: true

  - type: textarea
    id: server-logs
    attributes:
      label: Server Logs (Inspector Initialization)
      description: |
        With `debug = true` in your config, include the server startup logs 
        around inspector/Django initialization. This helps diagnose inspector issues.
      placeholder: |
        [DEBUG] Starting Django Language Server...
        [DEBUG] Python environment: /path/to/venv
        [DEBUG] Django settings: myproject.settings
        [DEBUG] Inspector: querying template tags...
        [DEBUG] Found 42 tags, 35 filters
        ...
      render: shell
    validations:
      required: false

  - type: textarea
    id: installed-apps
    attributes:
      label: Relevant Installed Apps
      description: |
        If the issue involves third-party template tags, list the relevant installed apps
        and any custom templatetags modules.
      placeholder: |
        INSTALLED_APPS = [
            'django.contrib.admin',
            'crispy_forms',
            'myapp',  # has templatetags/myapp_tags.py
        ]
    validations:
      required: false

  - type: textarea
    id: tagspecs
    attributes:
      label: Custom TagSpecs
      description: |
        If you have custom tagspecs defined, include them here.
        This helps determine if the issue is in djls core or custom configuration.
      placeholder: |
        [tagspecs]
        version = "0.6.0"
        
        [[tagspecs.libraries]]
        module = "myapp.templatetags.custom"
        ...
      render: toml
    validations:
      required: false

  - type: textarea
    id: additional-context
    attributes:
      label: Additional Context
      description: |
        Any other information that might help diagnose the issue:
        - Editor/client being used
        - Steps to reproduce
        - Workarounds you've tried
    validations:
      required: false

  - type: checkboxes
    id: checklist
    attributes:
      label: Checklist
      description: Please confirm the following
      options:
        - label: I have reduced the template to a minimal reproducing example
          required: true
        - label: I have verified this issue with the latest djls version
          required: true
        - label: I have searched existing issues for duplicates
          required: true
```

### Success Criteria

#### Automated Verification:
- [ ] YAML syntax valid: `python -c "import yaml; yaml.safe_load(open('.github/ISSUE_TEMPLATE/template-validation-mismatch.yml'))"`
- [ ] All required fields have `validations.required: true`

#### Manual Verification:
- [ ] Form renders correctly on GitHub (test by visiting repo → Issues → New Issue)
- [ ] Required fields are enforced (mismatch type, template snippet, expected behavior, djls diagnostic, versions, OS, inspector health)
- [ ] Dropdowns have sensible options
- [ ] Placeholders provide useful guidance
- [ ] Checklist enforces quality bar

---

## Phase 5: Update TagSpecs Documentation

### Overview

Add a separate link in tagspecs.md for template validation mismatches, keeping the existing general issue link unchanged.

### Changes Required

#### 1. Add Validation Mismatch Link

**File**: `docs/configuration/tagspecs.md`

**Find**:
```markdown
If you encounter issues during migration, please [open an issue](https://github.com/joshuadavidthomas/django-language-server/issues) with your tagspec configuration.
```

**Replace with**:
```markdown
If you encounter issues during migration, please [open an issue](https://github.com/joshuadavidthomas/django-language-server/issues) with your tagspec configuration.

If you believe djls template validation is incorrect compared to Django runtime behavior (false positives or false negatives), please use the [Template Validation Mismatch](https://github.com/joshuadavidthomas/django-language-server/issues/new?template=template-validation-mismatch.yml) form.
```

### Success Criteria

#### Automated Verification:
- [ ] Link syntax is valid markdown
- [ ] Documentation builds without errors

#### Manual Verification:
- [ ] General issue link still points to issues page (for config/migration issues)
- [ ] Validation mismatch link opens the specific form
- [ ] Distinction between the two types of issues is clear

---

## Testing Strategy

### Documentation Build Test

```bash
# Build documentation using repo tooling
just docs build

# Serve locally for visual verification
just docs serve

# Alternative: direct mkdocs (if just not available)
# uv run --group docs mkdocs build --strict --config-file .mkdocs.yml
```

### YAML Validation

```bash
# Validate all YAML files
python -c "
import yaml
from pathlib import Path

files = [
    '.mkdocs.yml',
    '.github/ISSUE_TEMPLATE/config.yml',
    '.github/ISSUE_TEMPLATE/template-validation-mismatch.yml',
]

for f in files:
    p = Path(f)
    if p.exists():
        yaml.safe_load(p.read_text())
        print(f'✓ {f}')
    else:
        print(f'✗ {f} (not found)')
"
```

### Link Verification

After pushing to a branch, verify:
1. GitHub renders issue form correctly
2. "New Issue" shows the template option
3. Form enforces required fields
4. Documentation links resolve

---

## Performance Considerations

- Documentation changes have no runtime impact
- Issue template adds negligible repo size (~5KB)
- No CI/workflow changes required

---

## Migration Notes

This is **additive only** — no breaking changes:
- New documentation page added
- Existing diagnostic codes section expanded (backward compatible)
- New issue template added (doesn't affect existing issues)
- Existing TagSpecs issue link unchanged

---

## References

- Charter: [`.agents/charter/2026-02-05-template-validation-port-charter.md`](../charter/2026-02-05-template-validation-port-charter.md)
- M3 Plan: [`.agents/plans/2026-02-05-m3-load-scoping.md`](2026-02-05-m3-load-scoping.md) (S108-S110 codes)
- M4 Plan: [`.agents/plans/2026-02-05-m4-filters-pipeline.md`](2026-02-05-m4-filters-pipeline.md) (S111-S113 codes)
- M6 Plan: [`.agents/plans/2026-02-05-m6-rule-evaluation.md`](2026-02-05-m6-rule-evaluation.md) (S114-S116 codes)
- Current docs: `docs/configuration/index.md`, `docs/configuration/tagspecs.md`
- MkDocs config: `.mkdocs.yml`
