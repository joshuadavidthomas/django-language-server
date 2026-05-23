# Implementation Notes: startup-rethink

## Summary
- Current status: in progress
- Current phase: Phase 1 complete; Phase 2 next
- Diff scope: split Project, Source File Inventory, and Project Root Discovery out of the old loading module; renamed project fact types across crates.

## Phase progress
- Phase 1 — complete, evidence: `cargo check -p djls-project --all-targets`, `cargo check --all-targets`, and `just fmt --check` passed after the split/rename.

## Divergences
### Divergence: loading module remains for run orchestration until Phase 3
- Planned: Phase 1 said to replace `mod loading;` with the new modules outright.
- Found: `crates/djls-project/src/loading/driver.rs`, `effects.rs`, and `plan.rs` still back the current CLI/LSP startup path until Phase 3 replaces them with `discovery_run.rs`.
- Decision: keep `mod loading;` private for the remaining legacy run orchestration, but move Project, Source File Inventory, and Project Root Discovery state/update code out to `project.rs`, `source_files.rs`, and `root_discovery.rs` now.
- Why this remains in scope: it preserves the phase boundary without keeping the old project-fact shapes. Phase 3 still owns deleting `LoadingPlan`, `LoadingEffects`, and the old loading module.
- Verification impact: Phase 3 must still satisfy the search check for `LoadingPlan|LoadingEffects|run_loading_plan|phase3|NodeId|MilestoneId|NodeTerminalStatus|MilestoneTerminalStatus`.

## Checks run during implementation
- `cargo check -p djls-project --all-targets` — passed, proves the split project crate compiles in tests and library targets.
- `cargo check --all-targets` — passed, proves cross-crate renames did not break CLI, DB, semantic, IDE, or server targets.
- `just fmt --check` — passed after running `just fmt`.
- `rg "ProjectSourceInventory|ReadyProjectSourceFiles|ProjectSourceFilesIssue|ProjectDiscovery|RootDiscoveryData|ProjectDiscoverySetData|ProjectDiscoveryLoadRequest|build_project_discovery_data|set_project_source_inventory|set_project_discovery" crates/djls-project crates/djls-db crates/djls-server crates/djls crates/djls-semantic crates/djls-ide` — no matches.
- `rg "use crate::(ProjectRootDiscovery|ProjectRootDiscoverySet|ProjectRootDiscoveryIssue|ProjectRootDiscoveryIssues|SourceFileInventory|ReadySourceFiles|SourceFilesIssue|SourceFilesApplyResult|SourceFilesUpdate|Project);" crates/djls-project/src -g '*.rs'` — no matches.

## Handoff notes
- Phase 2 should start in `crates/djls-project/src/source_files.rs` by replacing `finalize_project_source_files` with `SourceFilesUpdate::decide_apply(...)` and a private-field `SourceFilesApplyDecision`.
- `crates/djls-db/src/db.rs::apply_source_files` still calls `djls_project::finalize_project_source_files(...)`; this is the Phase 2 mutation seam to replace.
