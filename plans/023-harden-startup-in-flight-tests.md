# Plan 023: Reject internal in-flight startup hooks

> **Executor instructions**: Do not implement this plan. It records a rejected
> follow-up from the startup-loading track so future agents do not recreate the
> same hook-heavy test approach. If startup responsiveness regresses later,
> write a fresh plan that starts from the behavioral contract below.
>
> **Drift check**: Not applicable. The original executable plan was written
> against fetched `main` at commit `bbff2406` (`server: defer project loading
> from initialize (#679)`, 2026-06-21). It was attempted as draft PR #682 and
> rejected during review before merge.

## Status

- **Priority**: P2
- **Effort**: S/M
- **Risk**: MED
- **Depends on**: plans/022
- **Category**: testing / concurrency (startup-loading follow-up from PR #626)
- **Planned at**: commit `bbff2406`, 2026-06-21
- **Execution status**: REJECTED — internal pause hooks added too much
  implementation-coupled test machinery for the behavior they proved.

## Decision

Do not add private refresh pause hooks or deterministic white-box tests for
startup in-flight loading right now.

The true contract is behavioral:

- `initialize` returns protocol capabilities without waiting for Project Facts.
- `initialized` queues project loading and reports progress or log fallback.
- feature requests do not wait for startup refresh completion.
- facts-backed features work after startup progress completes.
- superseded refresh work cannot overwrite the active project state.

The draft implementation for this plan proved narrower internals:

- settings loading captured inputs and then released the session lock;
- Project Facts computation cloned the database and then released the session
  lock;
- stale epochs dropped settings/facts before apply.

Those facts are useful while reading the current code, but they are not the
stable product/server contract. Merging tests that pause inside
`load_project_settings` or `compute_project_refresh` would freeze the current
refresh choreography and create maintenance tax when the startup internals
change for valid reasons.

## Current coverage

Plan 022's merged shape already satisfies the contract through code structure
and black-box tests:

- `crates/djls-server/src/server.rs`: `initialize` stores protocol/session
  state and returns capabilities; `initialized` queues `ProjectRefreshReason::Startup`.
- `crates/djls-server/src/server.rs`: feature handlers read from current
  snapshots and do not call a startup refresh wait method.
- `crates/djls-server/src/session.rs`: `ProjectRefreshState` is an epoch
  counter only. There is no completion latch, completed-epoch watermark, or
  request gate.
- `tests/e2e/test_startup.py`: asserts protocol capabilities are available
  without startup loading, progress/log fallback is observable, and
  facts-backed completions work after startup progress completes.

This leaves one intentionally accepted gap: there is no deterministic
outside-in test proving that a request sent while refresh is truly in flight
returns before progress completes. Closing that gap without white-box hooks
would require one of three worse options:

- sleep or large-fixture timing tests;
- production-visible test protocol;
- internal `#[cfg(test)]` pause hooks in refresh code.

The project should prefer the current simpler codebase over those options.

## Rejected approach

Draft PR #682 (`plan-023-startup-in-flight-tests`) added `#[cfg(test)]`
refresh pause hooks and four `djls-server` tests that blocked settings
loading or Project Facts computation while taking a snapshot or bumping the
refresh epoch.

Reject that approach for now:

- it adds test machinery to production files to prove implementation details;
- it makes future refresh refactors update tests even when behavior remains
  correct;
- it does not improve user-visible startup coverage beyond the existing e2e
  contract;
- the no-wait behavior is easier to audit directly by absence of request gate
  code than by preserving private hook tests.

## Future trigger

Write a new plan only if there is a concrete regression or review concern that
the existing contract no longer protects. A future plan should prefer:

- a black-box request issued immediately after `initialized`, before waiting
  for startup progress, if it can be made deterministic without sleeps; or
- a small correctness test for stale epoch application if a future change
  weakens the existing epoch checks.

Do not revive internal refresh pause hooks, LSP test commands, startup
controllers, refresh-completion fields, or request wait methods without an
explicit maintainer decision.

## Done criteria

- [x] The executable hook-heavy plan is marked rejected.
- [x] The startup contract is recorded in behavioral terms.
- [x] No source changes are required for plan 023.
- [x] Future work has clear triggers and rejected alternatives.
