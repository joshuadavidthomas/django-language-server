# M9: Remove User Config TagSpecs — Extraction Replaces Everything

## Overview

Remove the entire user-config `tagspecs` system — the TOML schema, config types (`TagSpecDef`, `TagDef`, `TagArgDef`, etc.), legacy format support, the `From` conversion pipeline into semantic types, the `Project.tagspecs` salsa input field, and the user-config merge layer in `compute_tag_specs`. Also remove the `TagArg`-based argument validation engine and its 5 dead error variants (S104–S107).

After M8, Python AST extraction discovers everything: tag structure (block type, end tags, intermediates), argument validation rules (`ExtractedRule`), and argument structure for completions (`ExtractedArg`). The entire user-config TagSpecs system is dead weight — nobody uses it because the format is opaque, and extraction handles all the same information automatically.

Users who encounter false positives from extraction can suppress via `diagnostics.severity.S117 = "off"` or file a GitHub issue.

## Current State Analysis

### What the tagspecs config system provides

The `[tagspecs]` section in `djls.toml` lets users define:

| Feature | Config type | Extraction equivalent |
|---------|------------|----------------------|
| Tag type (block/standalone) | `TagTypeDef` | `BlockTagSpec` (end tag presence) |
| End tag name | `EndTagDef` | `BlockTagSpec.end_tag` |
| Intermediate tags | `IntermediateTagDef` | `BlockTagSpec.intermediate_tags` |
| Module path | `TagLibraryDef.module` | `RegistrationInfo` module detection |
| Argument validation | `TagArgDef` / `ArgKindDef` | `ExtractedRule` conditions |
| Argument completions | `TagArgDef` | `ExtractedArg` from AST |

**Every feature has an extraction equivalent.** The config system is redundant.

### The plumbing to remove

**Config layer (`djls-conf`):**
- `tagspecs.rs` — `TagSpecDef`, `TagLibraryDef`, `TagDef`, `EndTagDef`, `IntermediateTagDef`, `TagTypeDef`, `PositionDef`, `TagArgDef`, `ArgKindDef`, `ArgTypeDef` (~200 lines)
- `tagspecs/legacy.rs` — `LegacyTagSpecDef`, `LegacyEndTagDef`, `LegacyIntermediateTagDef`, `LegacyTagArgDef`, `LegacyArgTypeDef`, `LegacySimpleArgTypeDef`, `convert_legacy_tagspecs` (~430 lines)
- `lib.rs` — `tagspecs` field on `Settings`, `deserialize_tagspecs` custom deserializer, `Settings::tagspecs()` accessor, 10 re-exports, ~400 lines of tagspec tests

**Project layer (`djls-project`):**
- `project.rs` — `tagspecs: TagSpecDef` field on `Project` salsa input

**Server layer (`djls-server`):**
- `db.rs` — layer 4 merge in `compute_tag_specs`, `update_project_from_settings` tagspec diffing, 2 invalidation tests using `TagLibraryDef`

**Semantic layer (`djls-semantic`):**
- `specs.rs` — `TagSpecs::from_config_def()`, `impl From<(TagDef, String)> for TagSpec`, `impl From<EndTagDef> for EndTag`, `impl From<IntermediateTagDef> for IntermediateTag`, `impl From<&Settings> for TagSpecs`, `impl From<TagArgDef> for TagArg`
- `specs.rs` — `TagArg` enum (7 variants), `TokenCount`, `LiteralKind`, `TagArgSliceExt`, all TagArg constructors
- `specs.rs` — `args: L<TagArg>` field on `TagSpec`, `EndTag`, `IntermediateTag`
- `builtins.rs` — ~500 lines of hand-crafted `TagArg` arrays
- `arguments.rs` — `validate_args_against_spec`, `validate_argument_order` (~200 lines)
- `errors.rs` — 5 error variants: `MissingRequiredArguments`, `TooManyArguments`, `MissingArgument`, `InvalidLiteralArgument`, `InvalidArgumentChoice`

**IDE layer (`djls-ide`):**
- `diagnostics.rs` — S104-S107 code mappings and span arms
- `completions.rs` — `TagArg` match arms for argument completion
- `snippets.rs` — `TagArg` match arms for snippet generation

**Docs:**
- `docs/configuration/tagspecs.md` — entire page
- `docs/configuration/index.md` — S104-S107 in diagnostic codes table, `tagspecs` config section

### Key Discoveries

- `Project.tagspecs` is a salsa input field (`project.rs:47`) — removing it changes the `Project::new()` and `Project::bootstrap()` signatures
- `compute_tag_specs` (`db.rs:148-170`) has 4 layers; layer 4 (user config) is the one being removed
- `update_project_from_settings` (`db.rs:376-383`) compares and sets `tagspecs` — this diff/set logic goes away
- `Settings` has a custom `deserialize_tagspecs` function (`lib.rs:81-105`) that tries v0.6.0 then falls back to legacy with deprecation warning — all of this goes
- `impl From<&Settings> for TagSpecs` (`specs.rs:210-220`) calls `from_config_def` then merges builtins — both the `From` impl and `from_config_def` go
- `TagArg` is used in `completions.rs` and `snippets.rs` but M8 Phase 4 replaces those with `ExtractedArg`-based logic before M9 runs
- All 5 dead error variants (S104-S107) are only accumulated in `arguments.rs` — no other module produces them
- Legacy format tests (`legacy.rs:230-430`) are entirely self-contained and can be deleted wholesale
- The `tagspecs_change_invalidates` test in `db.rs:743-763` and the `tag_index_invalidation` test (`db.rs:820-837`) both construct `TagLibraryDef` — they need to be rewritten to use extraction-based invalidation or deleted

## Desired End State

After M9:

1. **No `[tagspecs]` section** recognized in `djls.toml` / `pyproject.toml` — silently ignored if present
2. **No tagspec types** in `djls-conf` — `TagSpecDef`, `TagDef`, `TagArgDef`, etc. all deleted
3. **No `Project.tagspecs`** salsa input — one fewer invalidation trigger
4. **No user-config merge layer** in `compute_tag_specs` — 3 layers remain (builtins, workspace extraction, external extraction)
5. **No `TagArg` enum** or associated types — extraction is the sole argument source
6. **No old validation engine** — `validate_args_against_spec`/`validate_argument_order` deleted
7. **No S104–S107** — 5 error variants and their diagnostic codes removed
8. **Simplified docs** — no tagspecs config page, diagnostic codes table updated

### Verification

```bash
cargo test -q
cargo clippy -q --all-targets --all-features --fix -- -D warnings
cargo +nightly fmt
```

## What We're NOT Doing

- **Removing extraction** — extraction is the replacement, not the target
- **Removing `diagnostics.severity` config** — users still need severity overrides for suppression
- **Adding per-tag diagnostic overrides** — `diagnostics.severity.S117 = "off"` is sufficient
- **Removing `EndTag`/`IntermediateTag` semantic types** — these still exist on `TagSpec` in `djls-semantic`, populated by extraction and builtins. Only their `args` fields and `From<conf types>` impls are removed.

## Implementation Approach

Four phases. Phase 1 removes the config types and plumbing. Phase 2 removes the semantic types and validation code. Phase 3 removes dead error variants. Phase 4 updates docs.

**Prerequisite:** M8 must be complete. The extracted rule evaluator (S117) and `ExtractedArg`-powered completions must be the active paths.

---

## Phase 1: Remove TagSpecs Config System

### Overview

Delete the entire tagspecs module from `djls-conf`, remove the `tagspecs` field from `Settings` and `Project`, remove the user-config merge layer from `compute_tag_specs`, and remove all `From<conf types>` conversions from `djls-semantic`.

### Changes Required

#### 1. Delete tagspecs module from `djls-conf`

**Delete files:**
- `crates/djls-conf/src/tagspecs.rs` — all types (`TagSpecDef`, `TagLibraryDef`, `TagDef`, `EndTagDef`, `IntermediateTagDef`, `TagTypeDef`, `PositionDef`, `TagArgDef`, `ArgKindDef`, `ArgTypeDef`)
- `crates/djls-conf/src/tagspecs/legacy.rs` — all legacy types and conversion functions

**File:** `crates/djls-conf/src/lib.rs`

Remove:
- `pub mod tagspecs;` (line 2)
- All 10 re-exports (lines 21-30): `ArgKindDef`, `ArgTypeDef`, `EndTagDef`, `IntermediateTagDef`, `PositionDef`, `TagArgDef`, `TagDef`, `TagLibraryDef`, `TagSpecDef`, `TagTypeDef`
- `tagspecs` field from `Settings` struct (lines 74-75)
- `deserialize_tagspecs` function (lines 81-105)
- `Settings::tagspecs()` accessor (lines 200-202)
- The `tagspecs: TagSpecDef::default()` in the `Settings` defaults (line 233)
- The tagspec override logic in `Settings::new()` (lines 124-125): `if !overrides.tagspecs.libraries.is_empty() { settings.tagspecs = overrides.tagspecs; }`
- Entire `mod tagspecs { ... }` test module (lines 539-990+) — all tagspec parsing and legacy conversion tests

**Backward compatibility:** Existing `djls.toml` files with `[tagspecs]` sections will have those sections silently ignored by serde (unknown fields are skipped by default unless `deny_unknown_fields` is enabled, which it isn't).

#### 2. Remove `tagspecs` field from `Project` salsa input

**File:** `crates/djls-project/src/project.rs`

Remove:
- `use djls_conf::TagSpecDef;` import (line 4)
- `pub tagspecs: TagSpecDef` field (line 47) and its doc comment (lines 44-46)

Update `Project::bootstrap()` (line 116):
- Remove `settings.tagspecs().clone()` from the `Project::new()` call — shift remaining args up

#### 3. Remove user-config merge layer from `compute_tag_specs`

**File:** `crates/djls-server/src/db.rs`

In `compute_tag_specs` (lines 148-170), remove layer 4 (lines 163-167):
```rust
// DELETE THIS BLOCK:
// Apply user config overrides (highest priority)
let user_specs = TagSpecs::from_config_def(project.tagspecs(db));
if !user_specs.is_empty() {
    specs.merge(user_specs);
}
```

Update function doc comment to list only 3 layers, remove `User config changes → via Project.tagspecs` from invalidation triggers.

In `update_project_from_settings` (around line 376-383), remove the tagspecs diff/set logic:
```rust
// DELETE:
let new_tagspecs = settings.tagspecs().clone();
if project.tagspecs(self) != &new_tagspecs {
    changed = true;
    project.set_tagspecs(self).to(new_tagspecs);
}
```

Update `Session::new()` / `DjangoDatabase::create_project()` — remove `settings.tagspecs().clone()` from `Project::new()` call (around line 712).

Update or delete tests:
- `tagspecs_change_invalidates` test (lines 743-763) — **delete entirely** (tests layer 4 which no longer exists)
- `tag_index_invalidation` test (lines 820-837) — if it uses `set_tagspecs`, rewrite to use extraction-based invalidation instead
- Remove `use djls_conf::TagLibraryDef;` import in test module (line 643)

#### 4. Remove `From<conf types>` conversions and `from_config_def` from `djls-semantic`

**File:** `crates/djls-semantic/src/templatetags/specs.rs`

Delete:
- `TagSpecs::from_config_def()` method (lines 190-208)
- `impl From<&djls_conf::Settings> for TagSpecs` (lines 210-220)
- `impl From<(djls_conf::TagDef, String)> for TagSpec` (lines 236-276)
- `impl From<djls_conf::EndTagDef> for EndTag` (lines 585-596)
- `impl From<djls_conf::IntermediateTagDef> for IntermediateTag` (lines 606-614)

Delete tests that use conf types:
- `test_conversion_from_conf_types` (lines 950-1070)
- `test_conversion_from_settings` (lines 1080-1145) — creates TOML with tagspecs and asserts conversion
- Any other test that constructs `djls_conf::TagDef`, `djls_conf::EndTagDef`, etc.

### Success Criteria

#### Automated Verification:
- [ ] `cargo test -q` — all tests pass across all crates
- [ ] `cargo clippy -q --all-targets --all-features -- -D warnings`
- [ ] No `TagSpecDef`, `TagLibraryDef`, `TagDef`, `EndTagDef`, `IntermediateTagDef`, `TagTypeDef` types exist in `djls-conf`
- [ ] No `tagspecs` field on `Settings` or `Project`
- [ ] `compute_tag_specs` has 3 layers (builtins, workspace extraction, external extraction)
- [ ] Existing `djls.toml` with `[tagspecs]` section parses without error

**Implementation Note:** After completing this phase and all automated verification passes, pause here for manual confirmation before proceeding to Phase 2.

---

## Phase 2: Remove `TagArg` System and Old Validation Engine

### Overview

Delete the `TagArg` enum and all associated types, remove the `args` field from `TagSpec`/`EndTag`/`IntermediateTag`, delete `validate_args_against_spec` and `validate_argument_order`, strip ~500 lines from `builtins.rs`, and remove `TagArg` references from `djls-ide`.

### Changes Required

#### 1. Delete `TagArg` and associated types from `specs.rs`

**File:** `crates/djls-semantic/src/templatetags/specs.rs`

Delete entirely:
- `TokenCount` enum (lines 18-23)
- `LiteralKind` enum (lines 30-40)
- `TagArg` enum (lines 344-378) — all 7 variants
- `impl TagArg` block (lines 381-493) — `name()`, `is_required()`, `expr()`, `var()`, `syntax()`, `modifier()`, `string()`, `choice()`, `varargs()`, `assignment()`
- `TagArgSliceExt` trait and impl (lines 495-510)
- `impl From<djls_conf::TagArgDef> for TagArg` (lines 515-568) — already broken from Phase 1 since `TagArgDef` is gone

Remove `args` field from:
- `TagSpec` (line 229): delete `pub args: L<TagArg>`
- `EndTag` (line 582): delete `pub args: L<TagArg>`
- `IntermediateTag` (line 603): delete `pub args: L<TagArg>`

Update `TagSpec::merge_block_spec()`:
- Remove `args: B(&[])` from `EndTag` construction (line 303)
- Remove `args: B(&[])` from `IntermediateTag` construction (line 313)

Update `TagSpec::from_extraction()`:
- Remove `args: B(&[])` from `TagSpec` construction (line 332)

#### 2. Update re-exports

**File:** `crates/djls-semantic/src/templatetags.rs`

Remove:
```rust
pub use specs::TagArg;
pub(crate) use specs::TagArgSliceExt;
pub use specs::LiteralKind;
pub use specs::TokenCount;
```

**File:** `crates/djls-semantic/src/lib.rs`

Remove:
```rust
pub use templatetags::TagArg;
pub use templatetags::LiteralKind;
pub use templatetags::TokenCount;
```

#### 3. Strip `args` from `builtins.rs`

**File:** `crates/djls-semantic/src/templatetags/builtins.rs`

Remove imports (lines 13-14, 17):
```rust
use super::specs::LiteralKind;
use super::specs::TagArg;
use super::specs::TokenCount;
```

For all 32 tag spec entries, remove the `args:` field. Delete:
- `BLOCKTRANS_ARGS` constant (lines 657-672)
- `args:` field from `TRANS_SPEC` constant (lines 679-706)
- Every `args: B(&[...])` line across all specs (~500 lines total)
- Every `args: B(&[])` from `EndTag` and `IntermediateTag` constructions

#### 4. Gut `arguments.rs`

**File:** `crates/djls-semantic/src/arguments.rs`

Delete:
- `validate_args_against_spec()` function (lines 80-116)
- `validate_argument_order()` function (lines 137-340)
- `use crate::templatetags::TagArg;` import (line 6)
- `use crate::templatetags::TagArgSliceExt;` import (line 7)

Simplify `validate_tag_arguments()` — after M8, this dispatches to the extracted rule evaluator only:

```rust
pub fn validate_tag_arguments(db: &dyn Db, tag_name: &str, bits: &[String], span: Span) {
    let tag_specs = db.tag_specs();

    if let Some(spec) = tag_specs.get(tag_name) {
        if !spec.extracted_rules.is_empty() {
            crate::rule_evaluation::evaluate_extracted_rules(
                db, tag_name, bits, &spec.extracted_rules, span,
            );
        }
        return;
    }
}
```

Update tests:
- Delete all tests that construct `TagArg` specs (~20 tests, lines 441-1025)
- Delete `check_validation_errors()` and `check_validation_errors_with_db()` helpers
- Delete `TestDatabase::with_custom_specs()` and `custom_specs` field
- Keep `test_endblock_with_name_is_valid`, `test_endblock_without_name_is_valid` (update to use `validate_template`)
- Update `test_for_rejects_extra_token_after_iterable` to expect `ExtractedRuleViolation` instead of `TooManyArguments`
- Remove `TagArg`-related imports from test module (`use crate::templatetags::django_builtin_specs`, `use crate::TagIndex` may still be needed — check)

#### 5. Remove `TagArg` from `djls-ide`

**File:** `crates/djls-ide/src/completions.rs`

Remove `use djls_semantic::TagArg;` import. All `TagArg` match arms (lines 423-490) and `spec.args`-based logic (lines 370, 416, 420, 494-495) should already be replaced by M8 Phase 4's `ExtractedArg`-based logic. Remove any remaining dead references.

**File:** `crates/djls-ide/src/snippets.rs`

Remove `use djls_semantic::TagArg;` import. The `generate_snippet_from_args(&[TagArg])` function (lines 6-60) and `generate_partial_snippet` (lines 98-105) should already be replaced by M8. Remove dead code and tests that construct `TagArg` instances (lines 128-239).

#### 6. Update `specs.rs` tests

**File:** `crates/djls-semantic/src/templatetags/specs.rs` (test module)

- Remove `args` from all `TagSpec`, `EndTag`, `IntermediateTag` constructions in `create_test_specs()`
- Delete `test_get_end_spec_for_closer` assertions about `endblock_spec.args`
- Delete `test_merge_block_spec_preserves_existing_end_tag_args` entirely
- Update `test_merge_block_spec_preserves_existing_intermediate_tags` — remove assertions about `intermediate.args.len()`

### Success Criteria

#### Automated Verification:
- [ ] `cargo test -q` — all tests pass across all crates
- [ ] `cargo clippy -q --all-targets --all-features -- -D warnings`
- [ ] No `TagArg`, `TokenCount`, `LiteralKind` types exist anywhere
- [ ] No `validate_args_against_spec` or `validate_argument_order` functions exist
- [ ] `builtins.rs` has zero `TagArg` references
- [ ] `completions.rs` and `snippets.rs` have zero `TagArg` references
- [ ] `grep -r "spec\.args" crates/` returns nothing

**Implementation Note:** After completing this phase and all automated verification passes, pause here for manual confirmation before proceeding to Phase 3.

---

## Phase 3: Remove Dead Error Variants and Diagnostic Codes

### Overview

5 `ValidationError` variants are now unreachable. Remove them along with their S-code mappings.

### Changes Required

#### 1. Remove error variants

**File:** `crates/djls-semantic/src/errors.rs`

Delete these variants:
- `MissingRequiredArguments { tag, min, span }` — was S104
- `TooManyArguments { tag, max, span }` — was S105
- `MissingArgument { tag, argument, span }` — was S104
- `InvalidLiteralArgument { tag, expected, span }` — was S106
- `InvalidArgumentChoice { tag, argument, choices, value, span }` — was S107

#### 2. Remove diagnostic code mappings

**File:** `crates/djls-ide/src/diagnostics.rs`

Remove from span extraction match:
```rust
| ValidationError::MissingRequiredArguments { span, .. }
| ValidationError::TooManyArguments { span, .. }
| ValidationError::MissingArgument { span, .. }
| ValidationError::InvalidLiteralArgument { span, .. }
| ValidationError::InvalidArgumentChoice { span, .. }
```

Remove from code mapping:
```rust
ValidationError::MissingRequiredArguments { .. } => "S104",
ValidationError::MissingArgument { .. } => "S104",
ValidationError::TooManyArguments { .. } => "S105",
ValidationError::InvalidLiteralArgument { .. } => "S106",
ValidationError::InvalidArgumentChoice { .. } => "S107",
```

#### 3. Fix match exhaustiveness

Grep for all remaining matches on these variants across all crates and remove those arms. Check:
- Any `filter` closures in tests that match `ValidationError::TooManyArguments`
- The `test_for_rejects_extra_token_after_iterable` test (should have been updated in Phase 2 to use `ExtractedRuleViolation`)

### Success Criteria

#### Automated Verification:
- [ ] `cargo test -q` — all tests pass
- [ ] `cargo clippy -q --all-targets --all-features -- -D warnings`
- [ ] No `S104`, `S105`, `S106`, `S107` strings exist in codebase
- [ ] No `MissingRequiredArguments`, `TooManyArguments`, `MissingArgument`, `InvalidLiteralArgument`, `InvalidArgumentChoice` exist in codebase

**Implementation Note:** After completing this phase and all automated verification passes, pause here for manual confirmation before proceeding to Phase 4.

---

## Phase 4: Update Documentation

### Overview

Delete the tagspecs documentation page, update the config docs to remove `tagspecs` as a config option, update the diagnostic codes table, and add migration guidance.

### Changes Required

#### 1. Delete tagspecs docs page

**Delete file:** `docs/configuration/tagspecs.md`

#### 2. Update MkDocs nav

**File:** `.mkdocs.yml`

Remove the `tagspecs.md` entry from the `nav:` section under configuration.

#### 3. Update `docs/configuration/index.md`

Remove:
- The `### tagspecs` config section (near the bottom) that references tagspecs documentation
- S104-S107 rows from the "Block Structure" diagnostic codes table
- Rename section header from "Block Structure (S100-S107)" to "Block Structure (S100-S103)"
- Any `diagnostics.severity` examples that use S104-S107

Ensure S117 (`ExtractedRuleViolation`) is documented (should already be from M8). It should be in an "Argument Validation" subsection.

Add a note to the config overview:

> Template tag validation — including argument checking, block structure, and completions — is handled automatically by analyzing your project's Python source code. No configuration needed.

#### 4. Update `docs/template-validation.md`

Remove any references to:
- User-defined tagspecs as a way to configure validation
- S104-S107 diagnostic codes
- The `args` configuration format

Add a note that argument validation uses Django's own error messages via AST extraction, and users can suppress with severity config if needed.

#### 5. Update links and cross-references

Search docs for links to `tagspecs.md` or references to `[tagspecs]` config and remove or redirect them. The `docs/configuration/index.md` previously linked to `tagspecs.md` — remove that link.

The `.github/ISSUE_TEMPLATE/config.yml` or template validation mismatch form may reference tagspecs — update if needed.

### Success Criteria

#### Automated Verification:
- [ ] `just docs build` succeeds (no broken links)
- [ ] No references to S104, S105, S106, S107 in docs
- [ ] No references to `tagspecs.md` in any doc file
- [ ] No `[tagspecs]` config examples in docs

#### Manual Verification:
- [ ] Review config docs — `tagspecs` section is gone, remaining config options are clear
- [ ] Review diagnostic codes table — S104-S107 removed, S117 present
- [ ] Review template-validation.md — accurate description of extraction-based validation

---

## Testing Strategy

### What's Deleted

| Test file | Tests removed | Reason |
|-----------|--------------|--------|
| `djls-conf/src/lib.rs` | ~15 tagspec parsing tests | Config types deleted |
| `djls-conf/src/tagspecs/legacy.rs` | 6 legacy conversion tests | Legacy module deleted |
| `djls-semantic/src/templatetags/specs.rs` | `test_conversion_from_conf_types`, `test_conversion_from_settings`, `test_merge_block_spec_preserves_existing_end_tag_args` | Conversion impls deleted, EndTag.args deleted |
| `djls-semantic/src/arguments.rs` | ~20 tests constructing TagArg specs | Old validation engine deleted |
| `djls-ide/src/snippets.rs` | ~6 snippet generation tests | TagArg-based snippets replaced by M8 |
| `djls-server/src/db.rs` | `tagspecs_change_invalidates`, possibly `tag_index_invalidation` | Tagspec salsa input deleted |

### What's Retained (updated)

- `test_endblock_with_name_is_valid` / `test_endblock_without_name_is_valid` — structural, no TagArg dependency
- `test_for_rejects_extra_token_after_iterable` — updated to expect `ExtractedRuleViolation`
- All block structure tests (S100-S103)
- All load resolution tests (S108-S113)
- All expression validation tests (S114)
- All filter arity tests (S115-S116)
- M8's rule evaluation unit tests and corpus validation tests

### Key Regression Tests

- `{% for item in items football %}` → `ExtractedRuleViolation` (S117, not TooManyArguments/S105)
- `{% for item in items %}` → no error
- `{% endblock content %}` → no error
- `{% if and x %}` → S114 (unchanged)
- Existing `djls.toml` with `[tagspecs]` section → parses without error, section silently ignored

## Performance Considerations

None. Pure dead code removal. Slightly faster config loading (no tagspec parsing).

## Migration Notes

- **Existing `djls.toml` files:** `[tagspecs]` sections are silently ignored. No parse errors. Users don't need to change anything immediately, but can clean up their config at leisure.
- **Diagnostic codes retired:** S104-S107 no longer exist. Users with `diagnostics.severity.S104 = "off"` will get a silent no-op. They should update to `S117` if they need to suppress argument validation.
- **False positive recourse:** `diagnostics.severity.S117 = "off"` to suppress all argument validation, or file a [Template Validation Mismatch](https://github.com/joshuadavidthomas/django-language-server/issues/new?template=template-validation-mismatch.yml) issue for specific tags.

## References

- M8 Plan: [`.agents/plans/2026-02-06-m8-extracted-rule-evaluation.md`](2026-02-06-m8-extracted-rule-evaluation.md)
- Config types: `crates/djls-conf/src/tagspecs.rs`, `crates/djls-conf/src/tagspecs/legacy.rs`
- Settings: `crates/djls-conf/src/lib.rs`
- Project salsa input: `crates/djls-project/src/project.rs`
- Server merge logic: `crates/djls-server/src/db.rs`
- Semantic specs: `crates/djls-semantic/src/templatetags/specs.rs`
- Builtins: `crates/djls-semantic/src/templatetags/builtins.rs`
- Old validation: `crates/djls-semantic/src/arguments.rs`
- Error types: `crates/djls-semantic/src/errors.rs`
- Diagnostic mapping: `crates/djls-ide/src/diagnostics.rs`
- Completions: `crates/djls-ide/src/completions.rs`
- Snippets: `crates/djls-ide/src/snippets.rs`
- User docs: `docs/configuration/tagspecs.md`, `docs/configuration/index.md`
