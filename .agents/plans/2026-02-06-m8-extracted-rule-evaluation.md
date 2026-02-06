# M8: Extracted Rule Evaluation — Complete Replacement of Static Argument Validation

## Overview

Build the evaluator that applies `ExtractedRule` conditions to template tag arguments, extract argument structure from the Python AST for completions/snippets, remove the old hand-crafted `args`-based validation path entirely, and **prove the system works via corpus-scale template validation tests**.

This milestone closes the gap identified in the M6 post-mortem: extraction rules are computed but never read. After M8, the old `builtins.rs` args and `validate_argument_order()` are gone — not "kept as fallback", gone.

## Current State (The Dual-System Problem)

- **OLD path (active):** `TagSpec.args` (hand-crafted `TagArg` specs in `builtins.rs`, ~973 lines) → `validate_args_against_spec()` → `validate_argument_order()`. Works for Django builtins only, hand-maintained.
- **NEW path (dead end):** `TagSpec.extracted_rules` (AST-derived `ExtractedRule` conditions) → stored via `merge_extracted_rules()` → **never read by anything**.
- **Completions/snippets** use `spec.args` for position-aware argument completion and snippet generation.

## Desired End State

1. **Extracted rule evaluator** validates template tag arguments using conditions from Python AST
2. **Argument structure** extracted from Python AST powers completions/snippets (replaces hand-crafted `args`)
3. **`validate_argument_order()`** and hand-crafted `args:` in `builtins.rs` are removed
4. **Corpus template validation tests** prove zero false positives against Django admin templates, Wagtail, allauth, crispy-forms, Sentry, NetBox
5. **No fallback** to old system — tags without extracted rules get no argument validation (conservative)

## What We're NOT Doing

- Keeping `args` as a fallback for builtins (the deferral pattern that created this mess)
- Variable type checking or cross-template state
- Perfect variable names for every manual tag (best-effort from AST, generic fallback)

---

## Phase 1: Argument Structure Extraction

**Goal**: Add a new extraction pass in `djls-extraction` that derives argument structure from the Python AST, producing `ExtractedArg` types that can power completions/snippets.

For `simple_tag`/`inclusion_tag`/`simple_block_tag`: extract directly from function parameters. The pattern already exists in `extract_filter_arity` in `filters.rs` — same approach but richer output. Handle `takes_context=True` (skip first param), `*args`, `**kwargs`, parameter defaults (optional vs required). Also append the `as varname` optional args that simple/inclusion tags get automatically from Django.

For manual `@register.tag`: reconstruct from `ExtractedRule` conditions plus AST analysis. `LiteralAt` rules give literal positions, `ChoiceAt` gives choice positions. For variable names, analyze tuple unpacking (`tag_name, item, _in, iterable = bits`) and indexed access (`format_string = bits[1]`). Fall back to generic names (`arg1`, `arg2`) when AST analysis can't determine better ones.

Add `extracted_args: Vec<ExtractedArg>` to `ExtractedTag`. Update golden tests.

## Phase 2: Extracted Rule Evaluator

**Goal**: Build the function that evaluates `ExtractedRule` conditions against template tag bits.

Create a `rule_evaluation` module in `djls-semantic`. Follow the `filter_arity.rs` pattern: resolve, lookup, evaluate, accumulate errors.

Critical implementation details:
- **Index offset**: Extraction uses `split_contents()` indices (tag name at 0). Parser `bits` excludes the tag name. Evaluator must adjust: extraction index N → `bits[N-1]`.
- **Negation semantics**: Rules represent error conditions. `negated: true` means "error when the condition is NOT met" (e.g., `LiteralAt{value:"in", negated:true}` = error when bits[1] != "in").
- **Opaque rules**: `RuleCondition::Opaque` = silently skip, never error.
- **Error messages**: Use `ExtractedRule.message` (Django's original error text) when available.
- **MaxArgCount semantics**: `MaxArgCount{max:3}` means "error when split_len ≤ 3" (represents `if len(bits) < 4` in Django source).

Add a new `ExtractedRuleViolation` error variant (S117) that carries Django's original message directly — more informative than generic S104-S107 messages.

Test each `RuleCondition` variant individually.

## Phase 3: Wire into Validation Pipeline

**Goal**: Replace the old `args`-based validation with the extracted rule evaluator. No fallback.

Modify `validate_tag_arguments` in `arguments.rs`: when `spec.extracted_rules` is non-empty, call the rule evaluator. When empty, skip argument validation entirely. Do NOT fall back to the old `args`-based path for builtins.

Remove the hand-crafted `args:` values from all 31 tag specs in `builtins.rs` (set to empty). Keep block structure (end tags, intermediates, module mappings).

Remove `EndTag.args` and `IntermediateTag.args` — extraction doesn't produce argument specs for closers/intermediates, and Django accepts them without restriction.

Keep `validate_args_against_spec`/`validate_argument_order` reachable ONLY for user-config-defined `args` in `djls.toml` (escape hatch for tags extraction can't handle). This is not a fallback — builtins all have extracted rules.

Key regression test: `{% for item in items football %}` must still produce an error.

## Phase 4: Wire Extracted Args into Completions/Snippets

**Goal**: Populate `TagSpec.args` from extraction-derived argument structure so completions and snippets continue working unchanged.

Convert `ExtractedArg` → `TagArg` during `compute_tag_specs` (in `merge_extraction_into_specs`). The completions and snippets code in `djls-ide` reads `spec.args` as before — the source just changes from hand-crafted to extraction-derived.

No changes needed to `completions.rs` or `snippets.rs` — they read `TagSpec.args` which is now populated from extraction.

## Phase 5: Clean Up Dead Code

**Goal**: Remove unused code paths and update documentation.

Remove `TagArgSliceExt` trait (only used by deleted `validate_argument_order` internals). Update doc comments on `TagSpec.args` to reflect its new role (completions only, not validation). Clean up `from_extraction` constructor to handle `extracted_args`. Update AGENTS.md operational notes.

Keep `TagArg` enum and S104-S107 variants — still needed for user-config `args` escape hatch.

## Phase 6: Corpus Template Validation Tests

**Goal**: Prove the system works at scale. This is THE proof.

Port the prototype's `test_corpus_templates.py` and `test_real_templates.py` to Rust. The prototype validated actual templates from the corpus against extracted rules and asserted zero false positives — this was never ported to Rust.

Create integration tests (likely in `crates/djls-server/tests/corpus_templates.rs`) that:
1. For each Django version in corpus (4.2, 5.1, 5.2, 6.0): extract rules from THAT version's source, validate its shipped templates (contrib/admin), assert zero false positives
2. For each third-party package (Wagtail, allauth, crispy-forms, debug-toolbar, compressor): extract rules from the package's own templatetags + Django builtins, validate templates, assert zero argument-validation false positives
3. For repos (Sentry, NetBox): same approach with entry-local extraction
4. Known-invalid templates produce expected errors
5. Tests skip gracefully when corpus not synced

Port the prototype's template exclusion list (AngularJS templates under static dirs, known-invalid upstream templates).

The test needs a lightweight `CorpusTestDatabase` that builds tag specs from extraction results rather than hand-crafted builtins.

---

## Testing Strategy

| Tier | What | Gating | Purpose |
|------|------|--------|---------|
| Unit | Per-RuleCondition variant, index offset, negation | Always | Correctness |
| Integration | Golden extraction snapshots with `extracted_args` | Always | Stability |
| Corpus | Template validation across entire corpus | Corpus synced | **THE PROOF** |

Key regression tests: `{% for item in items football %}` → error, `{% for item in items %}` → clean, `{% autoescape %}` → error, `{% if and x %}` → S114, Django admin templates → zero false positives.

## References

- Charter: [`.agents/charter/2026-02-05-template-validation-port-charter.md`](../charter/2026-02-05-template-validation-port-charter.md)
- M5 Plan: [`.agents/plans/2026-02-05-m5-extraction-engine.md`](2026-02-05-m5-extraction-engine.md)
- M6 Plan: [`.agents/plans/2026-02-05-m6-rule-evaluation.md`](2026-02-05-m6-rule-evaluation.md)
- Prototype corpus tests: `template_linter/tests/test_corpus_templates.py`, `template_linter/tests/test_real_templates.py`
- Working extraction→evaluation model: `crates/djls-semantic/src/filter_arity.rs`
