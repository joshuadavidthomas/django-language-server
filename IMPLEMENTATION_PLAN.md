# Implementation Plan — Extraction Crate Refactor (M14-M20)

**Source of truth:** `.agents/ROADMAP.md` (milestones M14-M20), `.agents/rfcs/2026-02-09-rfc-extraction-salsa-integration.md`

**Branch:** `eval-intent-opus-4.6`

## Progress

| Milestone | Status | Description |
|-----------|--------|-------------|
| M14 | **done** | Test baseline + corpus-grounded tests |
| M15 | **done** | Return values, not mutation (+ domain types T1-T4) |
| M16 | **done** | Split god-context (+ CompileFunction, OptionLoop) |
| M17 | **done** | Decompose blocks.rs into strategy modules |
| M18 | **done** | Move environment scanning to djls-project |
| M19 | stub | HelperCache → Salsa tracked functions |
| M20 | stub | Rename djls-extraction → djls-python |

## M14 — Test baseline + corpus-grounded tests ✅

Replaced fabricated tests with corpus-sourced equivalents. Cleaned orphaned snapshots.

## M15 — Return values, not mutation (+ domain types T1-T4) ✅

All eval/collection functions return values instead of mutating `&mut` params. New domain types: `ConstraintSet`, `Classification`, `SplitPosition`, `TokenSplit`.

## M16 — Split god-context (+ CompileFunction, OptionLoop) ✅

`AnalysisContext` renamed to `CallContext`. `constraints` and `known_options` removed from context; functions return `AnalysisResult` instead. `CompileFunction<'a>` validated input type added. `KnownOptions` kept (not renamed to `OptionLoop` — name describes semantics, not extraction origin).

## M17 — Decompose blocks.rs into strategy modules ✅

Split `blocks.rs` into `blocks/` with strategy submodules: `opaque.rs`, `dynamic_end.rs`, `next_token.rs`, `parse_calls.rs`. Orchestrator + shared helpers remain in `blocks.rs`. `BlockEvidence` enum not introduced — module structure already provides the separation. Public API unchanged.

## M18 — Move environment scanning to djls-project ✅

`environment/scan.rs` moved from `djls-extraction` to `djls-project/src/scanning.rs`. Types stay in `djls-extraction`. Scan functions now imported from `djls_project`.

## M19 — HelperCache → Salsa tracked functions

**RFC:** `.agents/rfcs/2026-02-09-rfc-extraction-salsa-integration.md`
**Plan file:** `.agents/plans/2026-02-09-m19-salsa-integration.md`

Replace `HelperCache` + manual recursion guards with Salsa tracked functions. Add `salsa` and `djls-source` dependencies. Introduce `parse_python_module`, `analyze_helper` (with cycle recovery), and `extract_module` tracked functions. Move `extract_module_rules` from `djls-server` into `djls-extraction`.

### Phase 1: Add Salsa + djls-source dependencies

- [x] **M19.1** Add `salsa = { workspace = true }` and `djls-source = { workspace = true }` to `crates/djls-extraction/Cargo.toml` dependencies.
- [x] **M19.2** Validate: `cargo build -q`, `cargo test -q` — all green (745 passed, 0 failed, 7 ignored), no regressions.

### Phase 2: Add `ParsedPythonModule` and `parse_python_module` tracked function

- [x] **M19.3** Create `crates/djls-extraction/src/parse.rs` with `ParsedPythonModule<'db>` (`#[salsa::tracked]`) and `parse_python_module(db, file)` (`#[salsa::tracked]`). Follow `djls-templates::parse_template` / `NodeList` pattern. Note: RFC's `no_eq` attribute is from Ruff's forked Salsa — upstream salsa 0.25.2 doesn't support it. Used `#[salsa::tracked]` with `#[tracked] #[returns(ref)]` on the body field, matching the `NodeList` pattern exactly.
- [x] **M19.4** Add `mod parse;` to `lib.rs` (under `#[cfg(feature = "parser")]`), re-export `parse_python_module` and `ParsedPythonModule`.
- [x] **M19.5** Validate: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q` — all green (745 passed, 0 failed, 7 ignored).

### Phase 3: Add `HelperCall` interned type and `analyze_helper` with cycle recovery

- [x] **M19.6** Add `HelperCall<'db>` (`#[salsa::interned]`) with `file: File`, `callee_name: String`, `args: Vec<AbstractValueKey>`. Made `AbstractValueKey` public (was private in `calls.rs`), re-exported from `dataflow.rs` and `lib.rs`.
- [x] **M19.7** Add `analyze_helper(db, call)` (`#[salsa::tracked(cycle_fn=..., cycle_initial=...)]`) that looks up function def in parsed module, runs eval, returns `AbstractValue`. Cycle recovery returns `Unknown`. Added `Eq` to `AbstractValue` (needed for Salsa tracked returns). Added `From<&AbstractValueKey> for AbstractValue` reverse conversion. Made `extract_return_value` pub(crate). Salsa 0.25.2 cycle_fn signature: `fn(db, &Cycle, &last_provisional, value, input) -> ReturnType`.
- [x] **M19.8** Add `db: &'a dyn djls_source::Db` and `file: File` to `CallContext`. Thread `db` through the eval call chain. Used `Option` wrappers so existing callers/tests pass `None`; `parse.rs::analyze_helper` passes `Some(db)` and `Some(file)`. Fields have `#[allow(dead_code)]` until M19.9 wires them into `resolve_call`.
- [x] **M19.9** Update `resolve_helper_call` in `dataflow/calls.rs` to construct `HelperCall` interned value and call `analyze_helper(db, call)` instead of `HelperCache` lookup + manual depth/recursion guards. When `ctx.db` and `ctx.file` are `Some`, `resolve_call` constructs a `HelperCall` and delegates to `analyze_helper` (Salsa tracked with cycle recovery). When `None` (tests/standalone), falls back to existing `resolve_call_manual` with `HelperCache` + depth guards. Removed `#[allow(dead_code)]` from `db`/`file` fields.
- [x] **M19.10** Delete `HelperCache`, `HelperCacheKey`, `MAX_CALL_DEPTH`, `caller_name`, `call_depth` from `CallContext`. Also removed `module_funcs` from `CallContext` (dead — Salsa path doesn't use it; `analyze_helper` gets funcs from parsed module), `name` from `CompileFunction` (only used for deleted `caller_name`), `analyze_compile_function_with_cache` (merged into `analyze_compile_function`), and `module_funcs` param from `analyze_compile_function` + `RegistrationKind::extract`. Tests in `calls.rs` converted from manual `HelperCache` + bounded inlining to Salsa test DB. `depth_limit` test renamed to `deep_call_chain_returns_unknown` — deep chains still return Unknown because `extract_return_value` uses `eval_expr` (no ctx), so nested helper calls in return expressions can't resolve. This is a pre-existing limitation, not a regression.
- [x] **M19.11** Validate: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q` — all green (745 passed, 0 failed, 7 ignored). `grep -rn "HelperCache\|call_depth\|caller_name\|MAX_CALL_DEPTH" crates/djls-extraction/src/` returns no results.

### Phase 4: Add `extract_module` tracked function, move from server

- [x] **M19.12** Add `extract_module(db, file)` tracked function to extraction crate (in `parse.rs`). Calls `parse_python_module`, runs extraction pipeline via shared `extract_rules_from_body` helper, returns `ExtractionResult`. Refactored `extract_rules` to also use the shared helper. Re-exported as `djls_extraction::extract_module`.
- [x] **M19.13** Update `crates/djls-server/src/db.rs`: remove `extract_module_rules`, update `collect_workspace_extraction_results` to call `djls_extraction::extract_module(db, file)`. Update server tests. Removed the `extract_module_rules` tracked function (12 lines). Updated `collect_workspace_extraction_results` to call `djls_extraction::extract_module(db, file)` directly. Updated 3 server tests to use `djls_extraction::extract_module` and check for `"extract_module"` in Salsa events. Salsa upcasting from `&dyn SemanticDb` to `&dyn djls_source::Db` works transparently for tracked functions.
- [x] **M19.14** Validate: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q` — all green (745 passed, 0 failed, 7 ignored).

### Phase 5: Final validation and cleanup

- [ ] **M19.15** Remove dead code, unused imports. Run `cargo +nightly fmt`.
- [ ] **M19.16** Final validation: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q` — all green (761+ passed, 0 failed, 7 ignored). Verify no `HelperCache` references in production code.

## M20 — Rename djls-extraction → djls-python

**RFC:** `.agents/rfcs/2026-02-09-rfc-extraction-salsa-integration.md`

_Tasks not yet expanded. Needs plan file: `.agents/plans/2026-02-09-m20-rename-crate.md`_

## Baseline

Starting point: 732 workspace tests (241 in djls-extraction). Every change must maintain all green.

## Current Test Counts (M18 complete)

| Suite | Passed |
|-------|--------|
| **Full workspace** | **745 passed, 0 failed, 7 ignored** |

