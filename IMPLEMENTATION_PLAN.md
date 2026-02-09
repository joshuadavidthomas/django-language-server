# Implementation Plan — Extraction Crate Refactor (M14-M20)

**Source of truth:** `.agents/ROADMAP.md` (milestones M14-M20), `.agents/rfcs/2026-02-09-rfc-extraction-salsa-integration.md`

**Branch:** `eval-intent-opus-4.6`

## Progress

| Milestone | Status | Description |
|-----------|--------|-------------|
| M14 | planned | Test baseline + corpus-grounded tests |
| M15 | stub | Return values, not mutation (+ domain types T1-T4) |
| M16 | stub | Split god-context (+ CompileFunction, OptionLoop) |
| M17 | stub | Decompose blocks.rs into strategy modules |
| M18 | stub | Move environment scanning to djls-project |
| M19 | stub | HelperCache → Salsa tracked functions |
| M20 | stub | Rename djls-extraction → djls-python |

## M14 — Test baseline + corpus-grounded tests

**Design docs:** `docs/dev/extraction-test-strategy.md`, `docs/dev/corpus-refactor.md`
**Plan file:** `.agents/plans/2026-02-09-m14-test-baseline.md`

### Phase 1: Record Baseline + Audit Fabricated Tests

- [ ] **M14.1** Record baseline test counts: run `cargo test -q -p djls-extraction` and `cargo test -q -p djls-extraction --test corpus`, record total test count (pass/fail/ignored), total snapshot count, and corpus test count in this file
- [ ] **M14.2** Audit fabricated Python tests across all extraction source files: categorize each test as (a) has corpus equivalent → replace, (b) pattern is real but no clean isolatable corpus example → keep with comment, or (c) pattern doesn't exist in real code → remove. Record audit results as a section below

### Phase 2: Create Corpus Test Helpers

- [ ] **M14.3** Add corpus test helpers to extraction crate test utilities: `find_function_in_source()`, `corpus_function()`, `corpus_source()` that work with `Corpus::discover()` and skip gracefully when corpus is not synced
- [ ] **M14.4** Validate: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings` pass, no test behavior changes

### Phase 3: Replace Fabricated Tests — Registration & Blocks & Filters

- [ ] **M14.5** Replace fabricated Python in `src/registry.rs` with corpus-sourced equivalents (map each registration pattern to a real Django function)
- [ ] **M14.6** Replace fabricated Python in `src/blocks.rs` with corpus-sourced equivalents (parser.parse, skip_past, next_token patterns from defaulttags.py, i18n.py)
- [ ] **M14.7** Replace fabricated Python in `src/filters.rs` with corpus-sourced equivalents (filter arity from defaultfilters.py)
- [ ] **M14.8** Replace fabricated Python in `src/signature.rs` with corpus-sourced equivalents (simple_tag/inclusion_tag parameter patterns)
- [ ] **M14.9** Update and review snapshots — extraction results must be equivalent. Run `cargo insta test --accept -p djls-extraction` and review diffs
- [ ] **M14.10** Validate: `cargo test -q -p djls-extraction`, `cargo clippy -q --all-targets --all-features -- -D warnings` clean

### Phase 4: Replace Fabricated Tests — Dataflow

- [ ] **M14.11** Replace fabricated Python in `src/dataflow/constraints.rs` that models Django guard patterns with corpus-sourced equivalents. Keep inherently unit-level constraint logic tests as fabricated with justification comments
- [ ] **M14.12** Replace fabricated Python in `src/dataflow/eval.rs` that models Django compile function patterns with corpus-sourced equivalents. Keep pure unit tests (abstract value arithmetic, env operations) as fabricated with justification
- [ ] **M14.13** Replace fabricated Python in `src/dataflow/calls.rs` with corpus-sourced equivalents (helper function inlining patterns, e.g. allauth parse_tag)
- [ ] **M14.14** Replace fabricated Python in `src/environment/scan.rs` with corpus-sourced equivalents (AST scanning for registration patterns)
- [ ] **M14.15** Validate: `cargo test -q -p djls-extraction`, snapshots reviewed, `cargo clippy -q --all-targets --all-features -- -D warnings` clean

### Phase 5: Replace Fabricated Tests — Golden/End-to-End

- [ ] **M14.16** Audit and replace fabricated Python in `src/lib.rs` golden tests with corpus-sourced equivalents. Keep edge case tests (malformed registrations, error handling) as fabricated with documented justification
- [ ] **M14.17** Run `cargo insta test --accept --unreferenced delete -p djls-extraction` to clean up orphaned snapshots
- [ ] **M14.18** Validate: `cargo test -q -p djls-extraction`, no orphaned snapshot files, `cargo clippy -q --all-targets --all-features -- -D warnings` clean

### Phase 6: Validation — Full Suite Green

- [ ] **M14.19** Run full suite: `cargo build -q`, `cargo test -q`, `cargo clippy -q --all-targets --all-features -- -D warnings` — all green across all crates
- [ ] **M14.20** Update baseline counts in this file with final numbers, mark M14 as "done" in progress table

## M15 — Return values, not mutation (+ domain types T1-T4)

**Design docs:** `docs/dev/extraction-refactor-plan.md` (Phase 1), `docs/dev/extraction-type-driven-vision.md`

_Tasks not yet expanded. Needs plan file: `.agents/plans/2026-02-09-m15-return-values.md`_

## M16 — Split god-context (+ CompileFunction, OptionLoop)

**Design docs:** `docs/dev/extraction-refactor-plan.md` (Phase 2)

_Tasks not yet expanded. Needs plan file: `.agents/plans/2026-02-09-m16-split-context.md`_

## M17 — Decompose blocks.rs into strategy modules

**Design docs:** `docs/dev/extraction-refactor-plan.md` (Phase 3), `docs/dev/extraction-type-driven-vision.md` (`BlockEvidence`)

_Tasks not yet expanded. Needs plan file: `.agents/plans/2026-02-09-m17-decompose-blocks.md`_

## M18 — Move environment scanning to djls-project

_Tasks not yet expanded. Needs plan file: `.agents/plans/2026-02-09-m18-move-env-scanning.md`_

## M19 — HelperCache → Salsa tracked functions

**RFC:** `.agents/rfcs/2026-02-09-rfc-extraction-salsa-integration.md`

_Tasks not yet expanded. Needs plan file: `.agents/plans/2026-02-09-m19-salsa-integration.md`_

## M20 — Rename djls-extraction → djls-python

**RFC:** `.agents/rfcs/2026-02-09-rfc-extraction-salsa-integration.md`

_Tasks not yet expanded. Needs plan file: `.agents/plans/2026-02-09-m20-rename-crate.md`_

## Discoveries

_(Record anything learned during implementation that affects future milestones)_
