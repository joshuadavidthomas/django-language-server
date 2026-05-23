# Discovery Run Cleanup Plan

## Purpose

This plan turns the current startup-rethink branch into the intended end-state shape instead of shipping the generic loading scaffolding introduced during the PR.

The target is a clean break:

- no `LoadingPlan`, `LoadingEffects`, `run_loading_plan`, or `phase3` compatibility layer;
- no public `loading` module surface;
- **Django Discovery Run** is the domain process that advances **Project Facts**;
- **Project Root Discovery** is the Project Fact containing per-root interpreter/settings/pythonpath/env-var discovery inputs;
- **Source File Inventory** is the Project Fact containing known source files and readiness;
- `djls-project` owns discovery sequencing and domain update decisions;
- `djls-db` owns concrete Salsa materialization and mutation;
- `djls-server` and `djls` own runtime policy only: cancellation, locking, file walking, apply/observe callbacks, progress, and CLI strictness.

## Non-goals

- Do not preserve old public names as aliases.
- Do not add compatibility wrappers around `LoadingPlan` or `LoadingEffects`.
- Do not introduce dynamic node registries, plugin schedulers, or speculative parallel execution.
- Do not make modules public with `pub mod`; cross-crate API stays curated through crate-root `pub use`.
- Do not move file walking into tracked Salsa queries.
- Do not make runtime enrichment a readiness milestone.

## Target module split

Replace `crates/djls-project/src/loading.rs` and its submodules with explicit modules:

```text
crates/djls-project/src/
  project.rs          // stable Project Salsa input only
  source_files.rs     // Source File Inventory domain: roots, partitions, updates, materialization decisions
  root_discovery.rs   // Project Root Discovery load/update data from config/env/interpreter inputs
  discovery_run.rs    // Django Discovery Run sequencing, host contract, observer, stages, milestones
```

`crates/djls-project/src/lib.rs` remains the only external module surface. It should declare private modules and re-export only intentional cross-crate API.

## Target names

### Discovery run names

```rust
DjangoDiscoveryRequest
DiscoveryStage
DiscoveryMilestone
DiscoveryStageStatus
DiscoveryMilestoneStatus
DiscoveryCancellation
DiscoveryExecutionOutcome
DiscoveryApplyOutcome<T>
DiscoveryObservationOutcome<T>
DiscoveryHost
DiscoveryObserver
NoopDiscoveryObserver
DiscoveryRunResult
DiscoveryStageResult
DiscoveryMilestoneResult
run_django_discovery
```

Stage variants:

```rust
DiscoveryStage::SourceFiles
DiscoveryStage::ProjectRootDiscovery
DiscoveryStage::PythonSourceModels
DiscoveryStage::DjangoEnvironments
DiscoveryStage::InstalledAppFiles
DiscoveryStage::TemplateDirectoryFiles
DiscoveryStage::Enrichment
```

Milestone variants:

```rust
DiscoveryMilestone::WorkspaceReady
DiscoveryMilestone::DjangoAppsReady
```

Execution/cancellation model:

```rust
pub enum DiscoveryCancellation {
    Superseded,
}

pub enum DiscoveryExecutionOutcome {
    Superseded,
    StaleSnapshot,
}

pub enum DiscoveryApplyOutcome<T> {
    Applied(T),
    Aborted(DiscoveryExecutionOutcome),
}

pub enum DiscoveryObservationOutcome<T> {
    Observed(T),
    Cancelled(DiscoveryCancellation),
}
```

### Source file names

```rust
SourceFileInventory
ReadySourceFiles
SourceFilesIssue
SourceFilesUpdate
SourceFilesApplyDecision
SourceFilesApplyResult
SourceFilesApplied
SourceFilesMaterializationPatch
SourceFileSetMaterialized
SourceFileHandleChanges
SourceFileMaterializationIssue
SourceFilePartitionReadiness
SourceFilePartitionTransition
```

Do not export source-file patch/building internals:

- `FileSetPartition`
- `FileSetPartitionId`
- partition snapshots
- first-party/partition patch builders
- source-root request builders
- merge helpers

### Project Root Discovery names

```rust
ProjectRootDiscovery
ProjectRootDiscoverySet
ProjectRootDiscoveryApplyResult
ProjectRootDiscoveryIssue
ProjectRootDiscoveryIssues
ProjectRootDiscoveryLoadRequest
ProjectRootDiscoveryUpdate
RootDiscoveryUpdate
RootDiscoveryInput
load_project_root_discovery
set_project_root_discovery
Project::root_discovery
```

## Target discovery host contract

`DiscoveryHost` should expose runtime/materialization ports, not the domain algorithm:

```rust
pub trait DiscoveryHost {
    fn checkpoint(&mut self) -> Result<(), DiscoveryCancellation>;

    fn load_files_for_roots(
        &mut self,
        request: djls_workspace::FilesForRootsRequest,
    ) -> Result<djls_workspace::FilesForRootsResult, DiscoveryCancellation>;

    fn current_source_files(&mut self) -> Option<ReadySourceFiles>;

    fn apply_source_files(
        &mut self,
        update: SourceFilesUpdate,
    ) -> DiscoveryApplyOutcome<SourceFilesApplyResult>;

    fn apply_project_root_discovery(
        &mut self,
        update: ProjectRootDiscoveryUpdate,
    ) -> DiscoveryApplyOutcome<ProjectRootDiscoveryApplyResult>;

    fn observe_python_source_index(
        &mut self,
    ) -> DiscoveryObservationOutcome<PythonSourceIndexOutcome>;

    fn observe_django_environment_candidates(
        &mut self,
    ) -> DiscoveryObservationOutcome<DjangoEnvironmentCandidatesOutcome>;

    fn load_project_enrichment(
        &mut self,
    ) -> Result<ProjectEnrichment, DiscoveryCancellation>;

    fn apply_project_enrichment(
        &mut self,
        enrichment: ProjectEnrichment,
    ) -> DiscoveryApplyOutcome<ProjectEnrichment>;
}
```

The host may check cancellation before file walking or enrichment. File walking itself does not need internal cancellation. The driver must checkpoint before and after expensive work and must discard superseded results before apply.

## Phase 1: Split modules and rename Project Facts state

### Files

- `crates/djls-project/src/lib.rs`
- `crates/djls-project/src/project.rs`
- `crates/djls-project/src/source_files.rs`
- `crates/djls-project/src/root_discovery.rs`
- `crates/djls-project/src/db.rs`
- all `crates/djls-project/src/**/*.rs` imports/tests that reference old names
- `crates/djls-db/src/db.rs`
- `crates/djls-server/src/session.rs`

### Edits

1. Create `project.rs` from the `Project` Salsa input in `loading/state.rs`.
   - `Project` fields become:
     ```rust
     pub source_inventory: SourceFileInventory,
     #[returns(ref)]
     pub root_discovery: ProjectRootDiscovery,
     #[returns(ref)]
     pub enrichment: ProjectEnrichment,
     ```
   - `Project::virtual_project` uses `SourceFileInventory::Unavailable { issue: SourceFilesIssue::NotLoaded }` and `ProjectRootDiscovery::Absent`.
   - `Project::fixture_unavailable` uses `SourceFilesIssue::FixtureUnavailable` and `ProjectRootDiscovery::Unavailable`.

2. Create `source_files.rs` from source-file inventory and file-set logic in `loading/state.rs` and `loading/files.rs`.
   - Move `ProjectSourceInventory` to `SourceFileInventory`.
   - Move `ReadyProjectSourceFiles` to `ReadySourceFiles`.
   - Move `ProjectSourceFilesIssue` to `SourceFilesIssue`.
   - Move source root building, partition readiness, source-file updates, materialization patches, apply result types, and source-file tests.
   - Rename related source-file types according to the target names above.
   - Keep internal partition types private or `pub(crate)`.

3. Create `root_discovery.rs` from `discovery.rs` and `loading/settings.rs` concepts.
   - Rename `ProjectDiscovery` to `ProjectRootDiscovery`.
   - Rename `ProjectDiscoverySet` to `ProjectRootDiscoverySet`.
   - Rename `ProjectDiscoveryApplyResult` to `ProjectRootDiscoveryApplyResult`.
   - Rename `ProjectDiscoveryIssue(s)` to `ProjectRootDiscoveryIssue(s)`.
   - Rename `ProjectDiscoveryLoadRequest` to `ProjectRootDiscoveryLoadRequest`.
   - Rename `ProjectDiscoverySetData` to `ProjectRootDiscoveryUpdate`.
   - Rename `RootDiscoveryData` to `RootDiscoveryUpdate`.
   - Rename `build_project_discovery_data` to `load_project_root_discovery`.
   - Keep `RootDiscoveryInput` as the Salsa input type.

4. Update `db.rs` trait setters.
   - Rename:
     ```rust
     set_project_source_inventory -> set_source_file_inventory
     set_project_discovery -> set_project_root_discovery
     ```
   - Import from owning private modules inside `djls-project`, not from crate-root exports.

5. Update `lib.rs`.
   - Replace `mod loading;` with:
     ```rust
     mod discovery_run;
     mod project;
     mod root_discovery;
     mod source_files;
     ```
   - Do not add `pub mod`.
   - Re-export only intentional crate-root API needed by other crates.
   - Remove all `loading::*` re-exports.

### Verification

Run after this phase:

```bash
cargo check -p djls-project --all-targets
```

Expected success criteria:

- [x] No `loading` module required for project state/source-files/root-discovery types.
  - Evidence: `Project` lives in `crates/djls-project/src/project.rs`, source-file inventory/update types live in `crates/djls-project/src/source_files.rs`, and Project Root Discovery load/update types live in `crates/djls-project/src/root_discovery.rs`.
- [x] No old `ProjectSourceInventory`, `ReadyProjectSourceFiles`, `ProjectSourceFilesIssue`, or `ProjectDiscovery*` names remain except in docs/historical plan files if intentionally untouched.
  - Evidence: `rg "ProjectSourceInventory|ReadyProjectSourceFiles|ProjectSourceFilesIssue|ProjectDiscovery|RootDiscoveryData|ProjectDiscoverySetData|ProjectDiscoveryLoadRequest|build_project_discovery_data|set_project_source_inventory|set_project_discovery" crates/djls-project crates/djls-db crates/djls-server crates/djls crates/djls-semantic crates/djls-ide` returned no matches.
- [x] Internal `djls-project` code imports from owning modules.
  - Evidence: `rg "use crate::(ProjectRootDiscovery|ProjectRootDiscoverySet|ProjectRootDiscoveryIssue|ProjectRootDiscoveryIssues|SourceFileInventory|ReadySourceFiles|SourceFilesIssue|SourceFilesApplyResult|SourceFilesUpdate|Project);" crates/djls-project/src -g '*.rs'` returned no matches; `cargo check --all-targets` and `just fmt --check` passed.

## Phase 2: Make source-file apply a decision, not a mutation

### Files

- `crates/djls-project/src/source_files.rs`
- `crates/djls-db/src/db.rs`
- `crates/djls-project/src/db.rs`
- tests in both crates

### Edits

1. Replace mutating source-file finalization with `SourceFilesApplyDecision`.

   Target shape:

   ```rust
   pub struct SourceFilesApplyDecision {
       result: SourceFilesApplyResult,
       next_inventory: Option<SourceFileInventory>,
   }

   impl SourceFilesApplyDecision {
       pub fn result(&self) -> &SourceFilesApplyResult { ... }
       pub fn next_inventory(&self) -> Option<&SourceFileInventory> { ... }
       pub fn into_result(self) -> SourceFilesApplyResult { ... }
   }
   ```

   Keep fields private.

2. Move `finalize_project_source_files(...)` behavior into an inherent method:

   ```rust
   impl SourceFilesUpdate {
       pub fn decide_apply(
           self,
           previous: Option<ReadySourceFiles>,
           materialized: SourceFileSetMaterialized,
       ) -> SourceFilesApplyDecision {
           ...
       }
   }
   ```

3. Remove `&mut dyn Db` from source-file decision logic.
   - `source_files.rs` may compute `SourceFileInventory::Ready(files)` or `SourceFileInventory::Unavailable { issue }` as a value.
   - It must not call `set_source_file_inventory`.

4. Update `DjangoDatabase::apply_project_source_files`, renaming if appropriate to `apply_source_files`.
   - It should:
     1. capture `previous = current_ready_source_files()`;
     2. materialize using the update materialization patch;
     3. call `update.decide_apply(previous, materialized)`;
     4. if `decision.next_inventory()` is `Some`, call `LoadingDb::set_source_file_inventory(self, inventory.clone())`;
     5. return `decision.into_result()`.

5. Update tests to assert transition invariants.
   Required cases:
   - applied result sets next inventory to `Ready(applied.files())`;
   - materialization mismatch with no previous ready publishes unavailable/failed inventory;
   - materialization mismatch with previous ready preserves the exact previous ready inventory;
   - terminal update issue with previous ready preserves previous;
   - `SourceFilesApplyDecision` cannot be constructed outside `source_files.rs` with inconsistent fields.

### Verification

```bash
cargo test -p djls-project source_files
cargo test -p djls-db source_files
cargo check -p djls-db --all-targets
```

Expected success criteria:

- `source_files.rs` contains no concrete Salsa mutation of `Project.source_inventory`.
- `djls-db` is the only production code applying `SourceFileInventory` to the Project input.
- Tests prove previous ready facts are preserved on terminal failures.

## Phase 3: Replace loading plan/effects with Django Discovery Run

### Files

- `crates/djls-project/src/discovery_run.rs`
- delete old `crates/djls-project/src/loading/driver.rs`
- delete old `crates/djls-project/src/loading/effects.rs`
- delete old `crates/djls-project/src/loading/plan.rs`
- `crates/djls-project/src/lib.rs`
- `crates/djls-project/src/python/source.rs` tests using fake loading effects

### Edits

1. Implement `DjangoDiscoveryRequest`.

   ```rust
   pub struct DjangoDiscoveryRequest {
       workspace_roots: Vec<Utf8PathBuf>,
       client_settings: Settings,
   }
   ```

   Provide constructor and accessors.

2. Implement discovery result/status types using new names.
   - Rename node result to stage result.
   - Rename milestone result to discovery milestone result.
   - Remove `LoadingPlan` entirely.
   - Replace the plan node list with an internal constant:
     ```rust
     const DISCOVERY_STAGES: &[DiscoveryStage] = &[ ... ];
     ```
   - Keep milestone prerequisite policy as internal static data.

3. Implement `DiscoveryHost` with the target contract.

4. Implement `run_django_discovery(request, host, observer)`.
   - The driver owns fixed stage order.
   - The driver owns status projection.
   - The driver owns milestone advancement.
   - It calls `host.checkpoint()` before and after expensive operations.
   - It maps `DiscoveryCancellation::Superseded` to `DiscoveryExecutionOutcome::Superseded` in the run result.

5. Move repeated domain choreography from server/CLI into the driver.
   Source-files stage should:
   - build source roots from `request.workspace_roots`;
   - create first-party file request;
   - call `host.load_files_for_roots(request)`;
   - construct first-party patch internally;
   - read `host.current_source_files()`;
   - merge to `SourceFilesUpdate` internally;
   - call `host.apply_source_files(update)`.

   Project-root-discovery stage should:
   - build root paths from `request.workspace_roots` through the source-root builder;
   - call `load_project_root_discovery(ProjectRootDiscoveryLoadRequest::new(...))`;
   - call `host.apply_project_root_discovery(update)`.

   Installed-app/template-directory stages should:
   - use project-owned functions to compute roots/outcomes;
   - call `host.load_files_for_roots` for file walking;
   - construct partitioned patches internally;
   - merge with `host.current_source_files()` internally;
   - call `host.apply_source_files(update)` for each partition update.

   Enrichment stage should:
   - call `host.load_project_enrichment()`;
   - checkpoint after load;
   - call `host.apply_project_enrichment(enrichment)`.

6. Status projection should use `DiscoveryReadiness` or equivalent internal trait, not `LoadingReadiness`.
   - `SourceFilesApplyResult` maps to stage status.
   - `ProjectRootDiscoveryApplyResult` maps to stage status.
   - `PythonSourceIndexOutcome`, `DjangoEnvironmentCandidatesOutcome`, and `ProjectEnrichment` map to stage status.

7. Update discovery-run tests.
   - Fixed stage order test uses `run_django_discovery`, not `LoadingPlan::phase3()`.
   - Milestone tests use `DiscoveryMilestone` and `DiscoveryStageStatus`.
   - Tests should prove there is no plan input.
   - Tests should prove `WorkspaceReady` and `DjangoAppsReady` milestone policy still matches current behavior.
   - Tests should prove `Enrichment` runs last and is not a milestone prerequisite.
   - Tests should prove cancellation before/after file load aborts without apply.

### Verification

```bash
cargo test -p djls-project discovery_run
cargo check -p djls-project --all-targets
```

Expected success criteria:

- `rg "LoadingPlan|LoadingEffects|run_loading_plan|phase3|NodeId|MilestoneId|NodeTerminalStatus|MilestoneTerminalStatus" crates/djls-project crates/djls-server crates/djls` returns no code matches.
- No public or private compatibility wrappers preserve old loading names.
- `run_django_discovery` has no plan parameter.

## Phase 4: Update CLI and LSP hosts

### Files

- `crates/djls/src/loading.rs`
- `crates/djls/src/commands/check.rs`
- `crates/djls-server/src/startup.rs`
- `crates/djls-server/src/session.rs`
- `crates/djls-server/src/server.rs`

### Edits

1. Rename `CliLoadingExecutor` to `CliDiscoveryHost` or `CliDiscoveryExecutor`.
   - Prefer `CliDiscoveryHost` because it implements `DiscoveryHost`.
   - Remove domain choreography from it.
   - Implement only host callbacks:
     - `checkpoint` returns `Ok(())`.
     - `load_files_for_roots` calls `djls_workspace::load_files_for_roots`.
     - `current_source_files` reads `ProjectDb::project(self.db).source_inventory(self.db).ready()`.
     - `apply_source_files` calls DB source-file apply method.
     - `apply_project_root_discovery` calls DB root-discovery apply method.
     - observation callbacks run tracked queries on the DB.
     - enrichment callbacks call DB enrichment methods.

2. Update CLI check flow.
   - Replace `run_loading_plan(LoadingPlan::phase3(), ...)` with:
     ```rust
     run_django_discovery(
         DjangoDiscoveryRequest::new(roots, db.settings()),
         &mut host,
         &mut NoopDiscoveryObserver,
     )
     ```
   - Preserve existing strict/fatal CLI behavior around config and command failure.

3. Rename `LspLoadingExecutor` to `LspDiscoveryHost` or equivalent.
   - Remove root/request/merge/project-helper choreography.
   - Implement host callbacks only.
   - `checkpoint` returns `Err(DiscoveryCancellation::Superseded)` if generation is no longer current.
   - `load_files_for_roots` checks generation before starting; call `djls_workspace::load_files_for_roots`; driver checkpoints after.
   - `current_source_files` must read from the live session or an appropriate snapshot without holding locks longer than needed.
   - `apply_source_files` uses `GenerationGuard::apply` and performs stale snapshot rejection before applying; map rejected stale snapshot to `DiscoveryExecutionOutcome::StaleSnapshot`.
   - Observation methods continue to use `project_db_snapshot_for_observation` and return only `DiscoveryObservationOutcome::Cancelled(Superseded)` when superseded.
   - `load_project_enrichment` returns `Result<ProjectEnrichment, DiscoveryCancellation>`.

4. Update `StartupRunOutcome` mapping.
   - `DiscoveryExecutionOutcome::Superseded` maps to startup superseded.
   - `DiscoveryExecutionOutcome::StaleSnapshot` maps to failed/rejected apply as current behavior requires.
   - Preserve current progress finish behavior.

5. Update startup progress types/events.
   - `NodeStarted` becomes stage-started wording where appropriate.
   - Use `DiscoveryStage` and `DiscoveryStageStatus`.
   - Use `DiscoveryMilestone` and `DiscoveryMilestoneStatus`.
   - User-visible progress text should still be readable; do not expose internal rename churn if not currently user-visible.

6. Update LSP startup tests.
   - Replace node/status names.
   - Preserve tests proving requests can proceed while discovery is running.
   - Preserve stale snapshot rejection behavior.
   - Add/adjust tests for cancellation around file load callback if current gates cover it.

### Verification

```bash
cargo test -p djls --test check
cargo test -p djls-server startup
cargo check -p djls-server --all-targets
```

Expected success criteria:

- CLI and LSP hosts no longer import source-root builders, merge helpers, or patch builders from `djls-project`.
- CLI/LSP host code reads as runtime policy only.
- Startup generation/stale-snapshot behavior remains tested.

## Phase 5: Update semantic/IDE/project consumers for renamed Project Facts

### Files

- `crates/djls-project/src/apps.rs`
- `crates/djls-project/src/environments.rs`
- `crates/djls-project/src/resolver.rs`
- `crates/djls-project/src/settings/candidates.rs`
- `crates/djls-project/src/settings/composition.rs`
- `crates/djls-project/src/templates/inventory.rs`
- `crates/djls-project/src/python/inventory.rs`
- `crates/djls-project/src/python/source.rs`
- `crates/djls-semantic/src/**/*.rs`
- `crates/djls-ide/src/**/*.rs`

### Edits

1. Update all `project.discovery(db)` references to `project.root_discovery(db)`.

2. Update `ProjectDiscovery::*` matches to `ProjectRootDiscovery::*`.

3. Update source inventory names throughout query code.
   - `ProjectSourceInventory::Ready` -> `SourceFileInventory::Ready`.
   - `ReadyProjectSourceFiles` -> `ReadySourceFiles`.
   - `ProjectSourceFilesIssue` -> `SourceFilesIssue`.

4. Ensure semantic template-resolution cleanup remains intact.
   - `djls-semantic` should continue to consume `djls_project::template_files` and construct semantic `Template` at tracked use sites.
   - Do not reintroduce `discover_templates` or crate-root `resolve_static_template` exports.

5. Update tests and fixtures to use new names.
   - Avoid importing from crate-root when inside `djls-project`; use owning modules internally.
   - External crates import from crate-root curated API.

### Verification

```bash
cargo test -p djls-project
cargo test -p djls-semantic resolution
cargo test -p djls-ide
cargo check --all-targets
```

Expected success criteria:

- No old Project Discovery or Project Source Files type names remain in code.
- Semantic/IDE behavior remains unchanged.
- Internal project code imports from owning modules.

## Phase 6: Delete obsolete loading module and stale exports

### Files

- `crates/djls-project/src/loading.rs`
- `crates/djls-project/src/loading/*`
- `crates/djls-project/src/lib.rs`
- docs that describe the current code path

### Edits

1. Delete `crates/djls-project/src/loading.rs` and `crates/djls-project/src/loading/` after all code has moved.

2. Clean crate-root exports.
   - Remove all obsolete loading exports.
   - Ensure exported API is curated and intentional.
   - Keep no `pub mod` declarations.

3. Run search checks:

   ```bash
   rg "loading|Loading|phase3|run_loading_plan|LoadingPlan|LoadingEffects" crates/djls-project crates/djls-server crates/djls crates/djls-db crates/djls-semantic crates/djls-ide
   ```

   Accept only literal low-level file-loading uses where “load” is truthful, such as `load_files_for_roots`, config/env load, or runtime enrichment load. No generic discovery-run scaffolding should remain.

4. Update `ARCHITECTURE.md` if implementation names changed from the current architecture prose.
   - Replace generic loading terminology with Django Discovery Run where describing the startup process.
   - Name `djls-db` as the concrete Salsa materialization boundary.
   - Keep `CONTEXT.md` glossary free of implementation details; it already has the relevant terms.

5. Update the new research/current inventory doc if needed.
   - `docs/agents/startup-rethink/current-architecture-inventory.md` was a snapshot before this cleanup; either leave it as historical or add a short note pointing to this plan and the new code shape.
   - Do not silently rewrite old historical evidence as if it described the new state.

### Verification

```bash
cargo check --all-targets
just fmt --check
```

Expected success criteria:

- The old loading module is gone.
- The crate-root API is explicit and curated.
- No false old names remain in live architecture docs.

## Phase 7: Review and full validation

### Automated checks

Run:

```bash
cargo check --all-targets
cargo test -p djls-project
cargo test -p djls-db
cargo test -p djls-semantic resolution
cargo test -p djls-ide
cargo test -p djls --test check
cargo test -p djls-server startup
just fmt --check
cargo clippy --all-targets --all-features --benches -- -D warnings
```

If a test fails, fix it before continuing. Do not classify failures as unrelated.

### Manual review checklist

- `djls-project` owns Django Discovery Run sequencing.
- `djls-project` owns Source File Inventory decisions, but not concrete DB mutation.
- `djls-db` owns Salsa materialization and setters.
- `djls-server` owns LSP runtime policy only.
- `djls` CLI owns CLI runtime policy only.
- No old generic loading plan/effects API remains.
- No compatibility aliases preserve old names.
- No `pub mod` was introduced in `djls-project`.
- Cross-crate API in `djls-project/src/lib.rs` is broad only where the seam is truly broad.
- Runtime enrichment remains optional and does not gate `WorkspaceReady` or `DjangoAppsReady`.
- `SemanticDb::template_libraries()` migration seam was not made worse.

### Optional implementation review

After validation, ask for an adversarial implementation review focused on:

- Ousterhout: module depth and leakage.
- Lamport: Source File Inventory transition invariants and discovery cancellation/apply ordering.
- Grug: needless wrappers, over-abstracted names, or compatibility sludge.

## Final success criteria

The PR is ready to commit/push when all of these are true:

1. `LoadingPlan`, `LoadingEffects`, `run_loading_plan`, `phase3`, `NodeId`, and milestone/node terminal old names are gone from live code.
2. `run_django_discovery` is the single discovery-run entrypoint.
3. The fake plan abstraction is removed; stage order is internal to the run.
4. CLI and LSP hosts no longer duplicate source-root/request/merge choreography.
5. Source File Inventory decisions are computed in `djls-project`; inventory mutation happens in `djls-db`.
6. Project Root Discovery naming is used consistently for the per-root discovery Project Fact.
7. `djls-project` has no public modules and no accidental crate-root convenience facade.
8. Full validation passes.
