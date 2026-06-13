# Plan 012: Report startup progress and pin the startup contract with e2e tests

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: Plans 010 and 011 are prerequisites. Verify:
> `with_snapshot` exists in `crates/djls-server/src/server.rs` (010) and the
> refresh task follows the compute → epoch-checked apply → warm-up shape
> (011). Then `git diff --stat 922cc4d7..HEAD -- crates/djls-server/src tests/e2e`
> and content-match the excerpts below.

## Status

- **Priority**: P3
- **Effort**: M
- **Risk**: LOW
- **Depends on**: plans/010, plans/011
- **Category**: dx (startup track, salvaged from PR #626)
- **Planned at**: commit `922cc4d7`, 2026-06-10
- **Execution status**: READY — prerequisites 010 and 011 are merged; this is
  the next startup-track plan

## Why this matters

With plans 010/011 the server is responsive during load — but silently:
users see no signal that Django facts are still loading, and nothing pins
the startup contract against regression. This plan adds the two pieces of
PR #626 that were unambiguously good, at a fraction of the size: (1)
`$/progress` work-done reporting for the load/warm-up phase, with a log
fallback for clients that don't support it (#626 built this as a six-layer
relay — reporter trait, three impls, mpsc dispatcher, state machine; the
budget here is one small helper), and (2) black-box pytest-lsp startup
contract tests (#626's `tests/lsp/test_startup.py` proved: fast handshake,
responsiveness during load, progress begin/end vs. log fallback — main's
e2e suite from #635–643 covers features but not this contract).

## Current state

- Server capability/notification plumbing:
  `crates/djls-server/src/server.rs` — the `tower_lsp_server::Client` is on
  `DjangoLanguageServer` (`server.rs:23`); notifications are sent like
  `self.client.publish_diagnostics(...)` (`server.rs:109-111`).
- Client-capability accessor pattern to mirror:
  `crates/djls-server/src/client.rs:73` —
  `pub(crate) fn supports_pull_diagnostics(&self) -> bool` on `ClientInfo`,
  built from `InitializeParams.capabilities` (`session.rs:76-80`). Work-done
  support lives at `capabilities.window.work_done_progress`.
- Log fallback transport already exists: the `tracing` `LspLayer` routes
  `tracing::info!` to the editor's output panel via `window/logMessage`
  (ARCHITECTURE.md "Observability") — the fallback is just structured log
  lines, no new machinery.
- The reference implementation, verified:
  `reference/ruff/crates/ty_server/src/server/lazy_work_done_progress.rs:44-96`
  — `LazyWorkDoneProgress`: server-initiated token via
  `window/workDoneProgress/create`, **async without blocking** ("it feels
  unfortunate to delay a client request only so that ty can show a progress
  bar"), progress sent only after the create request succeeds (LSP spec
  requirement), **string tokens because Zed does not support numeric tokens**
  (`:78-81`), end-on-drop.
- E2E infrastructure: `tests/e2e/conftest.py:28-57` — pytest-lsp
  `ClientServerConfig` fixtures per editor profile (`emacs_client`,
  `neovim_client`, …) booting `cargo run -p djls -- serve` against
  `tests/project`. Existing files: `test_initialized.py`,
  `test_completions.py`, etc. The matrix runs via `just e2e` (nox session `e2e`; `just test` runs cargo tests only).
- The model for the new tests: PR #626's startup contract suite —
  `jj file show -r startup-rethink tests/lsp/test_startup.py` (158 lines;
  read it before writing Step 3 — it is the spec, adapt names/fixtures to
  main's `tests/e2e` conventions).

## Commands you will need

| Purpose      | Command                          | Expected on success |
|--------------|----------------------------------|---------------------|
| Build        | `cargo build -q`                 | exit 0              |
| Test (all)   | `cargo test -q`                  | exit 0, all pass    |
| Rust matrix  | `just test`                      | exit 0 (cargo via nox; does NOT run tests/e2e) |
| E2E suite    | `just e2e` (or `nox -s e2e`)     | exit 0 (uv-frozen sync + django==5.2 + pytest over tests/e2e — noxfile.py:112-129) |
| E2E (direct) | `uv run pytest tests/e2e/test_startup.py -x` | all pass (after `uv sync`; mirror the nox session env) |
| Lint         | `just clippy`                    | exit 0, no warnings |
| Format       | `just fmt`                       | exit 0              |
| Hooks        | `just lint`                      | exit 0              |

## Scope

**In scope** (the only files you should modify/create):
- `crates/djls-server/src/progress.rs` (create — the one helper)
- `crates/djls-server/src/server.rs`, `src/client.rs`, `src/lib.rs` (wiring)
- `tests/e2e/test_startup.py` (create)
- `tests/e2e/conftest.py` (only if a capabilities-variant fixture is needed)

**Out of scope** (do NOT touch, even though they look related):
- Reporter traits, channels, dispatcher tasks, progress state machines —
  the #626 anti-model. One struct, direct `Client` calls.
- Progress for individual requests (completion etc.) — load/warm-up only.
- `window/workDoneProgress/cancel` handling — tower-lsp-server 0.23 has no
  callback for it; explicitly deferred (set `cancellable: false`).
- The refresh/warm-up logic itself (plan 011's shape stays).

## Git workflow

jj repo — no mutating `git`. Two commits suggested:
`jj commit -m "add work-done progress for project loading"`, then
`jj commit -m "test: add e2e startup contract coverage"`. Do NOT push.

## Steps

### Step 1: Capability accessor

In `client.rs`, add `supports_work_done_progress(&self) -> bool` reading
`capabilities.window.as_ref().and_then(|w| w.work_done_progress).unwrap_or(false)`,
following the `supports_pull_diagnostics` pattern (`client.rs:73`).

**Verify**: `cargo build -q` → exit 0.

### Step 2: The progress helper

New `crates/djls-server/src/progress.rs`, modeled on ty's
`LazyWorkDoneProgress` (cited above) but smaller — we are always
server-initiated (no request token exists for `initialized`):

```rust
pub(crate) struct LoadProgress { /* client handle + Option<token> + title */ }

impl LoadProgress {
    /// Requests a server-initiated token (string token — Zed compat) via
    /// window/workDoneProgress/create. On failure or when the client lacks
    /// the capability, returns a logger-only instance: report/finish become
    /// tracing::info! lines (already routed to the editor via LspLayer).
    pub(crate) async fn begin(client: &Client, info: &ClientInfo, title: &str) -> Self { ... }
    pub(crate) async fn report(&self, message: &str) { ... }
    pub(crate) async fn finish(self, message: &str) { ... }  // sends End
}
```

Spec constraints (from the LSP spec, via ty's doc comment): only send
`$/progress` with the token if the create request returned success; send
`Begin` exactly once, `End` exactly once (`finish` consumes self; also send
`End` on drop if `finish` was skipped — match ty's end-on-drop). Use
`tower_lsp_server::Client`'s request-sending surface for
`window/workDoneProgress/create` (find the method with
`rg -n "send_request|work_done" ~/.cargo` against the vendored
tower-lsp-server, or its docs.rs — record what you used).

Wire into plan 011's task: `begin` before compute ("Loading Django
project"), `report` between phases ("Resolving environment", "Extracting
settings", "Warming caches"), `finish` after the diagnostics republish.
A superseded (epoch-stale) task still calls `finish` ("superseded") — never
leak an open progress token.

**Verify**: `cargo build -q` → exit 0; `cargo test -q -p djls-server` →
all pass; manual smoke optional (run an editor or skip — e2e covers it).

### Step 3: Startup contract e2e tests

Read `jj file show -r startup-rethink tests/lsp/test_startup.py` first.
Create `tests/e2e/test_startup.py` following `tests/e2e` conventions
(fixtures from `conftest.py`), covering:

1. **Handshake is protocol-only**: `initialize` returns capabilities without
   the server having loaded project facts (assert the response arrives and
   contains the expected capability set — reuse assertions from
   `test_initialized.py` as the pattern).
2. **Responsive during load**: immediately after `initialized`, send a
   completion (or hover) request and assert it returns successfully —
   possibly with degraded results, but within the suite's normal timeout
   and without error.
3. **Progress sequence**: with a client-capabilities profile that advertises
   `window.workDoneProgress` (pytest-lsp's `client_capabilities("vscode")`
   does; verify with `python -c` introspection), assert the client observed
   `window/workDoneProgress/create` and a Begin → (Reports) → End sequence.
   pytest-lsp's `LanguageClient` records progress params — check its API
   (`lsp_client.progress_reports` or equivalent; consult the version pinned
   in this repo's dev dependencies) and #626's test file for the exact
   idiom.
4. **Log fallback**: with a profile lacking the capability (check which of
   emacs/neovim profiles lacks it; if all advertise it, add a conftest
   fixture with stripped `window.work_done_progress`), assert no
   `$/progress` notifications arrive and a "Loading Django project" log
   message does (pytest-lsp records `window/logMessage`).

**Verify**: the pytest invocation for just this file passes locally; then
`just e2e` → full e2e suite green.

### Step 4: Full validation

**Verify**: `cargo test -q`, `just test`, `just e2e`, `just clippy`, `just fmt`,
`just lint` → all exit 0.

## Test plan

Step 3 IS the test plan (the deliverable is partly tests). Rust-side: one
unit test that `LoadProgress::begin` with a capability-less `ClientInfo`
produces a logger-only instance (no panic, no notification attempts — make
the client handle injectable or test through the e2e layer only and say so).

## Done criteria

Machine-checkable. ALL must hold:

- [ ] `tests/e2e/test_startup.py` exists with ≥ 4 tests, all passing under `just e2e`
- [ ] `rg -c "struct|enum" crates/djls-server/src/progress.rs` ≤ 3 (size budget — no relay apparatus)
- [ ] Begin/End pairing enforced (end-on-drop test or code-review evidence in the diff)
- [ ] `cargo test -q` exits 0
- [ ] `just test` exits 0 and `just e2e` exits 0
- [ ] `just clippy` exits 0
- [ ] Only in-scope files modified (`jj diff --stat`)
- [ ] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- `tower_lsp_server::Client` (v0.23) exposes no way to send the
  `window/workDoneProgress/create` *request* (it may only support
  notifications) — report the actual API surface; the fallback design
  (log-only everywhere) is a product decision, not yours.
- pytest-lsp's pinned version doesn't record progress/log notifications in
  an assertable way — report the API gap before hand-rolling a capture
  shim.
- Test 2 (responsive during load) is flaky because `tests/project` loads
  too fast to observe the during-load window — do not add sleeps to the
  server; report and consider asserting only the success property, noting
  the timing property as untestable at this fixture size.

## Maintenance notes

- When file watching or heavier warm-ups arrive, `report()` call sites are
  the only thing to extend — the helper itself should not grow states.
- `window/workDoneProgress/cancel` support is deferred on a tower-lsp-server
  limitation; revisit if the dependency gains the callback (PR #626 hit the
  same wall and set `cancellable: false`).
- These e2e tests are the durable artifact — they pin the startup contract
  for every future refactor. Reviewers should treat changes that weaken
  them (longer timeouts, removed assertions) as regressions, not test
  maintenance.
