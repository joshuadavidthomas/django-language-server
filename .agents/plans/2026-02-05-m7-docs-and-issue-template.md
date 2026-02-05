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

1. **New documentation page** (`docs/template-validation.md`) explaining what djls validates vs what Django validates at runtime, the inspector + extraction architecture at a high level, and "inspector unavailable" behavior
2. **Updated diagnostic codes** in `docs/configuration/index.md` with S108-S116 grouped by category
3. **Navigation updated** in `.mkdocs.yml` with the new page
4. **GitHub issue form** (`.github/ISSUE_TEMPLATE/template-validation-mismatch.yml`) for reporting validation mismatches with a structured repro checklist

## What We're NOT Doing

- **API reference docs**: Internal architecture details beyond what users need
- **Tutorial/getting-started rewrites**: Focus on new validation features only
- **Automated debug dump command**: Manual collection instructions for now

---

## Implementation Plan

### Phase 1: Create Template Validation Documentation Page

**Goal**: Create `docs/template-validation.md` explaining the validation system.

Content should cover:
- **How validation works**: Inspector provides runtime inventory (what tags/filters exist, which libraries they belong to), Rust extraction provides validation rules (how to validate usage) via AST analysis
- **What djls validates**: Unknown tags/filters, unloaded library tags/filters, block structure, expression syntax in `{% if %}`/`{% elif %}`, filter argument arity
- **What djls cannot validate**: Runtime-only behavior (variable resolution, type coercion, format strings), dynamic tag behavior (tags that validate based on runtime state), template inheritance (`{% extends %}`/`{% include %}` — future work)
- **Inspector availability**: When healthy, full validation; when unavailable (Django init failed, Python not configured), scoping diagnostics are suppressed to avoid false positives
- **Ambiguous symbols**: When multiple libraries define the same name without a resolved registry, validation is skipped for that symbol with a warning
- **Reporting mismatches**: Link to the issue template

### Phase 2: Update Diagnostic Codes Documentation

**Goal**: Add S108-S116 to `docs/configuration/index.md`.

Group diagnostic codes by category:
- **Block Structure (S100-S107)**: Existing codes
- **Tag Scoping (S108-S110)**: UnknownTag, UnloadedTag, AmbiguousUnloadedTag
- **Filter Scoping (S111-S113)**: UnknownFilter, UnloadedFilter, AmbiguousUnloadedFilter
- **Expression & Filter Arity (S114-S116)**: ExpressionSyntaxError, FilterMissingArgument, FilterUnexpectedArgument

Add a link to the template validation page for more context.

### Phase 3: Create GitHub Issue Template Directory

**Goal**: Set up `.github/ISSUE_TEMPLATE/` with a config file.

Create `.github/ISSUE_TEMPLATE/config.yml` that links to documentation and existing issue channels.

### Phase 4: Create Template Validation Mismatch Issue Form

**Goal**: Create a YAML issue form at `.github/ISSUE_TEMPLATE/template-validation-mismatch.yml`.

The form should require:
- **djls version** and **Django version**
- **Minimal template snippet** reproducing the mismatch
- **Relevant `{% load %}` statements**
- **Expected behavior** (what Django does) vs **actual behavior** (what djls reports)
- **djls.toml excerpt** (diagnostic severity overrides, tagspecs)
- **Inspector status** (healthy or unavailable)

Include copy/paste commands for collecting debug information.

### Phase 5: Update TagSpecs Documentation

**Goal**: Update `docs/configuration/tagspecs.md` to link to the new issue template for validation mismatches, replacing the generic "open an issue" text.

---

## Testing Strategy

- **Documentation build**: Verify mkdocs builds without errors (`mkdocs build --strict` if available)
- **YAML validation**: Verify issue template YAML is valid
- **Link verification**: Check internal documentation links resolve correctly

## References

- Charter: [`.agents/charter/2026-02-05-template-validation-port-charter.md`](../charter/2026-02-05-template-validation-port-charter.md)
- M3: [`.agents/plans/2026-02-05-m3-load-scoping.md`](2026-02-05-m3-load-scoping.md) (S108-S110)
- M4: [`.agents/plans/2026-02-05-m4-filters-pipeline.md`](2026-02-05-m4-filters-pipeline.md) (S111-S113)
- M6: [`.agents/plans/2026-02-05-m6-rule-evaluation.md`](2026-02-05-m6-rule-evaluation.md) (S114-S116)
