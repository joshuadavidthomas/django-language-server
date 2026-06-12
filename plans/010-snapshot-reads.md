# Plan 010: Serve read requests from session snapshots instead of holding the session lock

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat 922cc4d7..HEAD -- crates/djls-server/src`
> Plans 001–009 may have landed and changed `server.rs` (phase-1 cache
> removal in 008). Compare the "Current state" excerpts against live code;
> structural mismatches beyond the documented prerequisite changes are a
> STOP condition.

## Status

- **Priority**: P2
- **Effort**: M
- **Risk**: MED
- **Depends on**: plans/003 (stable project handle); independent of the static track otherwise
- **Category**: perf / dx (startup track, salvaged from PR #626)
- **Planned at**: commit `922cc4d7`, 2026-06-10
- **Execution status**: source-complete locally at `8d22896d`; not pushed/merged

## Execution record — local source stack (2026-06-12)

Implemented as one source commit:

1. `8d22896d` — `refactor: serve read requests from session snapshots`

Implementation notes:

- Promoted `SessionSnapshot` and `Session::snapshot()` out of test-only code.
- Moved document request resolution helpers onto `SessionSnapshot`, with
  `Session` retaining delegating methods for the old call surface.
- Added `with_snapshot`, which captures a snapshot under the session lock,
  runs request work through `tokio::task::spawn_blocking`, catches
  `salsa::Cancelled`, retries twice with fresh snapshots, then returns the
  response type's default fallback.
- Converted `maybe_push_diagnostics`, `completion`, `hover`, `diagnostic`,
  `folding_range`, `document_symbol`, `goto_definition`, `references`, and
  `formatting` to `with_snapshot`. `formatting` was included because it is a
  read-only feature handler and the done criteria require no read-only feature
  handlers to keep using `with_session`.
- Added server tests for plain snapshot task reads and conversion of a manual
  `salsa::Cancelled::PendingWrite` unwind into a caught cancellation result.

Divergences recorded:

- `crates/djls-server/Cargo.toml` entered source scope after the plan's STOP
  check found that `djls-server` only had `salsa` as a dev-dependency. The
  production `server.rs` cancellation path needs `salsa::Cancelled`, so the
  dependency moved from `[dev-dependencies]` to `[dependencies]` with Josh's
  approval.
- The cancellation fallback is represented by `R: Default` on `with_snapshot`
  rather than an explicit fallback argument. Current handlers return `Option`
  or `Vec` shapes where `Default` is the planned fallback (`None` or empty
  vector).
- The test plan's full concurrent setter/read orchestration was not added;
  the accepted proxy test path was used instead: a plain snapshot read plus a
  manually raised `salsa::Cancelled` unwind through the same catch wrapper.
- `Session::file_for_document_request` and
  `Session::position_for_document_request` are retained as delegating methods
  per the plan, but are currently unused after the read-handler conversion.
  They carry local `#[allow(dead_code)]` attributes so `just clippy`'s
  `-D warnings` gate stays green without deleting the planned compatibility
  surface inside the crate.

Validation passed on the final source commit:

- `cargo test -q -p djls-server`
- `cargo build -q`
- `cargo test -q`
- `just test`
- `just clippy`
- `just fmt`
- `just lint`

**Review verdict (2026-06-12): approved.** Independently re-verified on
source commit `8d22896d`: `cargo test -q`, `just clippy`, `just fmt --check`,
and `just e2e` (27 passed) all exit 0; the done-criteria sweeps confirm 10
`with_snapshot` call sites, the only remaining `with_session` read is the
`Session::open_documents` accessor (explicitly carved out — the documents map
lives on `Session`, not the snapshot), and `cfg(test)` in `session.rs` gates
only test imports/helpers/modules. The vendored salsa exposes
`Cancelled::catch` and `Cancelled::PendingWrite` exactly as the implementation
uses them, so the preferred Step 3 shape was available and taken. Closures in
every converted handler touch only the snapshot — no path re-locks the
session, so the deadlock STOP condition cannot trigger. Divergences ratified:
the `Cargo.toml` salsa dependency-scope move (pre-approved), `R: Default` as
the fallback mechanism, `Fn` + `Arc` instead of `FnOnce` (forced by the retry
policy, which must re-invoke the closure against a fresh snapshot — the plan's
`FnOnce` sketch and its retry policy were mutually incompatible; the
implementation resolved it the right way), `formatting` included in the
conversion per the done criteria, the proxy cancellation test path (both
tests verified meaningful: a plain read and a manual
`Cancelled::PendingWrite` unwind through `run_snapshot_task`), and the
`#[allow(dead_code)]` retention of the delegating `Session` methods per the
plan's explicit instruction. One residual noted, not a blocker: a
triple-cancelled pull-diagnostics request falls back to an empty `Vec`, which
a client may read as "no diagnostics" — this is the plan's own stated policy,
and the maintenance note already names `ContentModified` as the upgrade path
if it ever matters in practice. Remaining: push and PR when Josh says go.

## Why this matters

Every LSP request currently locks the whole `Session` for its full duration
(`with_session`), and the server runs a **current-thread** tokio runtime
(`crates/djls-server/src/lib.rs:32`), so one slow computation freezes the
entire event loop — including `didChange` handling and shutdown. The fix is
the rust-analyzer/ty shape: requests briefly lock to capture an immutable
snapshot (a cheap database clone + client info), release the lock, and
compute on a blocking thread. ty's `SessionSnapshot`/`DocumentSnapshot` and
rust-analyzer's `GlobalStateSnapshot` are this exact pattern; this repo
already has the type — `SessionSnapshot` — but locked behind `#[cfg(test)]`.
This plan is the structural precondition for the rest of the startup track
(plans 011–012): background loading can only stop blocking requests once
requests stop needing the lock.

## Current state

- `crates/djls-server/src/session.rs:251-272` — the existing test-only type:

  ```rust
  /// Immutable snapshot of session state for tests.
  #[cfg(test)]
  #[derive(Clone)]
  struct SessionSnapshot {
      db: DjangoDatabase,
      client_info: ClientInfo,
  }
  ```

  with `Session::snapshot()` at `session.rs:89-92`, also `#[cfg(test)]`.

- `crates/djls-server/src/server.rs:40-54` — the lock helpers:

  ```rust
  pub(crate) async fn with_session<F, R>(&self, f: F) -> R
  where F: FnOnce(&Session) -> R,
  { let session = self.session.lock().await; f(&session) }
  ```

- A representative read handler, `server.rs:285-313` (`completion`): resolves
  `(file, offset)` via `session.position_for_document_request(...)`, checks
  `FileKind::Template`, then runs `djls_ide::completion(db, ...)` — all
  inside `with_session`. `hover` (:315-334), `diagnostic` (:336-368),
  `folding_range` (:370), `document_symbol` (:394), `goto_definition`
  (:418), `references` (:442) follow the same shape.

- `maybe_push_diagnostics` (`server.rs:88-118`) computes diagnostics under
  the lock too.

- `DjangoDatabase` is `Clone` and `Send` (marker test
  `crates/djls-db/src/db.rs:197-201`; it is deliberately `!Sync`).
  Cheap-clone concurrent reads are established practice: ARCHITECTURE.md
  documents `SessionSnapshot` as "idea borrowed from Ruff/ty", and `djls
  check` already runs parallel work via `db.clone()` per rayon task.

- `session.file_for_document_request` / `position_for_document_request`
  (`session.rs:199-233`) call `db.get_or_create_file(&path)` — this works on
  `&DjangoDatabase` and registers files in a side table shared across
  clones, so it can run during the brief lock (keep it there — conservative).

- There is **no** `salsa::Cancelled` handling anywhere in the repo today
  (`rg "Cancelled" crates/` → no matches): nothing currently reads
  concurrently with writes. This plan introduces the first such concurrency,
  so it must also introduce cancellation handling (Step 3). The reference
  pattern is ty's: background reads unwind with `salsa::Cancelled` when a
  mutation cancels them, and the request layer retries
  (`reference/ruff/crates/ty_server` retries cancelled requests by
  re-queueing).

## Commands you will need

| Purpose      | Command                          | Expected on success |
|--------------|----------------------------------|---------------------|
| Build        | `cargo build -q`                 | exit 0              |
| Test (crate) | `cargo test -q -p djls-server`   | exit 0, all pass    |
| Test (all)   | `cargo test -q`                  | exit 0, all pass    |
| E2E matrix   | `just test`                      | exit 0              |
| Lint         | `just clippy`                    | exit 0, no warnings |
| Format       | `just fmt`                       | exit 0              |
| Hooks        | `just lint`                      | exit 0              |

## Scope

**In scope** (the only files you should modify):
- `crates/djls-server/src/session.rs`
- `crates/djls-server/src/server.rs`

**Out of scope** (do NOT touch, even though they look related):
- `did_open` / `did_change` / `did_close` / `did_save` /
  `did_change_configuration` — write paths stay on `with_session_mut`.
- `crates/djls-server/src/queue.rs` and the background refresh — plan 011.
- Progress reporting — plan 012.
- `djls-ide` / `djls-semantic` — no feature-layer changes; this is pure
  server plumbing.
- Converting the runtime to multi-thread — not needed; `spawn_blocking`
  provides the off-loop execution.

## Git workflow

jj repo — no mutating `git`. Finish with:
`jj commit -m "refactor: serve read requests from session snapshots"`.
Do NOT push.

## Steps

### Step 1: Promote `SessionSnapshot`

In `session.rs`: remove `#[cfg(test)]` from `SessionSnapshot`, its impl, and
`Session::snapshot()`; make them `pub(crate)`. Give the snapshot the
read-side helpers handlers need — move (not duplicate) the resolution logic:

```rust
pub(crate) struct SessionSnapshot { db: DjangoDatabase, client_info: ClientInfo }

impl SessionSnapshot {
    pub(crate) fn db(&self) -> &DjangoDatabase { ... }
    pub(crate) fn client_info(&self) -> &ClientInfo { ... }
}
```

`file_for_document_request` / `position_for_document_request` read only
`self.db` and `self.client_info` (`session.rs:199-233`) — re-home them onto
`SessionSnapshot` and have `Session` delegate (Session keeps its methods so
write paths and tests don't churn).

**Verify**: `cargo test -q -p djls-server` → all pass.

### Step 2: Add the snapshot-request helper

In `server.rs`, alongside `with_session`:

```rust
/// Capture a snapshot under a brief lock, then compute on the blocking
/// pool so the single-threaded event loop stays responsive.
async fn with_snapshot<F, R>(&self, f: F) -> R
where
    F: FnOnce(&SessionSnapshot) -> R + Send + 'static,
    R: Send + 'static,
{
    let snapshot = { self.session.lock().await.snapshot() };
    tokio::task::spawn_blocking(move || f(&snapshot))
        .await
        .expect("snapshot task must not panic")  // see Step 3 — Cancelled is caught inside
}
```

**Verify**: `cargo build -q` → exit 0.

### Step 3: Cancellation handling

Determine the vendored salsa's cancellation API first:
`rg -n "pub fn catch|struct Cancelled" $(find ~/.cargo -name "*.rs" -path "*salsa*" 2>/dev/null | head -1 | xargs dirname 2>/dev/null) 2>/dev/null` — or more reliably,
check `Cargo.lock` for the salsa source and read its `Cancelled` type. Two
acceptable shapes:

- `salsa::Cancelled::catch(|| ...) -> Result<R, Cancelled>` (preferred if it
  exists), or
- `std::panic::catch_unwind(AssertUnwindSafe(|| ...))` + downcast the payload
  to `salsa::Cancelled`, resuming the unwind for any other panic.

Wrap the closure execution inside `with_snapshot` with it. Policy on
cancellation: retry against a **fresh** snapshot (the write that cancelled
us has completed once we can re-lock), up to 2 retries, then return the
fallback the handler provides (`None` / empty vec). Log at debug level.
This mirrors ty's retry-on-`Cancelled` request handling, simplified to an
inline loop.

**Verify**: `cargo build -q` → exit 0; add the unit test from the Test plan.

### Step 4: Convert the read handlers

Convert `completion`, `hover`, `diagnostic`, `folding_range`,
`document_symbol`, `goto_definition`, `references`, and
`maybe_push_diagnostics` from `with_session` to `with_snapshot`. The body is
unchanged except `session.` → `snapshot.` (the helpers moved in Step 1).
Example for `completion` (current code at `server.rs:285-313`):

```rust
let response = self
    .with_snapshot(move |snapshot| {
        let (file, offset) = snapshot.position_for_document_request(
            &params.text_document_position.text_document,
            params.text_document_position.position,
            "completion",
        )?;
        let db = snapshot.db();
        if *file.source(db).kind() != FileKind::Template { return None; }
        djls_ide::completion(db, file, offset,
            snapshot.client_info().position_encoding(),
            snapshot.client_info().supports_snippets())
    })
    .await;
```

Note `params` moves into the closure (`Send + 'static` bound). Leave
`with_session` in place — write paths and any handler that genuinely needs
`&Session` still use it.

**Verify**: `cargo test -q` → all pass; `just test` → e2e suite passes (the
behavioral contract: identical responses, now computed off-lock).

### Step 5: Full validation

**Verify**: `just clippy`, `just fmt`, `just lint` → exit 0.

## Test plan

- New unit test in `server.rs` or `session.rs` tests: spawn a snapshot
  compute that loops reading a query while the main thread performs a
  setter write (`db_mut` + `set_*`); assert the snapshot path returns the
  fallback or retried value rather than panicking the process. (If
  orchestrating this is impractical in-crate, a simpler proxy: assert
  `with_snapshot` returns correct results for a plain read, and that the
  catch wrapper converts a manually raised `salsa::Cancelled` unwind into a
  retry — construct via the same mechanism salsa uses if its API allows.)
- Existing e2e (`just test`) is the main behavioral gate: all feature
  responses unchanged.
- `test_snapshot_creation` (session.rs:350-362) keeps passing — now against
  the promoted type.

## Done criteria

Machine-checkable. ALL must hold:

- [x] `rg -n "cfg\(test\)" crates/djls-server/src/session.rs` shows `SessionSnapshot` no longer gated (`cfg(test)` remains only for test imports/helpers/modules)
- [x] `rg -c "with_snapshot" crates/djls-server/src/server.rs` ≥ 8 (`10` on source commit `8d22896d`)
- [x] `rg -n "with_session\(" crates/djls-server/src/server.rs` shows no remaining *read-only feature* handlers (only `Session::open_documents` accessor remains)
- [x] `cargo test -q` exits 0
- [x] `just test` exits 0
- [x] `just clippy` exits 0
- [x] Source diff limited to `crates/djls-server/src/session.rs`, `crates/djls-server/src/server.rs`, plus approved `crates/djls-server/Cargo.toml` dependency-scope divergence (`jj diff --stat -r @-`)
- [x] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- The vendored salsa exposes neither `Cancelled::catch` nor a panic payload
  you can downcast — report the actual cancellation surface.
- `DjangoDatabase` fails the `Send + 'static` bound for `spawn_blocking`
  (it must not — the marker test asserts `Send` — but if a field changed,
  report).
- A handler turns out to mutate state through `&Session` (e.g. a hidden
  `get_or_create_file` consequence that breaks under concurrency) — report
  which one.
- Deadlock risk materializes: a salsa setter blocks waiting for a snapshot
  that is itself blocked on the session lock. Snapshots must never lock the
  session — if you find a code path needing that, STOP.

## Maintenance notes

- After this plan, the session lock guards only: document buffer mutations,
  settings updates, and Salsa writes. Keep it that way — reviewers should
  reject new feature code that takes `with_session` for reads.
- Plan 011 builds directly on this: the background refresh applies writes
  under a brief lock and warms queries on a snapshot.
- The retry-on-cancel policy (2 retries → fallback) is intentionally dumber
  than ty's re-queueing. If completion under heavy typing ever degrades,
  the upgrade path is returning LSP `ContentModified` errors and letting the
  client re-request — note it, don't build it now.
