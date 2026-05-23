# Implementation Notes: startup-rethink

## Summary
- Current status: in progress
- Current phase: Phase 2 complete; Phase 3 next
- Diff scope: split Project, Source File Inventory, and Project Root Discovery out of the old loading module; renamed project fact types across crates; moved source-file apply decisions into `djls-project` without concrete Salsa mutation.

## Phase progress
- Phase 1 — complete, evidence: `cargo check -p djls-project --all-targets`, `cargo check --all-targets`, and `just fmt --check` passed after the split/rename.
- Phase 2 — complete, evidence: `cargo test -p djls-project source_files`, `cargo test -p djls-db source_files`, `cargo test -p djls-db`, `cargo check -p djls-db --all-targets`, `cargo check --all-targets`, and `just fmt --check` passed.

## Divergences
### Divergence: loading module remains for run orchestration until Phase 3
- Planned: Phase 1 said to replace `mod loading;` with the new modules outright.
- Found: `crates/djls-project/src/loading/driver.rs`, `effects.rs`, and `plan.rs` still back the current CLI/LSP startup path until Phase 3 replaces them with `discovery_run.rs`.
- Decision: keep `mod loading;` private for the remaining legacy run orchestration, but move Project, Source File Inventory, and Project Root Discovery state/update code out to `project.rs`, `source_files.rs`, and `root_discovery.rs` now.
- Why this remains in scope: it preserves the phase boundary without keeping the old project-fact shapes. Phase 3 still owns deleting `LoadingPlan`, `LoadingEffects`, and the old loading module.
- Verification impact: Phase 3 must still satisfy the search check for `LoadingPlan|LoadingEffects|run_loading_plan|phase3|NodeId|MilestoneId|NodeTerminalStatus|MilestoneTerminalStatus`.

### Divergence: materialization comparison uses a domain snapshot, not `SourceFileSetData`
- Planned: remove `&mut dyn Db` from source-file apply decisions while preserving materialization mismatch checks.
- Found: comparing a Salsa `SourceFileSet` directly would require a database read. A first draft added a parallel `source_file_set_data` field, but that exposed storage mechanics and duplicated names.
- Decision: keep `SourceFileSetMaterialized` as the DB/project seam, but pass roots and discovered files into its constructor and store a private `DiscoveredSourceFiles` snapshot for comparison. No `SourceFileSetData` twin crosses the decision API.
- Why this remains in scope: it preserves the planned boundary: `djls-project` decides with domain values, while `djls-db` still owns Salsa materialization.
- Verification impact: source-file tests cover mismatched materialization with and without previous ready facts.

## Checks run during implementation
- `cargo check -p djls-project --all-targets` — passed, proves the split project crate compiles in tests and library targets.
- `cargo check --all-targets` — passed, proves cross-crate renames did not break CLI, DB, semantic, IDE, or server targets.
- `just fmt --check` — passed after running `just fmt`.
- `rg "ProjectSourceInventory|ReadyProjectSourceFiles|ProjectSourceFilesIssue|ProjectDiscovery|RootDiscoveryData|ProjectDiscoverySetData|ProjectDiscoveryLoadRequest|build_project_discovery_data|set_project_source_inventory|set_project_discovery" crates/djls-project crates/djls-db crates/djls-server crates/djls crates/djls-semantic crates/djls-ide` — no matches.
- `rg "use crate::(ProjectRootDiscovery|ProjectRootDiscoverySet|ProjectRootDiscoveryIssue|ProjectRootDiscoveryIssues|SourceFileInventory|ReadySourceFiles|SourceFilesIssue|SourceFilesApplyResult|SourceFilesUpdate|Project);" crates/djls-project/src -g '*.rs'` — no matches.
- `cargo test -p djls-project source_files` — passed, proves source-file decision transitions.
- `cargo test -p djls-db source_files` — passed, requested narrow DB filter.
- `cargo test -p djls-db` — passed, proves all DB materialization/apply tests after the decision split.
- `cargo check -p djls-db --all-targets` — passed, proves DB target integration.
- `rg "set_source_file_inventory|set_source_inventory" crates/djls-project/src/source_files.rs` — no matches.

## Handoff notes
- Phase 3 should replace `crates/djls-project/src/loading/{driver,effects,plan}.rs` with `discovery_run.rs` and remove `LoadingPlan`, `LoadingEffects`, `run_loading_plan`, and node/milestone legacy names.
- `SourceFilesApplyDecision` now has private fields and no public constructor; `DjangoDatabase::apply_source_files` is the production mutation boundary for applying `decision.next_inventory()`.
