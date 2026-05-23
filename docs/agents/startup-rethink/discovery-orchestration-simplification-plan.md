# Plan: discovery orchestration simplification

## Overview

Keep the Ty-like static Django analysis ambition. Cut the startup framework ceremony around it.

Django Discovery should remain the single process that advances Project Facts toward readiness. The simplification target is the orchestration layer: retained payloads, public result types, duplicated stage plumbing, overnamed wrapper enums, and helper types that leak beyond the crate surface.

## Current State

`djls-project` owns a fixed Django Discovery Run with seven stages: source files, project root discovery, Python source models, Django environments, installed-app files, template-directory files, and enrichment (`crates/djls-project/src/discovery_run.rs:58`, `crates/djls-project/src/discovery_run.rs:519`).

The current run returns `DiscoveryRunResult`, which stores rich per-stage payloads in `DiscoveryStageResult` plus milestone results and an optional execution outcome (`crates/djls-project/src/discovery_run.rs:187`, `crates/djls-project/src/discovery_run.rs:266`). Most of those payloads are not part of the lifecycle contract. LSP progress, logs, milestones, and server finish use only stages, statuses, milestones, and the execution outcome (`crates/djls-server/src/startup.rs:435`, `crates/djls-server/src/startup.rs:486`, `crates/djls-server/src/startup.rs:892`).

`DiscoveryStageResult`, `DiscoveryMilestoneResult`, and `DiscoveryReadiness` are effectively internal to `discovery_run.rs` and tests. The only external payload consumer found was a `djls-db` test helper using `source_file_set_result()` (`crates/djls-db/src/db.rs:549`).

The host seam is real. The CLI host performs direct, uncancellable operations (`crates/djls/src/discovery.rs:33`). The LSP host adds generation checks, DB snapshots, guarded mutation, and stale snapshot rejection (`crates/djls-server/src/startup.rs:892`). File walking still belongs to hosts, not tracked Salsa queries.

## Desired End State

Django Discovery keeps the same broad behavior with less API surface:

- `run_django_discovery` remains the ordered sequencer.
- `djls-project` still owns stage order, status policy, and source-file update construction.
- Hosts still own cancellation, file walking, guarded DB mutation, and runtime enrichment loading.
- LSP progress still reports stage started/finished and `WorkspaceReady` / `DjangoAppsReady`.
- The returned run result is status/event oriented, not a bag of stage payloads.
- Stale snapshot rejection remains apply-only.
- Source-file root issues and materialization invariants still flow into readiness/status decisions.
- CLI/server do not import source-root builders, patch builders, or merge helpers.

## Change Inventory

This plan separates deletion, refactoring, and new code so implementation does not drift into another framework.

### Removing

Remove retained data that does not drive behavior:

- Remove rich `DiscoveryStageResult` variants that store stage payloads after status has already been computed:
  - `SourceFiles { applied: SourceFilesApplyResult, status }`
  - `ProjectRootDiscovery { applied: ProjectRootDiscoveryApplyResult, status }`
  - `PythonSourceModels { observed: PythonSourceIndexOutcome, status }`
  - `DjangoEnvironments { observed: DjangoEnvironmentCandidatesOutcome, status }`
  - `InstalledAppFiles { applied: Vec<SourceFilesApplyResult>, status }`
  - `TemplateDirectoryFiles { applied: Vec<SourceFilesApplyResult>, status }`
  - `Enrichment { applied: ProjectEnrichment, status }`
- Remove `DiscoveryRunResult::source_file_set_result()`. It exists for one test helper and keeps source-file payloads alive in the public run result.
- Remove the fake `Vec<SourceFilesApplyResult>` fanout for installed-app and template-directory stages. Each stage currently applies zero or one update.
- Remove public exports for types that are not external API after the result collapse:
  - `DiscoveryStageResult` or its payload-bearing replacement;
  - `DiscoveryMilestoneResult` if milestone records remain internal;
  - `DiscoveryReadiness` if status projection becomes private.
- Remove `stage_status_from_readiness(...)`; it only forwards to the trait method.
- Remove the redundant `NoopDiscoveryObserver::milestone_reached` implementation; the trait default already does nothing.
- Remove public visibility from source-file construction internals that external crates should not name:
  - `SourceRootsPlan`;
  - `SourceFilesLoadRequest`;
  - `PartitionedSourceFilePatch`;
  - `PartitionedSourceFilePatchSet`;
  - patch merge helpers.

Do not remove the source-file materialization model itself. `SourceFilesUpdate`, `SourceFilesMaterializationPatch`, `SourceFileSetMaterialized`, and `SourceFilesApplyResult` remain the `djls-project` to `djls-db` apply contract.

### Refactoring

Refactor code that carries the right behavior through too much scaffolding:

- Refactor `DiscoveryRunResult` from "stored stage payloads" to "ordered lifecycle records plus final execution outcome."
- Refactor each `run_*_stage` function so it:
  1. computes the domain payload locally;
  2. derives a `DiscoveryStageStatus`;
  3. reports `stage_finished`;
  4. returns only the stage/status record.
- Refactor milestone advancement to read stage/status records rather than pattern matching on rich stage result variants.
- Refactor `djls-db` tests that depend on `source_file_set_result()` to record the applied source-files result in the test host, or assert against DB state and materialization counters.
- Refactor host trait methods that currently return `DiscoveryObservationOutcome<T>` / `DiscoveryApplyOutcome<T>` to return standard result shapes while preserving the semantic split:
  - observation/load/checkpoint paths can only cancel because the run was superseded;
  - apply paths can abort because the run was superseded or because the guarded mutation saw a stale snapshot.
- Refactor status projection from a public trait into private functions or private inherent helpers in `discovery_run.rs`.
- Refactor the first-party source-file request builder chain into a smaller internal entry point only if it preserves both normalized roots and root issues.
- Refactor repeated abort-handling matches into local helpers after the result shape is smaller.

### Adding

Add only small replacement shapes that let us delete larger ones:

- Add a compact stage lifecycle record:

```rust
pub struct DiscoveryStageRecord {
    stage: DiscoveryStage,
    status: DiscoveryStageStatus,
}
```

- Optionally add a compact milestone record if callers still need to inspect reached milestones after the run:

```rust
pub struct DiscoveryMilestoneRecord {
    milestone: DiscoveryMilestone,
    status: DiscoveryMilestoneStatus,
}
```

- Optionally add type aliases for the result shapes, if they improve readability without creating a new concept:

```rust
type DiscoveryObservation<T> = Result<T, DiscoveryCancellation>;
type DiscoveryApply<T> = Result<T, DiscoveryExecutionOutcome>;
```

- Add local helper functions in `discovery_run.rs` only when they delete repeated code directly. Candidate helpers:
  - unwrap an observation and finish the stage on supersession;
  - unwrap an apply result and finish the stage on abort;
  - load files, checkpoint, build a source-files update, and apply it.
- Add or adjust test-host fields only where tests need evidence that was previously taken from rich run payloads.

Do not add a generic stage runner, generic progress framework, or new host abstraction in the first pass. Those would replace old ceremony with new ceremony.

## Key Discoveries

- The progress channel is already payload-free. `DiscoveryObserver` only receives stage/status/milestone enums (`crates/djls-project/src/discovery_run.rs:143`).
- Client progress formats debug strings from those enums and sends them as work-done progress reports (`crates/djls-server/src/startup.rs:523`).
- Milestones use stage statuses, not rich stage payloads (`crates/djls-project/src/discovery_run.rs:816`).
- `InstalledAppFiles` and `TemplateDirectoryFiles` store `Vec<SourceFilesApplyResult>`, but each stage only creates zero or one apply result (`crates/djls-project/src/discovery_run.rs:658`, `crates/djls-project/src/discovery_run.rs:692`).
- `DiscoveryCancellation` currently means only supersession. `StaleSnapshot` is an apply-side execution outcome and should not leak into observation/load paths (`crates/djls-project/src/discovery_run.rs:88`, `crates/djls-server/src/startup.rs:928`).
- Source-root normalization issues are real. Any request-helper collapse must preserve root issues alongside the file-walk request (`crates/djls-project/src/source_files.rs:126`, `crates/djls-project/src/source_files.rs:239`).

## What We're Not Doing

- Do not weaken static Project Facts or retreat to runtime-first Django introspection.
- Do not move file walking into tracked Salsa queries.
- Do not make CLI/server build source-root plans, partition patches, or merge updates.
- Do not remove LSP progress or milestones.
- Do not make runtime enrichment a readiness gate.
- Do not start by redesigning `DiscoveryHost`; simplify the run result and wrappers first.
- Do not add compatibility aliases for renamed or removed discovery types unless a public contract forces it.

## Implementation Approach

Work from the highest-leverage deletion inward:

1. Remove retained payloads from run results.
2. Make milestones operate on ordered status records.
3. Replace custom wrapper enums with standard `Result` shapes while preserving the stale-snapshot boundary.
4. Delete readiness indirection.
5. Demote source-file construction internals.
6. Reassess the host seam only after the simpler shape lands.

After each phase, run focused checks before continuing. If a phase changes observable progress behavior, stop and inspect the affected LSP/startup tests before proceeding.

## Phase 1: Replace rich stage results with status records

### Overview

Make the returned discovery run describe lifecycle, not stage payloads. Stage payloads should stay local to each `run_*_stage` function long enough to compute a status, then be dropped.

### Changes Required

#### 1. Discovery run result shape

**File**: `crates/djls-project/src/discovery_run.rs`

**Changes**:

- Replace `DiscoveryStageResult` payload variants with a compact record:

```rust
pub struct DiscoveryStageRecord {
    stage: DiscoveryStage,
    status: DiscoveryStageStatus,
}
```

- Rename accessors from `stage_results()` to `stage_records()` if useful.
- Keep ordered records. Append one record after each completed stage.
- Keep `execution_outcome()` for server finish.
- Remove `source_file_set_result()`.
- Replace `InstalledAppFiles { applied: Vec<_> }` and `TemplateDirectoryFiles { applied: Vec<_> }` with status-only records.

#### 2. Stage functions

**File**: `crates/djls-project/src/discovery_run.rs`

**Changes**:

- Change each `run_*_stage` to return `DiscoveryStageRecord`.
- Keep local variables like `applied`, `source_index`, or `enrichment` only until status is computed.
- Preserve `observer.stage_finished(stage, status.clone())` exactly.

#### 3. DB test helper

**File**: `crates/djls-db/src/db.rs`

**Changes**:

- Replace `result.source_file_set_result()` in the test helper with direct DB/host evidence.
- The helper already records `host.materializations`; keep that as the handle-change evidence.
- If the test needs the source-files apply result, make the test host record the last `SourceFilesApplyResult` returned from `apply_source_files`.

#### 4. Public exports

**File**: `crates/djls-project/src/lib.rs`

**Changes**:

- Stop exporting rich stage/milestone payload types if no external crate needs them.
- Export only the lifecycle types that hosts/progress need.

### Success Criteria

#### Automated Verification

- [x] Discovery run tests pass: `cargo test -p djls-project discovery_run` (7 passed)
- [x] DB materialization tests pass: `cargo test -p djls-db source_file_set_materialization` (2 passed)
- [x] Typecheck passes: `cargo check --all-targets`
- [x] Formatting passes: `just fmt --check`

#### Manual Verification

- [x] `DiscoveryRunResult` no longer stores `PythonSourceIndexOutcome`, `DjangoEnvironmentCandidatesOutcome`, `ProjectEnrichment`, or source-file apply payloads. Evidence: `DiscoveryRunResult` stores only `Vec<DiscoveryStageRecord>`, milestone results, and execution outcome in `crates/djls-project/src/discovery_run.rs`.
- [x] Stage ordering remains visible in the run result. Evidence: `DiscoveryStageRecord` preserves `stage`, and discovery tests assert record order.
- [x] Superseded stages still emit `DiscoveryStageStatus::Superseded`. Evidence: supersession discovery tests passed and assert the observer event.

## Phase 2: Keep progress and milestones status-first

### Overview

Keep milestone behavior, but make it operate on stage status records instead of rich stage results.

### Changes Required

#### 1. Milestone advancement

**File**: `crates/djls-project/src/discovery_run.rs`

**Changes**:

- Change `advance_milestones` to inspect `DiscoveryStageRecord` values.
- Keep `MILESTONE_SPECS` private.
- Preserve the existing rules:
  - `WorkspaceReady` requires `SourceFiles: Succeeded`, `PythonSourceModels: Succeeded | Skipped`, and `DjangoEnvironments: Succeeded | Degraded`.
  - `DjangoAppsReady` requires `InstalledAppFiles: Succeeded | Skipped` and `TemplateDirectoryFiles: Succeeded | Skipped`.
- Preserve degraded milestone status when any accepted prerequisite is degraded/skipped rather than succeeded.

#### 2. Progress tests

**File**: `crates/djls-server/src/startup.rs`

**Changes**:

- Update tests only if names or records changed.
- Do not change work-done progress wording unless the tests require it.

### Success Criteria

#### Automated Verification

- [x] Startup progress tests pass: `cargo test -p djls-server startup_progress` (3 passed)
- [x] Discovery milestone tests pass: `cargo test -p djls-project milestone` (5 passed)
- [x] Typecheck passes: `cargo check --all-targets`
- [x] Formatting passes: `just fmt --check`

#### Manual Verification

- [x] LSP progress still reports started/finished stages. Evidence: `cargo test -p djls-server startup_progress` passed.
- [x] LSP progress still reports `WorkspaceReady` and `DjangoAppsReady`. Evidence: startup progress and milestone tests passed.
- [x] Milestone code does not depend on stage payloads. Evidence: `advance_milestones` now takes `&[DiscoveryStageRecord]` and reads only `stage()` / `status()`.

## Phase 3: Replace wrapper outcomes with standard result shapes

### Overview

Remove custom `Observed` / `Applied` wrapper names where `Result` communicates the same thing. Preserve the semantic split between read-side cancellation and apply-side rejection.

### Changes Required

#### 1. Read/load/checkpoint paths

**File**: `crates/djls-project/src/discovery_run.rs`

**Changes**:

- Replace `DiscoveryObservationOutcome<T>` with `Result<T, DiscoveryCancellation>` or a type alias with that shape.
- Keep `DiscoveryCancellation` read-side and supersession-only unless a real second cancellation reason appears.
- Update host trait observation methods.
- Update CLI, server, and test host implementations.

#### 2. Apply paths

**File**: `crates/djls-project/src/discovery_run.rs`

**Changes**:

- Replace `DiscoveryApplyOutcome<T>` with `Result<T, DiscoveryExecutionOutcome>` or a type alias with that shape.
- Keep `DiscoveryExecutionOutcome::StaleSnapshot` apply-only.
- Add small local helpers for apply/observe unwrapping so each stage does not hand-match the same abort shape.

#### 3. Host implementations

**Files**:

- `crates/djls/src/discovery.rs`
- `crates/djls-server/src/startup.rs`
- `crates/djls-db/src/db.rs` test host
- `crates/djls-project/src/python/source.rs` test host

**Changes**:

- CLI host returns `Ok(...)` for observations/applies.
- LSP observation paths return `Err(DiscoveryCancellation::Superseded)` only for supersession.
- LSP apply paths return `Err(DiscoveryExecutionOutcome::Superseded)` or `Err(DiscoveryExecutionOutcome::StaleSnapshot)` as appropriate.

### Success Criteria

#### Automated Verification

- [x] Typecheck passes: `cargo check --all-targets`
- [x] Startup cancellation tests pass: `cargo test -p djls-server superseded` (4 passed)
- [x] Stale snapshot tests pass: `cargo test -p djls-server stale` (3 passed)
- [x] Formatting passes: `just fmt --check`

#### Manual Verification

- [x] No observation/load path can produce `StaleSnapshot`. Evidence: observation and load methods now return `DiscoveryObservation<T> = Result<T, DiscoveryCancellation>`; `rg "StaleSnapshot"` shows stale snapshots only in server apply handling and final outcome mapping.
- [x] Stale snapshot still aborts the run from guarded apply paths. Evidence: `cargo test -p djls-server stale` passed.
- [x] Supersession still reports a superseded stage finish. Evidence: `cargo test -p djls-server superseded` passed.

## Phase 4: Remove readiness indirection

### Overview

Keep status policy centralized but remove the trait/free-function ceremony.

### Changes Required

#### 1. Status projection

**File**: `crates/djls-project/src/discovery_run.rs`

**Changes**:

- Delete `stage_status_from_readiness`.
- Replace the public `DiscoveryReadiness` trait with private helper functions or private inherent methods.
- Keep the existing match logic in one place.
- Do not move status classification into CLI/server hosts.

Potential shape:

```rust
fn source_files_status(result: &SourceFilesApplyResult) -> DiscoveryStageStatus { ... }
fn project_root_status(result: &ProjectRootDiscoveryApplyResult) -> DiscoveryStageStatus { ... }
fn python_source_index_status(result: &PythonSourceIndexOutcome) -> DiscoveryStageStatus { ... }
fn django_environments_status(result: &DjangoEnvironmentCandidatesOutcome) -> DiscoveryStageStatus { ... }
fn enrichment_status(enrichment: &ProjectEnrichment) -> DiscoveryStageStatus { ... }
```

### Success Criteria

#### Automated Verification

- [x] Discovery run tests pass: `cargo test -p djls-project discovery_run` (7 passed)
- [x] Typecheck passes: `cargo check --all-targets`
- [x] Formatting passes: `just fmt --check`

#### Manual Verification

- [x] `DiscoveryReadiness` is no longer public API. Evidence: `rg "DiscoveryReadiness|stage_status_from_readiness" crates/djls-project/src/discovery_run.rs` has no matches.
- [x] Status mapping remains centralized in `discovery_run.rs`. Evidence: status projection now lives in private `*_status` functions in that file.
- [x] Exhaustive matches still cover domain outcome variants. Evidence: `cargo check --all-targets` passed after replacing trait impls with private functions.

## Phase 5: Demote source-file construction internals

### Overview

Keep the real `djls-project` to `djls-db` apply contract public. Demote root-plan, patch, and merge construction that only discovery stages need.

### Changes Required

#### 1. Public facade audit

**File**: `crates/djls-project/src/lib.rs`

**Changes**:

- Keep public only what external crates need:
  - `SourceFilesUpdate`
  - `SourceFilesMaterializationPatch`
  - `SourceFileSetMaterialized`
  - `SourceFilesApplyResult`
  - source inventory/readiness types used by callers
- Do not export root builders, patch builders, or merge helpers.

#### 2. Source-file helper visibility

**File**: `crates/djls-project/src/source_files.rs`

**Changes**:

- Demote these to `pub(crate)` where possible:
  - `SourceRootsPlan`
  - `SourceFilesLoadRequest`
  - `PartitionedSourceFilePatch`
  - `PartitionedSourceFilePatchSet`
  - `merge_partitioned_source_file_patch_set`
  - `merge_first_party_source_file_patch`
- If replacing the first-party helper chain, provide one narrow internal function that returns both root issues and the file-walk request.

Guardrail: do not drop `SourceRootsPlan` issues. Duplicate/unusable root facts must still flow into the update and status.

#### 3. App/template file-root wrappers

**Files**:

- `crates/djls-project/src/apps.rs`
- `crates/djls-project/src/templates/loading.rs`

**Changes**:

- Keep `InstalledAppFileRootsOutcome` and `TemplateDirectoryFileRootsOutcome` for the query → roots → host walk seam.
- Keep `files_request()` and `source_files_update()` methods crate-private unless external callers need them.
- Do not move request/update construction into CLI/server.

### Success Criteria

#### Automated Verification

- [x] Typecheck passes: `cargo check --all-targets`
- [x] Source-file tests pass: `cargo test -p djls-project source_files` (21 passed)
- [x] App/template inventory tests pass: `cargo test -p djls-project apps` (7 passed) and `cargo test -p djls-project templates::inventory` (9 passed). Note: the planned combined filter command is not valid Cargo syntax, so these were run separately.
- [x] Formatting passes: `just fmt --check`

#### Manual Verification

- [x] CLI/server do not import source-root builders, patch builders, or merge helpers. Evidence: `rg "build_source_roots|SourceRootsPlan|SourceFilesLoadRequest|FirstPartySourceFilePatch|PartitionedSourceFilePatch|merge_partitioned_source_file_patch|merge_first_party_source_file_patch" crates/djls/src crates/djls-server/src crates/djls-db/src` has no matches.
- [x] Root issues still degrade/fail the same source-file readiness cases. Evidence: `cargo test -p djls-project source_files` passed, including duplicate/missing root issue tests.
- [x] File walking still happens through `DiscoveryHost::load_files_for_roots`. Evidence: discovery stages still call the local `load_files_for_roots` wrapper, which delegates to `host.load_files_for_roots`.

## Phase 6: Reduce repeated stage plumbing

### Overview

After result shapes are smaller, remove repeated match blocks without adding a generic framework.

### Changes Required

#### 1. Local helpers

**File**: `crates/djls-project/src/discovery_run.rs`

**Changes**:

- Add small helpers for repeated abort handling only if they delete clear duplication.
- Candidates:
  - `observe_or_abort(...)`
  - `apply_or_abort(...)`
  - `load_and_apply_source_files(...)`
- Keep helpers local to `discovery_run.rs`.
- Avoid a generic `StageRunner` abstraction unless it deletes more interface than it adds.

#### 2. Noop observer cleanup

**File**: `crates/djls-project/src/discovery_run.rs`

**Changes**:

- Remove redundant `milestone_reached` implementation from `NoopDiscoveryObserver`; the trait already provides a default.

### Success Criteria

#### Automated Verification

- [x] Discovery run tests pass: `cargo test -p djls-project discovery_run` (7 passed)
- [x] Typecheck passes: `cargo check --all-targets`
- [x] Formatting passes: `just fmt --check`

#### Manual Verification

- [x] Stage functions are shorter but still readable. Evidence: repeated observation/apply abort matches now use local `observe_or_abort`, `apply_or_abort`, and `load_and_apply_source_files` helpers.
- [x] No new framework-level type exists only to run seven stages. Evidence: `rg "StageRunner|struct .*Runner|trait .*Runner" crates/djls-project/src/discovery_run.rs` has no matches.

## Phase 7: Reassess the host seam

### Overview

Only revisit `DiscoveryHost` after the earlier cleanup lands. The current seam exists for real reasons, but the simpler discovery run may reveal narrower host responsibilities.

### Questions to answer

- Can observation methods move out of the host without holding LSP locks too long?
- Can the host expose a DB snapshot capability instead of one method per observed fact?
- Would that reduce code, or merely move project-query knowledge into server/CLI adapters?
- Does the new shape preserve stale snapshot protection and supersession checks?

### Possible direction

A later host could own only:

- checkpoint/supersession;
- file walking;
- current source files;
- guarded apply methods;
- enrichment loading;
- maybe DB snapshot acquisition.

Do not implement this phase unless phases 1–6 make the remaining host methods obviously shallow.

### Assessment Result

Phase 7 is deferred. The remaining host methods are not obviously shallow enough to justify changing the seam in this pass.

Evidence:

- `checkpoint`, `load_files_for_roots`, `current_source_files`, and apply methods each preserve distinct LSP/process invariants: supersession checks, host-owned file walking, live source-file merge state, guarded mutation, and apply-only stale snapshot rejection.
- The four observation methods contain the only real repetition, but replacing them with a snapshot-style host seam would force discovery-run tests to fake query inputs through a project DB instead of directly faking observation outcomes.
- A concrete observation snapshot type would violate crate layering (`djls-project` cannot name `djls-db::DjangoDatabase`). An associated-type or closure-based observation seam avoids that but adds abstraction and still disrupts the precise `FakeHost` outcome tests.
- Runtime enrichment remains host-owned even though the LSP implementation currently uses a DB snapshot to load it.

Follow-up option, not implemented here: add a private `LspDiscoveryHost` helper for repeated guarded observation mechanics if this duplication becomes painful. That would be server-local cleanup, not a discovery host seam change.

### Success Criteria

#### Automated Verification

- [x] No implementation required unless a concrete simplification emerges. Evidence: Oracle follow-up found a snapshot seam would require substantial test rewrites/fake DB abstractions; no host-seam change was made.

#### Manual Verification

- [x] The host seam is judged by deletion: fewer methods, fewer adapters, same invariants. Evidence: the only deletion-positive seam change would remove observation methods, but it would move complexity into snapshot typing and tests rather than preserving the same invariants with less code.

## Testing Strategy

### Per-phase checks

Run after each phase:

- `cargo check --all-targets`
- `just fmt --check`
- The targeted tests named in that phase.

### Before push

Run the project gates:

- `cargo test --all-targets`
- `just clippy --allow-dirty`
- `just lint`

### Final validation

- [x] `cargo test --all-targets`
- [x] `just clippy --allow-dirty`
- [x] `just lint`
- [x] Static implementation review found no must-fix findings.

### Test focus

- Discovery stage ordering and milestone firing.
- LSP progress event order and superseded-run finish behavior.
- Stale snapshot rejection during guarded apply.
- Source-file materialization handle preservation.
- Root issue propagation.
- Installed-app and template-directory file loading behavior.

## Performance Considerations

The plan should reduce retained data in `DiscoveryRunResult`. It should not change the amount of source walking, Salsa materialization, Python extraction, or runtime enrichment work. Any helper extraction must not add extra DB snapshots or file walks.

## Migration Notes

This is internal crate API cleanup on an unreleased discovery branch. Prefer clean breaks over aliases. If an exported type disappears, update callers directly.

## References

- Domain glossary: `CONTEXT.md`
- Architecture overview: `ARCHITECTURE.md`
- Current discovery implementation: `crates/djls-project/src/discovery_run.rs`
- LSP startup/progress host: `crates/djls-server/src/startup.rs`
- CLI discovery host: `crates/djls/src/discovery.rs`
- DB apply helpers: `crates/djls-db/src/db.rs`
- Source-file update model: `crates/djls-project/src/source_files.rs`
- Prior cleanup plan: `docs/agents/startup-rethink/issue-payload-inventory.md`
