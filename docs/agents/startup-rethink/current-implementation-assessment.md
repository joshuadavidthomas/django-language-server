# Current implementation assessment against main and reference architectures

This note assesses the current `startup-rethink` stack against `main`, then compares the implementation shape to rust-analyzer and Ruff/ty. It is intentionally diagnostic: it should guide a plan revision before more phases build on the current seams.

Decision update: `architecture-decision-project-root.md` chooses a Ruff/ty-style stable `djls_project::Project` root input for semantic Project Facts, with rust-analyzer-style server/CLI ownership of loading/progress/quiescence. The open A/B/C directions near the end of this assessment are historical diagnosis; the accepted target is the stable Project root.

## Repository state

Current VCS state at assessment time:

```text
jj st
The working copy has no changes.
Working copy  (@) : rqpkyvqm 5c30c228 (empty) (no description set)
Parent commit (@-): zzwtomox c47ca83b startup-rethink | tighten startup progress reporting
```

`startup-rethink` points to `zzwtomox c47ca83b`.

Current stack tail:

```text
zzwtomox tighten startup progress reporting
vwkwqxpn add startup progress lifecycle
luktmluq tighten LSP loading control
kttmzkwn run source file loading through LSP executor
toyvwmzs add LSP startup generation guard
snvkzvko tighten CLI source file loading
sslnwvtv run source file node through CLI
tqmsxptl tighten source file materialization invariants
pytqoqsw materialize project source files
kknmtssr tighten startup plan invariants
qvorvkpv add first-party source loading seam
nmqyksul add project loading state shell
xowsopvw add project crate helper boundary
xsnutlnv add neutral source file set primitives
nyntuxws make LSP startup protocol-ready
sqoqvvrn docs: add startup rethink planning docs
```

Diff from `main` to `startup-rethink`:

```text
52 files changed, 9705 insertions(+), 668 deletions(-)
```

Major changed areas:

- new `djls-project` crate and loading modules;
- neutral source/workspace file-set primitives;
- `DjangoDatabase` project loading/source-file materialization hooks;
- CLI loading executor for `djls check`;
- server-local startup generation guard, LSP source-file executor, and progress lifecycle;
- startup planning docs and LSP smoke tests.

## Current implemented shape

### Protocol startup

Implemented:

- `DjangoDatabase::new(...)` no longer bootstraps the old semantic `Project` implicitly.
- `DjangoDatabase::bootstrap_project(...)` exists for project-aware callers.
- `Session::new` captures workspace roots/client settings without full project config/bootstrap.
- LSP `initialized` is still not wired to the new full loading graph; Phase 3A4d was next.

This matches the plan and matches rust-analyzer/Ruff guidance: handshake should not perform expensive project discovery.

### Source/workspace primitives

Implemented:

- `djls-source` owns `SourceFileSet`, source roots, discovered/loaded files, file-set summary and invariants.
- `djls-workspace` owns file walking mechanics.
- `djls-project` owns first-party root construction, first-party patch/merge policy, and source-file readiness/update types.

This is broadly aligned with rust-analyzer’s lowered-input direction: files and roots are real domain inputs/primitives rather than ad hoc side state.

### Shared loading runner and adapters

Implemented:

- `djls-project::loading::plan` defines the initial one-node `source-file-set` plan and terminal projection.
- `run_loading_plan(...)` is neutral and effect-driven.
- CLI and LSP each implement concrete effects.
- LSP generation supersession/rejected apply are now execution outcomes, not fake project readiness.
- LSP progress observes neutral loading events.

This is aligned with the plan’s separation of graph order, execution/apply behavior, and reporting. It also aligns with rust-analyzer keeping server orchestration/progress outside semantic core.

### The main drift: `ProjectLoadingState` as ambient DB singleton

Current code commits to:

```rust
#[salsa::input]
pub struct ProjectLoadingState {
    pub source_files: ProjectSourceFilesAvailability,
    pub discovery: ProjectDiscoveryAvailability,
    pub enrichment: ProjectEnrichmentState,
}
```

and:

```rust
#[salsa::db]
pub trait Db: djls_source::Db {
    fn project_loading_state(&self) -> ProjectLoadingState;
}
```

Concrete DBs implement that by storing a handle on the DB. In production this is currently:

```rust
pub(crate) project_loading_state: Arc<Mutex<Option<ProjectLoadingState>>>;
```

This is not only a `Mutex<Option<_>>` smell. Even replacing it with `OnceLock` would preserve the deeper design: an ambient, DB-owned singleton `ProjectLoadingState` is the semantic readiness root.

Evidence from the plan says this was intentional in Phase 3A2a:

> Define `#[salsa::db] pub trait Db: djls_source::Db` and add `fn project_loading_state(&self) -> ProjectLoadingState` as the single Salsa-visible readiness handle for project loading.

So the code did not accidentally invent this; the plan directed it. The question is whether the plan’s seam is correct.

## Comparison to rust-analyzer

rust-analyzer evidence says:

- analyzer inputs are lowered from VFS/project_model;
- concrete `RootDatabase` has storage, file/source-root maps, crate maps, and nonce, not a loading-state handle;
- loading/progress/quiescence is computed from server `GlobalState` queues and flags;
- semantic queries consume explicit inputs such as `FileId`, `SourceRoot`, `Crate`, and derived query results.

Under that comparison, current DJLS drift is:

- `ProjectLoadingState` stores `Loading`, `Ready`, `Unavailable`, `Stale`, `Failed` as semantic input state;
- tracked queries are expected to read `db.project_loading_state().source_files(db)` before deriving layout/settings/etc.;
- the source-file surface carries manual `previous` snapshots rather than letting durable domain inputs plus server quiescence determine what is current.

This is not rust-analyzer-style.

## Comparison to Ruff/ty

Ruff/ty evidence says:

- `ProjectDatabase` does store a DB field `project: Option<Project>` and exposes `fn project(&self) -> Project`;
- the handle is explicitly stable for the database lifetime;
- `Project` is a real domain root input: metadata, settings, included paths, file set, open files, diagnostics, check mode;
- structural reload updates the existing `Project` input via setters;
- LSP workspace initialization/readiness remains session state outside Salsa.

Under that comparison, current DJLS drift is:

- `ProjectLoadingState` is not a project/workspace root model; it is a readiness bag;
- `ProjectLoadingState` has discovery/enrichment placeholders and source-file availability, but not stable root/project metadata/settings/source roots as the primary identity;
- DB-owned `project_loading_state()` resembles Ruff’s `db.project()` mechanically, but not semantically.

This is not clean Ruff/ty-style either.

## Dual-source-of-truth risk already present

The current source-file implementation has two readiness surfaces:

1. Query-visible aggregate source-file readiness:

   ```rust
   ProjectLoadingState.source_files: ProjectSourceFilesAvailability
   ```

2. Node/partition readiness:

   ```rust
   ProjectFileLoadingTransition / ProjectFilePartitionReadiness
   ProjectSourceFilesApplied.transition
   node_status_from_readiness(ProjectSourceFilesApplyResult)
   ```

The plan says this distinction is intentional because aggregate source files are for queries while per-node/partition transitions drive node/milestone status.

Risk: every file-loading apply path must update both coherently forever. Phase 6B adds installed-app and template-directory partitions, which makes the dual state more complex. Reference architectures generally avoid this by making one domain input/index the source of truth and deriving status/progress from it.

## Manual previous snapshots risk

`ProjectSourceFilesAvailability` carries:

- `Deferred { previous }`
- `Unavailable { previous }`
- `Failed { previous }`
- `Stale { previous }`

That means stale-while-revalidate behavior is hand-threaded through the readiness enum. Every failure/deferred path must preserve the right previous value. This can be correct, but it is not the natural rust-analyzer shape; rust-analyzer tends to keep old server/project model until new lowering succeeds, then apply a coherent DB change.

## What is good and worth preserving

- Protocol startup is now cheap/protocol-only.
- Source/file/root primitives are mostly in the right crates.
- File walking is outside the session lock.
- LSP generations and progress are server-local.
- The loading runner now separates execution outcomes from domain readiness.
- Progress is observational and nonblocking after the review follow-up.

Those are not the drift. The drift is the semantic readiness root and upcoming plan structure.

## Immediate conclusion

We should stop before Phase 3A4d/3B and redesign the project-state/readiness model.

The current implementation followed the plan, but the plan appears to have selected a hybrid architecture:

- rust-analyzer language in the overview;
- Ruff/ty-like DB-owned handle mechanics;
- a readiness bag instead of a stable project root;
- future derived-query nodes with no `ProjectLoadingState` field;
- future file nodes with both aggregate and partition readiness.

That hybrid is the source of drift.

## Accepted correction direction

Use a Ruff/ty-style stable `djls_project::Project` Salsa input as the semantic root, and keep rust-analyzer-style loading/progress/quiescence in server/CLI orchestration.

Concretely:

- Introduce a stable `djls_project::Project` input.
- Store that handle as the database project root, with Ruff’s warning: never swap it during reload; mutate tracked fields with setters.
- Put root/project metadata, settings, included paths, source inventory, discovery diagnostics, inventories, and enrichment facts under that root as they land.
- Keep startup generations, progress, quiescence, stale-document rejection, and loading graph running state outside Salsa.
- Avoid a separate ambient `ProjectLoadingState` readiness bag.

The transition can be incremental, but it must target this model rather than preserving A/B/C as open choices.
