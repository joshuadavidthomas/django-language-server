# Plan 011: Make project refresh non-blocking with an epoch guard

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat 922cc4d7..HEAD -- crates/djls-server/src crates/djls-semantic/src/project/sync.rs`
> Plans 008/009/010 are prerequisites and HAVE reshaped these files (phase-1
> cache gone, inspector gone, snapshot reads in place). Verify each
> prerequisite in the README status table is DONE; then content-match the
> excerpts below against live code. The `refresh_external_data` you find
> should be down to source-roots refresh + revision bumps — if it still
> queries a subprocess, the static track hasn't landed: STOP.
> If plan 015 (djls-project crate) landed first, `sync.rs` lives at
> `crates/djls-project/src/sync.rs` — adjust paths accordingly; the shape
> is the same.

## Status

- **Priority**: P2
- **Effort**: M
- **Risk**: MED
- **Depends on**: plans/009, plans/010
- **Category**: perf (startup track, salvaged from PR #626)
- **Planned at**: commit `922cc4d7`, 2026-06-10
- **Execution status**: PR #675 open at `0d912c41`; not merged

## Execution record — local source commit (2026-06-12)

Implemented as one source commit:

1. `0d912c41` — `refactor: apply project refresh briefly and warm queries off-lock`

Implementation notes:

- Added `DjangoLanguageServer::refresh_epoch: Arc<AtomicU64>` and queued
  refresh tasks that stale-check before locking, after locking, before apply,
  after apply, and between warm-up queries.
- Split project refresh into `compute_refresh` and `apply_refresh` in
  `djls-project`. `compute_refresh` returns `RefreshData` containing new
  search paths plus the current file paths whose revisions should be bumped;
  `apply_refresh` only registers roots, updates the `Project.search_paths`
  input when changed, bumps registered roots, and bumps precomputed files.
- Moved the hidden `project.refresh_source_roots` call out of
  `DjangoDatabase::set_settings` so config reloads do not perform filesystem
  search-path probing while the session lock is held. `SettingsUpdate` now
  reports `semantic_changed` for tagspec-only changes so open-document
  diagnostics republish when tag specs change.
- `initialized` now submits refresh work and returns immediately; the existing
  `"Server initialization completed"` log line is emitted by the queued task
  after refresh/warm-up completes.
- `did_change_configuration` no longer awaits the refresh task. Env changes
  enqueue the refresh; diagnostics-only and semantic-only changes bump the
  refresh epoch and republish open-document diagnostics directly.
- Warm-up primes `tag_specs`, `template_dirs`, `template_libraries`, and
  `project_template_files` from a `SessionSnapshot`, with `salsa::Cancelled`
  caught and treated as superseded work.
- Refresh-originated diagnostics republish uses the same publish mutex as
  direct diagnostics pushes and rechecks the epoch while holding that mutex,
  preventing stale refresh diagnostics from overwriting a newer config publish.
- Removed the now-dead `Project::refresh_source_roots`; its body had been
  absorbed by `compute_refresh`/`apply_refresh`.
- Inlined the single-use `refresh_file_paths` and `bump_python_modules`
  helpers back into their owning compute/apply functions during PR review.

Divergences recorded:

- Plan 015/021 moved project sync from `crates/djls-semantic/src/project/sync.rs`
  to `crates/djls-project/src/sync.rs`; Plan 011 was applied at the final
  `djls-project` location.
- Source scope expanded to `crates/djls-db/src/settings.rs` and
  `crates/djls-db/src/db.rs` because `set_settings` still refreshed source
  roots under the session lock and because tagspec-only settings changes had
  no republish signal. This was required to satisfy the lock-scope invariant.
- Source scope expanded to `crates/djls-project/tests/resolve.rs` for direct
  compute/apply split coverage.
- The planned small outcome type became `RefreshData`; no stage enum,
  controller, second counter, queue change, progress relay, or file-watching
  machinery was introduced.
- A `diagnostic_publish_lock` was added after concurrency review found that an
  async `publish_diagnostics` call could otherwise let a stale refresh publish
  resume after a newer config publish.

Validation passed on the final source commit:

- `cargo build -q`
- `cargo test -q -p djls-server`
- `cargo test -q -p djls-project`
- `cargo test -q -p djls-db tagspecs_settings_change_reports_semantic_change`
- `cargo test -q`
- `just test`
- `just e2e`
- `cargo clippy --all-targets --all-features --benches -- -D warnings`
- clean-tree `just clippy`
- clean-tree `just fmt`
- clean-tree `just lint`
- review cleanup: `rg "refresh_source_roots" crates/djls-project/src crates/djls-db/src crates/djls-server/src -g '*.rs'` returns no matches
- review cleanup: `cargo test -q -p djls-project`, `cargo build -q`, `cargo clippy --all-targets --all-features --benches -- -D warnings`, and clean-tree `just clippy` exit 0

Review notes:

- Initial e2e run stalled on the first completion test because the first split
  left `refresh_python_modules` doing discovery under the session lock. The
  fix hoisted discovery to `compute_refresh`; `test_completes_available_template_tags`
  then passed in 8.07s and full `just e2e` passed afterwards.
- Two strict concurrency reviews were run. The first found stale refresh
  diagnostics could overwrite diagnostics-only config publishes and that
  tagspec-only changes had no republish signal; both were fixed. The second
  found a remaining async publish-order race; `diagnostic_publish_lock` fixed
  it. Final strict concurrency review reported no must-fix findings.

**Review verdict (2026-06-12): approved.** Independently re-verified on source
commit `0d912c41` (bookmark `plan-011-nonblocking-refresh`, parented on the
merged plan-010 main): `cargo test -q`, `just clippy`, `just fmt --check`,
`just e2e` (27 passed), and `just lint` all exit 0. Lock-scope invariant
checked by reading, not trusting names: the compute lock holds only a db
clone; the apply lock holds `apply_refresh` (register_roots is pure over
precomputed paths plus a side-table `replace_roots` — no fs walking, so the
named STOP condition does not trigger), the setter compare, revision bumps,
and the snapshot/documents capture; compute, warm-up, and diagnostics
collection all run on `spawn_blocking` against clones/snapshots inside
`Cancelled::catch`. Done-criteria sweeps confirm no `refresh_external_data`
or `rx.await` remains in `server.rs`; new types are exactly one
(`RefreshData`); no stage enum, second counter, or relay — the #626 slope was
avoided. Divergences ratified: the djls-db scope expansion (required — leaving
`refresh_source_roots` inside `set_settings` would have re-probed the fs under
the session lock, defeating the plan), `semantic_changed` on `SettingsUpdate`
(tagspec-only changes previously had no republish signal), the
`diagnostic_publish_lock` (closes a real publish-order race the epoch alone
cannot, since publishes are async sends), and applying at the post-015/021
`djls-project` location per the drift note. Review follow-up: the dead
`Project::refresh_source_roots` method was deleted before PR. One residual
noted, not a blocker: pull-diagnostics clients are skipped by the refresh
republish and the server sends no `workspace/diagnostic/refresh`, so a pull
client sees post-warm-up facts only on its next request — same family as plan
010's recorded residual. One ordering nuance verified safe: `compute_refresh`
gathers bump paths against pre-refresh roots, but the root-revision bumps in
apply invalidate discovery queries, and the new
`compute_and_apply_refresh_discovers_site_packages_created_after_bootstrap`
test covers the new-roots case directly. PR #675 is open.

## Why this matters

`initialized` and `did_change_configuration` currently run the whole project
refresh while holding the session lock (`server.rs:215-233`, `:530-553`) —
on a current-thread runtime, the editor's requests queue behind it. After
the static track (plans 006–009), "refresh" is cheap bookkeeping (search-path
probing + revision bumps) but the **first queries after it** — settings
extraction, library derivation, tag specs — do real work, and with plan
010's snapshot reads, the first unlucky completion request pays that cost.
The fix is the pattern PR #626 got right and over-built (a 1,936-line
controller): apply input writes briefly under the lock, then **warm the
derived queries on a background snapshot**, guarded by a single generation
counter so a config change supersedes an in-flight warm-up. rust-analyzer's
rule, which this implements at minimum size: I/O produces values; values are
applied to the db in one revision; queries only read inputs — and warming is
just running queries on a snapshot, cancellation-safe by construction.

## Current state

(Excerpts from `922cc4d7`; content-match after prerequisite churn.)

- `crates/djls-server/src/server.rs:215-233` — phase 2 of `initialized`
  holds the lock for the entire refresh:

  ```rust
  let rx = self
      .with_session_mut_task(|session| async move {
          ...
          let mut session_lock = session.lock().await;
          let db = session_lock.db_mut();
          ...
          refresh_external_data(db);
  ```

- `crates/djls-server/src/server.rs:530-553` — `did_change_configuration`
  repeats the shape and then **blocks on the refresh** (`let _ = rx.await;`)
  before republishing diagnostics.

- `crates/djls-server/src/queue.rs` — a sequential mpsc worker
  (`Queue::submit`, queue.rs:153) executes background closures one at a
  time. It stays; this plan changes what the closures do, not the queue.

- `crates/djls-semantic/src/project/sync.rs:37-47` (post-009 shape) —
  `refresh_external_data` = `refresh_source_roots` (re-probe search paths +
  `replace_roots` + setter) and `refresh_python_modules` (root-revision and
  file-revision bumps, sync.rs:301-335). The probing part
  (`SearchPaths::from_project_settings`, `resolve.rs:87-117`) is pure over
  `&dyn FileSystem` — computable without the db.

- Plan 010 provides `Session::snapshot()` + `with_snapshot` with
  `salsa::Cancelled` handling, and `maybe_push_diagnostics` reads from
  snapshots.

- PR #626's generation-guard concept (worth keeping; its apparatus is not):
  mark supersession *before* waiting for the lock, double-check under the
  lock, drop stale results on the floor.

## Commands you will need

| Purpose      | Command                          | Expected on success |
|--------------|----------------------------------|---------------------|
| Build        | `cargo build -q`                 | exit 0              |
| Test (crate) | `cargo test -q -p djls-server`   | exit 0, all pass    |
| Test (all)   | `cargo test -q`                  | exit 0, all pass    |
| Rust matrix  | `just test`                      | exit 0 (cargo via nox; does NOT run tests/e2e) |
| E2E suite    | `just e2e`                       | exit 0              |
| Lint         | `just clippy`                    | exit 0, no warnings |
| Format       | `just fmt`                       | exit 0              |
| Hooks        | `just lint`                      | exit 0              |

## Scope

**In scope** (the only files you should modify):
- `crates/djls-server/src/server.rs`
- `crates/djls-server/src/session.rs` (if the epoch lives on Session)
- `crates/djls-semantic/src/project/sync.rs` (split compute/apply if needed)

**Out of scope** (do NOT touch, even though they look related):
- `queue.rs` — keep the sequential worker as-is.
- Progress reporting — plan 012 wraps this plan's warm-up with `$/progress`.
- Any new state machine: no `DiscoveryStage` enums, no milestone tables, no
  readiness partitions. PR #626's `discovery_run.rs`/`startup.rs` are the
  anti-model; the budget for new types in this plan is: one `AtomicU64`,
  one small `RefreshOutcome` (if even needed).
- File watching.

## Git workflow

jj repo — no mutating `git`. Finish with:
`jj commit -m "refactor: apply project refresh briefly and warm queries off-lock"`.
Do NOT push.

## Steps

### Step 1: Add the refresh epoch

On `DjangoLanguageServer` (server.rs:22-27): `refresh_epoch: Arc<AtomicU64>`.
Bump it (`fetch_add`) at the top of `did_change_configuration` when
`settings_update.env_changed`, and once in `initialized`. Every refresh task
captures the epoch at submission; before *and* after acquiring the session
lock, and between warm-up queries, it compares against the current value and
returns early when stale. That's the entire guard — PR #626's
`GenerationGuard` semantics in ~10 lines.

**Verify**: `cargo build -q` → exit 0.

### Step 2: Split refresh into compute / apply

In `sync.rs`, restructure (keep the public name or rename — your call, the
README's maintenance note already flags the rename):

- `compute_refresh(fs, root, interpreter, pythonpath) -> SearchPaths` — the
  pure probing, callable without `&mut` db (it is already:
  `SearchPaths::from_project_settings`).
- `apply_refresh(db: &mut dyn ProjectDb, search_paths) ` — `register_roots`,
  the setter compare, and the revision bumps from `refresh_python_modules`.
  This is the only part that needs the lock, and it is O(inputs), no I/O
  beyond what bumps require.

In the server task: brief lock → clone the inputs (root, interpreter,
pythonpath) → unlock → `compute_refresh` (in `spawn_blocking`) → epoch check
→ brief lock → epoch re-check → `apply_refresh` → unlock.

**Verify**: `cargo test -q -p djls-semantic` → all pass (existing sync tests
reshaped, behavior identical).

### Step 3: Warm-up on a snapshot

After `apply_refresh`, in the same queued task: take a snapshot
(plan 010's), then on the blocking pool prime the hot derived queries —
`db.tag_specs()`, the template-dirs query, the template-libraries query, and
`project_template_files` — inside the `Cancelled` catch (a cancelled warm-up
is *correct*: it means newer inputs arrived; the next refresh re-warms).
Check the epoch between queries; bail silently when superseded.

Then republish diagnostics for open documents (the existing
`maybe_push_diagnostics` loop from `did_change_configuration:555-561`) —
also for the `initialized` path, which today never republishes after load
completes (documents opened before facts arrived keep stale diagnostics
until the next edit; this fixes that).

**Verify**: `cargo test -q` → all pass.

### Step 4: Rewire `initialized` and `did_change_configuration`

- `initialized`: submit the Step 2+3 task; do not block the handler on it
  (post-008 there is no cache phase; the handler returns immediately).
  **Preserve the log line** `"Server initialization completed"` — emit it
  when the warm-up task finishes; `tests/e2e/test_initialized.py:23-35`
  polls for it and hangs to timeout without it.
- `did_change_configuration`: settings update under brief lock (unchanged),
  bump epoch, submit the task, **remove** the `let _ = rx.await;` block —
  diagnostics republish moves inside the task (Step 3), so the handler no
  longer serializes on the refresh.

**Verify**: `just e2e` → e2e passes: `test_initialized.py` and the
diagnostics tests still hold (diagnostics arrive after load via the
republish; pull-diagnostics clients re-request).

### Step 5: Full validation

**Verify**: `cargo test -q`, `just test`, `just e2e`, `just clippy`, `just fmt`,
`just lint` → all exit 0.

## Test plan

- Unit test for the epoch guard: submit refresh A, bump the epoch, run A's
  task body; assert it applied nothing (a counter or the absence of setter
  effects on the db).
- Unit test for compute/apply split: `compute_refresh` over an
  `InMemoryFileSystem` returns the same `SearchPaths` the old monolithic
  refresh produced (fixture comparison).
- E2E: existing `tests/e2e/test_initialized.py` + diagnostics tests are the
  regression net. Plan 012 adds the startup-contract tests proper
  (responsiveness during load) — don't duplicate them here.

## Done criteria

Machine-checkable. ALL must hold:

- [x] `rg -n "refresh_external_data" crates/djls-server/src/server.rs` shows no server-side call remains; lock scope is clone inputs/snapshot/documents plus apply only
- [x] `rg -n "rx\.await" crates/djls-server/src/server.rs` shows `did_change_configuration` no longer blocking on the refresh task
- [x] `initialized` republishes diagnostics for open documents after warm-up via `republish_snapshot_diagnostics`; `just e2e` diagnostics tests pass against the non-blocking path
- [x] New types introduced by this plan: ≤ 2 (`RefreshData`; the epoch and publish lock use existing library types)
- [x] `cargo test -q` exits 0
- [x] `just test` exits 0 and `just e2e` exits 0
- [x] `just clippy` exits 0
- [x] Source diff is limited to intended Plan 011 code plus recorded drift/scope expansions: `djls-project` sync, `djls-server` server, `djls-db` settings/test coverage, and `djls-project` refresh split coverage
- [x] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- `refresh_external_data` still contains subprocess or cache work (plans
  008/009 not actually landed).
- The apply step turns out to need I/O you can't hoist into compute
  (e.g. `register_roots` walking the fs) — report what and why rather than
  holding the lock longer.
- You feel the need for a second counter, a stage enum, or a status relay —
  that is the #626 slope; one epoch is the design.
- E2E diagnostics tests fail because republish-after-load races
  pull-diagnostics clients — report the race, don't sleep() around it.

## Maintenance notes

- This plan deliberately keeps the sequential `Queue` — one refresh at a
  time, newest wins via the epoch. If refreshes ever become frequent
  (file watching), revisit with debouncing at the submission site, not with
  a smarter queue.
- Plan 012 wraps the Step 3 warm-up in `$/progress` reporting — the warm-up
  function should take an optional progress callback or be trivially
  wrappable; keep its structure flat.
- Reviewers: check lock scopes by reading, not trusting names — the entire
  point is that no `.lock()` guard lives across a compute or I/O await.
