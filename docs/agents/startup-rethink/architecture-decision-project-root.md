# Architecture decision: stable Project root input

Status: accepted.

## Decision

DJLS will use a **stable `djls_project::Project` Salsa input** as the semantic root for Project Facts.

Server and CLI orchestration own loading state:

- startup generations;
- supersession;
- stale-document rejection;
- work-done progress;
- running/queued jobs;
- quiescence and milestones.

Salsa owns project facts:

- workspace/source roots;
- source inventory;
- discovery facts and diagnostics;
- settings candidates and environment candidates;
- template/app/Python inventories;
- optional runtime enrichment facts.

`ProjectLoadingState` is not the target architecture. It was useful scaffolding, but it must not become the semantic readiness root.

## Why this model

The reference evidence points to two viable patterns:

- rust-analyzer lowers VFS/project-model data into many domain inputs and keeps loading/progress/quiescence in server `GlobalState`.
- Ruff/ty stores a stable `Project` input handle on the database and mutates tracked project fields in place.

Django is closer to Ruff/ty than rust-analyzer for our current model. We do not have a real crate graph where most semantic queries naturally take a `Crate`-like root. We have a project/workspace with settings, environment candidates, source files, template inventories, and enrichment. A stable project root is the right semantic anchor.

We still keep the rust-analyzer lesson for orchestration: loading/progress/quiescence stays out of Salsa.

## Replacement contract

Replace:

```rust
#[salsa::db]
pub trait Db: djls_source::Db {
    fn project_loading_state(&self) -> ProjectLoadingState;
}
```

with:

```rust
#[salsa::db]
pub trait Db: djls_source::Db {
    /// Stable Project Facts root for this database.
    ///
    /// This handle is created once during database construction and is not
    /// swapped during reload. Reloads update tracked fields through setters.
    fn project(&self) -> Project;
}
```

The new `djls_project::Project` is distinct from the legacy `djls_semantic::Project`. The legacy semantic Project remains only as a temporary migration bridge and should be named as legacy in new prose/code when ambiguity matters.

Initial shape:

```rust
#[salsa::input]
pub struct Project {
    pub workspace_roots: ProjectWorkspaceRoots,
    pub source_inventory: ProjectSourceInventory,
    pub discovery: ProjectDiscovery,
    pub diagnostics: ProjectDiagnostics,
    pub enrichment: ProjectEnrichment,
}
```

Names may change as implementation sharpens them, but the ownership rule must not change: these are domain facts, not loading lifecycle flags.

## Forbidden in Project facts

Do not store these in `Project` or tracked semantic query inputs:

- `Loading` as “currently running”;
- `Stale` as “reload in progress”;
- startup generation IDs;
- LSP document freshness/snapshot rejection;
- work-done progress tokens;
- cancellation state;
- queued/running job counters;
- node/milestone progress state.

Those are server/CLI orchestration state.

Domain facts may still represent durable facts such as:

- `Unavailable { issue }` when facts genuinely cannot be produced from the current project inputs;
- `Disabled` when a feature is intentionally disabled;
- `Failed { issue }` for optional enrichment results that were attempted and failed;
- diagnostics for config/interpreter/env-file/discovery failures.

The distinction is: a query may depend on project facts, but not on “what the startup executor is doing right now.”

## Reload and stale/failure semantics

Starting a reload must not erase existing facts.

Transitions:

| Transition | Salsa Project mutation | Server/CLI state |
|---|---|---|
| Initial DB construction | Create one virtual `Project` with empty/absent domain facts and diagnostics. | Not loading until a run starts. |
| Load/reload start | No semantic fact write just for starting. | Mark generation/job running; report progress. |
| Successful apply | Update the relevant `Project` fields through Salsa setters. | Mark node/run success or degraded from applied/domain outcome. |
| Failed reload with prior facts | Keep prior project fields; add/update diagnostics only if those diagnostics are durable project facts. | Report failed/degraded run. |
| Failed first load | Keep virtual project fields; add durable diagnostics where applicable. | Report failed/degraded run. |
| Superseded generation | No project fact write from the old generation. | Return `StartupRunOutcome::Superseded`. |
| Stale-document rejection | No project fact write. | Reject/restart/supersede via startup controller. |
| Configuration restart | Start new generation; only successful coherent applies update `Project`. | Older generation applies are rejected. |

This replaces hand-threaded `previous` snapshots. Previous good facts stay visible because failed/superseded reloads do not write over them.

## Source files and partitions

There must be one semantic owner for project source inventory.

`Project.source_inventory` owns the current domain source inventory. It can internally include partition data needed for first-party, installed-app, and configured-template-directory files, but aggregate source files, partition state, node status, and milestones must not become independent authorities.

The loading runner may return node outcomes for progress/CLI reporting, but semantic queries read the project source inventory and inventory-specific domain outcomes.

When installed-app and template-directory file loading arrive, their data must flow through one source-inventory update path. Avoid this shape:

```text
ProjectLoadingState.source_files = Ready(...)
plus separate per-node readiness as another truth source
```

Prefer this shape:

```text
Project.source_inventory owns partitions + merged view
node/milestone/progress status is derived from the apply result or domain query outcome
```

## Query API rule

Prefer explicit project identity in new project/semantic queries:

```rust
#[salsa::tracked]
fn template_inventory(db: &dyn Db, project: Project, env: DjangoEnvironmentId) -> TemplateInventory;
```

The DB method `db.project()` is allowed at request/session boundaries and transitional query roots, but new code should avoid hiding project identity deep inside unrelated helpers when passing `Project` is straightforward.

## What stays from the current implementation

Keep these pieces:

- protocol-ready startup;
- `djls-source` file/source-root/file-set primitives;
- `djls-workspace` neutral walking;
- source-file materialization invariants in `djls-db`;
- neutral loading runner/effects/observer as CLI/LSP orchestration;
- LSP `StartupController`, generation guard, stale-document detection, and progress reporter.

Change their role:

- the runner orchestrates and reports work;
- apply paths mutate `Project` facts only after coherent success;
- reset/start operations no longer write `Loading`/`Stale` semantic states.

## First cleanup checkpoint

Before Phase 3A4d/3B feature work resumes:

1. Add `djls_project::Project` as the stable root input.
2. Add `Db::project() -> Project` and initialize the handle once in production, bench, and test databases.
3. Move current source-file facts from `ProjectLoadingState.source_files` into the new project source-inventory field.
4. Remove `begin_project_loading_run` semantic writes of `Loading`/`Stale`; keep run-start state in server/CLI orchestration.
5. Change stale-document rejection so it does not write failed project facts.
6. Delete or quarantine `ProjectLoadingState` and `Db::project_loading_state()` behind a temporary compile bridge only if a single cleanup change cannot remove all uses.
7. Preserve current behavior gates or replace them with explicit migration tests.

## Behavior-preservation gates for the cleanup

Run before and after the cleanup where applicable:

- source-file materialization/round-trip tests;
- source-file terminal transition tests, rewritten around project source inventory rather than `ProjectLoadingState`;
- `cargo test -p djls-project loading`;
- `cargo test -p djls-server startup_source_files`;
- `cargo test -p djls-server startup_request_while_loading`;
- `cargo test -p djls --test check`;
- `just fmt --check`;
- `cargo build -q`.

## Reference anchors

- rust-analyzer keeps loading/progress/quiescence in server `GlobalState` and lowers VFS/project-model facts into Salsa inputs.
- Ruff/ty stores a stable DB-owned `Project` input with tracked fields and warns that reloads must mutate the existing handle through setters rather than swapping it.
- Ruff/ty keeps LSP workspace initialization/readiness in session state outside Salsa.

Pinned evidence is summarized in `reference-evidence-rust-analyzer-ruff-ty.md`.
