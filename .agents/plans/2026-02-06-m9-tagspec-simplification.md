# M9: Remove User Config TagSpecs — Extraction Replaces Everything

## Overview

Remove the entire user-config `tagspecs` system. After M8, Python AST extraction discovers everything: tag structure (block type, end tags, intermediates), argument validation rules, and argument structure for completions. The `[tagspecs]` section in `djls.toml`, the `TagArg`-based validation engine, and all the config/conversion plumbing are dead weight that nobody uses.

Users who encounter false positives from extraction can suppress via `diagnostics.severity.S117 = "off"` or file a GitHub issue.

## Current State (The Redundancy)

Every feature the tagspecs config provides has an extraction equivalent:

- Tag type (block/standalone) → `BlockTagSpec` from extraction
- End tag name → `BlockTagSpec.end_tag`
- Intermediate tags → `BlockTagSpec.intermediate_tags`
- Module path → `RegistrationInfo` module detection
- Argument validation → `ExtractedRule` conditions (S117)
- Argument completions → `ExtractedArg` from AST

The config system is a hand-maintained parallel universe: ~200 lines of config types, ~430 lines of legacy format support, ~500 lines of hand-crafted arg specs in builtins, ~200 lines of validation engine, 5 error variants, and a user-facing config schema so opaque nobody uses it.

## Desired End State

1. No `[tagspecs]` section recognized in config — silently ignored if present
2. No tagspec types in `djls-conf` — entire `tagspecs` module deleted
3. No `Project.tagspecs` salsa input — one fewer invalidation trigger
4. No user-config merge layer in `compute_tag_specs` — 3 layers remain (builtins, workspace extraction, external extraction)
5. No `TagArg` enum or old validation engine
6. No S104–S107 error variants
7. Docs updated — no tagspecs page, diagnostic codes table updated

## What We're NOT Doing

- Removing extraction (it's the replacement, not the target)
- Removing `diagnostics.severity` config (users still need suppression)
- Adding per-tag diagnostic overrides (severity suppression is sufficient)
- Removing `EndTag`/`IntermediateTag` semantic types (still populated by extraction and builtins, just losing their `args` fields and `From<conf types>` impls)

---

## Phase 1: Remove TagSpecs Config System

**Goal**: Delete the entire tagspecs module from `djls-conf`, remove `tagspecs` from `Settings` and `Project`, remove the user-config merge layer from `compute_tag_specs`.

Delete `crates/djls-conf/src/tagspecs.rs` and `crates/djls-conf/src/tagspecs/legacy.rs` entirely — all types gone (`TagSpecDef`, `TagLibraryDef`, `TagDef`, `EndTagDef`, `IntermediateTagDef`, `TagTypeDef`, `PositionDef`, `TagArgDef`, `ArgKindDef`, `ArgTypeDef`, and all legacy equivalents).

Strip the `tagspecs` field from `Settings` — remove the field, the custom `deserialize_tagspecs` function (tries v0.6.0 then legacy with deprecation warning), the `Settings::tagspecs()` accessor, and the override logic. Delete all tagspec-related tests in `lib.rs` (~15 tests).

Remove `tagspecs: TagSpecDef` from the `Project` salsa input. This changes `Project::new()` and `Project::bootstrap()` signatures — one fewer argument.

In `compute_tag_specs` in `db.rs`, delete layer 4 (user config merge). In `update_project_from_settings`, remove the tagspec diff/set logic. Delete the `tagspecs_change_invalidates` test and update `tag_index_invalidation` if it uses `set_tagspecs`.

In `djls-semantic/specs.rs`, delete `TagSpecs::from_config_def()`, `impl From<(TagDef, String)> for TagSpec`, `impl From<&Settings> for TagSpecs`, `impl From<EndTagDef> for EndTag`, `impl From<IntermediateTagDef> for IntermediateTag`. Delete tests that use conf types.

Existing `djls.toml` files with `[tagspecs]` sections will parse cleanly — serde silently ignores unknown fields.

## Phase 2: Remove `TagArg` System and Old Validation Engine

**Goal**: Delete the `TagArg` enum and all associated types, remove `args` from `TagSpec`/`EndTag`/`IntermediateTag`, delete the old validation functions, strip ~500 lines from `builtins.rs`.

Delete from `specs.rs`: `TokenCount`, `LiteralKind`, `TagArg` (7 variants + all constructors), `TagArgSliceExt`, `From<TagArgDef> for TagArg`. Remove the `args` field from `TagSpec`, `EndTag`, `IntermediateTag`. Update `merge_block_spec` and `from_extraction` constructors. Remove re-exports from `templatetags.rs` and `lib.rs`.

Strip all `args:` from `builtins.rs` — delete the `BLOCKTRANS_ARGS` constant, remove `args` from `TRANS_SPEC`, remove every `args: B(&[...])` line. Keep block structure (end tags, intermediates, module mappings).

Gut `arguments.rs`: delete `validate_args_against_spec` and `validate_argument_order`. Simplify `validate_tag_arguments` to only dispatch to the extracted rule evaluator. Delete ~20 tests that construct `TagArg` specs. Keep structural tests (`endblock_with_name_is_valid`). Update `for_rejects_extra_token_after_iterable` to expect `ExtractedRuleViolation` instead of `TooManyArguments`.

Remove `TagArg` from `completions.rs` and `snippets.rs` — M8 Phase 4 already replaced these with `ExtractedArg`-based logic. Remove any remaining dead references.

## Phase 3: Remove Dead Error Variants and Diagnostic Codes

**Goal**: Remove 5 unreachable `ValidationError` variants and their S-code mappings.

Delete from `errors.rs`: `MissingRequiredArguments` (S104), `TooManyArguments` (S105), `MissingArgument` (S104), `InvalidLiteralArgument` (S106), `InvalidArgumentChoice` (S107).

Remove corresponding span extraction arms and code mapping arms from `diagnostics.rs`. Fix match exhaustiveness in any remaining files.

## Phase 4: Update Documentation

**Goal**: Delete the tagspecs docs page, update config docs, update diagnostic codes table.

Delete `docs/configuration/tagspecs.md`. Remove its entry from `.mkdocs.yml` nav.

In `docs/configuration/index.md`: remove the `tagspecs` config section, remove S104-S107 from the diagnostic codes table, rename "Block Structure (S100-S107)" to "Block Structure (S100-S103)". Add a note that template tag validation is handled automatically by Python AST extraction.

Update `docs/template-validation.md` to remove tagspec references and note that argument validation uses Django's own error messages via extraction.

---

## Migration Notes

- `[tagspecs]` in `djls.toml` → silently ignored, no parse errors
- S104-S107 → retired. Users with severity overrides for these codes get silent no-ops. Update to S117 if needed.
- False positives → `diagnostics.severity.S117 = "off"` or file a Template Validation Mismatch issue

## References

- M8 Plan: [`.agents/plans/2026-02-06-m8-extracted-rule-evaluation.md`](2026-02-06-m8-extracted-rule-evaluation.md)
- Prototype corpus tests: `template_linter/tests/test_corpus_templates.py`
- Working extraction→evaluation model: `crates/djls-semantic/src/filter_arity.rs`
