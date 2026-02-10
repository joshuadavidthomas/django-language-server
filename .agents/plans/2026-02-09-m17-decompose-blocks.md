# M17: Decompose blocks.rs into Strategy Modules

## Overview

Split `blocks.rs` (1382 lines) into strategy modules organized under a `blocks/` directory. The file contains four distinct block detection strategies mixed together with shared helpers. Each strategy becomes its own module, and an orchestrator composes them. Optionally introduces `BlockEvidence` to separate observation from interpretation.

## Current State Analysis

### What Exists (blocks.rs — 1382 lines)

The file contains one public function (`extract_block_spec`) that dispatches to four detection strategies:

1. **Opaque detection** (~90 lines): `collect_skip_past_tokens()`, `extract_skip_past_token()` — detects `parser.skip_past("endtag")` patterns
2. **Parse-call extraction** (~340 lines): `collect_parser_parse_calls()`, `extract_parse_call_info()`, `classify_stop_tokens()`, `classify_in_body()`, `classify_from_if_chain()` — detects `parser.parse(("token1", "token2"))` and classifies tokens as end-tag vs intermediate
3. **Dynamic end-tag detection** (~110 lines): `has_dynamic_end_in_body()`, `is_dynamic_end_parse_call()`, `is_end_fstring()`, `has_dynamic_end_tag_format()`, `is_end_format_expr()` — detects `parser.parse((f"end{tag_name}",))`
4. **Next-token loop detection** (~230 lines): `extract_next_token_loop_spec()`, `has_next_token_loop()`, `is_parser_tokens_check()`, `body_has_next_token_call()`, `is_next_token_call()`, `collect_token_content_comparisons()`, `extract_comparisons_from_expr()` — detects `parser.next_token()` in while-loops (blocktrans/blocktranslate pattern)

Shared helpers (~100 lines):
- `is_parser_receiver()` — checks if expr is `parser_var.method()`
- `extract_string_sequence()` — extracts strings from tuple/list expressions
- `body_has_parse_call()` — recursive check for parse calls in nested blocks
- `extract_token_check()` / `extract_startswith_check()` / `is_token_contents_expr()` — token comparison helpers

Internal types:
- `ParseCallInfo { stop_tokens, is_nested }` — info about a parse call site
- `Classification { intermediates, end_tags }` — result of stop-token classification

### What's Downstream (Must Not Break)

- `crate::lib.rs` — calls `blocks::extract_block_spec(func)` for block-tag registration
- All snapshot tests that include block structure in extraction results
- Corpus integration tests (`djls-server/tests/corpus_templates.rs`)

## Desired End State

```
crates/djls-python/src/
    blocks.rs              → orchestrator + shared helpers + re-exports
    blocks/
        opaque.rs          — parser.skip_past() detection
        parse_calls.rs     — parser.parse(()) extraction + classification
        dynamic_end.rs     — f-string/format end tag detection
        next_token.rs      — parser.next_token() loop detection
```

The public API is unchanged: `blocks::extract_block_spec(func) -> Option<BlockTagSpec>`.

The orchestrator in `blocks.rs` tries strategies in priority order:

```rust
pub fn extract_block_spec(func: &StmtFunctionDef) -> Option<BlockTagSpec> {
    let parser_var = extract_parser_param(func)?;
    opaque::detect(&func.body, &parser_var)
        .or_else(|| parse_calls::detect(&func.body, &parser_var))
        .or_else(|| dynamic_end::detect(&func.body, &parser_var))
        .or_else(|| next_token::detect(&func.body, &parser_var))
}
```

Each strategy module exposes a `detect(body, parser_var) -> Option<BlockTagSpec>` function.

## What We're NOT Doing

- **BlockEvidence enum**: The design docs propose separating observation from interpretation via a `BlockEvidence` enum. Evaluate during implementation — if the strategies are cleanly separated into modules with clear `detect()` functions, the evidence layer may add unnecessary indirection for no testability gain. Decision documented in M17 implementation notes.
- **Changing strategy logic**: This is a structural refactor. The detection algorithms stay identical.
- **Changing public API**: `extract_block_spec()` signature and return type unchanged.
- **Changing test structure**: No new tests, no deleted tests. All existing tests stay green.

## Implementation Phases

### Phase 1: Create blocks/ directory and move opaque strategy

The simplest and most self-contained strategy.

**Changes required:**
- Create `crates/djls-python/src/blocks/` directory
- Create `crates/djls-python/src/blocks/opaque.rs`:
  - Move `collect_skip_past_tokens()` and `extract_skip_past_token()`
  - Add `pub fn detect(body: &[Stmt], parser_var: &str) -> Option<BlockTagSpec>`
- Update `blocks.rs`: add `mod opaque;` and call `opaque::detect()` in orchestrator
- Move `is_parser_receiver()` to `blocks.rs` as `pub(crate)` shared helper (used by multiple strategies)

**Success criteria:**
- [ ] `cargo build -q` passes
- [ ] `cargo clippy -q --all-targets --all-features -- -D warnings` passes
- [ ] `cargo test -q` passes (745 passed, 0 failed)

### Phase 2: Move dynamic_end strategy

Second simplest — self-contained functions for f-string/format detection.

**Changes required:**
- Create `crates/djls-python/src/blocks/dynamic_end.rs`:
  - Move `has_dynamic_end_in_body()`, `is_dynamic_end_parse_call()`, `is_end_fstring()`, `has_dynamic_end_tag_format()`, `is_end_format_expr()`
  - Add `pub fn detect(body: &[Stmt], parser_var: &str) -> Option<BlockTagSpec>`
- Update `blocks.rs`: add `mod dynamic_end;` and call `dynamic_end::detect()` in orchestrator

**Success criteria:**
- [ ] `cargo build -q` passes
- [ ] `cargo clippy -q --all-targets --all-features -- -D warnings` passes
- [ ] `cargo test -q` passes (745 passed, 0 failed)

### Phase 3: Move next_token strategy

The next-token loop detection for blocktrans/blocktranslate.

**Changes required:**
- Create `crates/djls-python/src/blocks/next_token.rs`:
  - Move `extract_next_token_loop_spec()`, `has_next_token_loop()`, `is_parser_tokens_check()`, `body_has_next_token_call()`, `is_next_token_call()`, `collect_token_content_comparisons()`, `extract_comparisons_from_expr()`
  - Add `pub fn detect(body: &[Stmt], parser_var: &str) -> Option<BlockTagSpec>`
- Update `blocks.rs`: add `mod next_token;`
- Move shared helpers used by next_token (like `is_parser_receiver`) if not already moved

**Success criteria:**
- [ ] `cargo build -q` passes
- [ ] `cargo clippy -q --all-targets --all-features -- -D warnings` passes
- [ ] `cargo test -q` passes (745 passed, 0 failed)

### Phase 4: Move parse_calls strategy

The largest and most complex strategy — parser.parse() extraction and stop-token classification.

**Changes required:**
- Create `crates/djls-python/src/blocks/parse_calls.rs`:
  - Move `ParseCallInfo`, `Classification` types
  - Move `collect_parser_parse_calls()`, `extract_parse_call_info()`, `classify_stop_tokens()`, `classify_in_body()`, `classify_from_if_chain()`, `extract_token_check()`, `extract_startswith_check()`, `is_token_contents_expr()`, `body_has_parse_call()`
  - Add `pub fn detect(body: &[Stmt], parser_var: &str) -> Option<BlockTagSpec>`
- Move `extract_string_sequence()` to `blocks.rs` shared helpers (used by parse_calls and potentially opaque)
- Update `blocks.rs`: add `mod parse_calls;`

**Success criteria:**
- [ ] `cargo build -q` passes
- [ ] `cargo clippy -q --all-targets --all-features -- -D warnings` passes
- [ ] `cargo test -q` passes (745 passed, 0 failed)

### Phase 5: Clean up orchestrator and evaluate BlockEvidence

After all strategies are moved, the orchestrator in `blocks.rs` should be compact.

**Changes required:**
- Review `blocks.rs` — should contain only:
  - `mod` declarations for strategy submodules
  - `pub fn extract_block_spec()` orchestrator
  - Shared helper functions used by multiple strategies
  - `pub(crate)` re-exports if needed
- Evaluate `BlockEvidence` enum: does the module structure already provide clean separation? If yes, skip the enum. If the strategies would benefit from a uniform evidence type (e.g., for logging/tracing what was detected), add it. Document decision.
- Verify `blocks.rs` is under ~100 lines (just orchestrator + shared helpers)

**Success criteria:**
- [ ] `blocks.rs` is the orchestrator only (~50-100 lines, not 1382)
- [ ] Each strategy module is self-contained with a `detect()` entry point
- [ ] `cargo build -q` passes
- [ ] `cargo clippy -q --all-targets --all-features -- -D warnings` passes
- [ ] `cargo test -q` passes (745 passed, 0 failed)
- [ ] No public API change — `blocks::extract_block_spec()` unchanged

## Risks

### Shared helper dependencies between strategies
Multiple strategies use `is_parser_receiver()` and `extract_string_sequence()`. These need to live in the parent `blocks.rs` module or a `blocks/helpers.rs` submodule and be `pub(super)` or `pub(crate)`. Plan to keep them in `blocks.rs` initially and extract to a helpers module only if they grow.

### Test coverage for individual strategies
The existing tests exercise `extract_block_spec()` end-to-end. After the split, individual strategy modules don't get unit-tested in isolation. This is acceptable for a pure structural refactor — the orchestrator tests are sufficient. If strategy logic changes in the future, add per-strategy tests then.

### Module convention
Per AGENTS.md, use `folder.rs` NOT `folder/mod.rs`. So the orchestrator stays as `blocks.rs` and submodules go in `blocks/` directory.
