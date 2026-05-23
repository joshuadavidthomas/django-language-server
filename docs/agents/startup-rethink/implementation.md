# Implementation Notes: startup-rethink

## Summary
- Current status: complete pending commit
- Current phase: Phase 7 complete
- Diff scope: split Project, Source File Inventory, and Project Root Discovery out of the old loading module; renamed project fact types across crates; moved source-file apply decisions into `djls-project` without concrete Salsa mutation; replaced the old loading plan/effects with Django Discovery Run and runtime-only CLI/LSP discovery hosts.

## Phase progress
- Phase 1 — complete, evidence: `cargo check -p djls-project --all-targets`, `cargo check --all-targets`, and `just fmt --check` passed after the split/rename.
- Phase 2 — complete, evidence: `cargo test -p djls-project source_files`, `cargo test -p djls-db source_files`, `cargo test -p djls-db`, `cargo check -p djls-db --all-targets`, `cargo check --all-targets`, and `just fmt --check` passed.
- Phase 3 — complete, evidence: old loading plan/effects names are gone from live code; `cargo test -p djls-project discovery_run` and `cargo check --all-targets` passed.
- Phase 4 — complete, evidence: CLI/LSP hosts no longer import project source-root builders, merge helpers, or patch builders; `cargo test -p djls-server startup`, `cargo test -p djls --test check`, `cargo check --all-targets`, and `just fmt --check` passed.
- Phase 5 — complete, evidence: old Project Discovery/Project Source Files names are absent; `cargo test -p djls-project`, `cargo test -p djls-semantic resolution`, and `cargo test -p djls-ide` passed.
- Phase 6 — complete, evidence: old loading module files are deleted; crate-root source-file/root-discovery helper exports are removed; `cargo check --all-targets`, targeted source/discovery/server/DB tests, and `just fmt --check` passed.
- Phase 7 — complete, evidence: Ousterhout and Lamport final reviews found no must-fix blockers; `cargo check --all-targets`, targeted plan tests, `cargo clippy --all-targets --all-features --benches -- -D warnings`, `just clippy --allow-dirty`, `cargo test --all-targets`, `just fmt --check`, `just lint`, and `just test` passed.

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

### Divergence: Phase 3 and Phase 4 landed together
- Planned: Phase 3 would introduce Django Discovery Run, then Phase 4 would update CLI/LSP hosts.
- Found: replacing `LoadingEffects` without updating the only production hosts would leave the workspace in a non-compiling transitional state.
- Decision: implement `DiscoveryHost` and update `CliDiscoveryHost`/`LspDiscoveryHost` in the same working change.
- Why this remains in scope: it avoids a compatibility bridge and moves directly to the requested end-state.
- Verification impact: ran both Phase 3 and Phase 4 checks before recording completion.

### Divergence: installed-app/template directory discovery uses typed root discoveries
- Planned: the target host sketch included direct stage callbacks for loading installed-app and template-directory file patches.
- Found: that kept Django choreography in the runtime hosts or pushed toward a rejected status/request cross-product. Ousterhout and Lamport review flagged the raw request/status shape as leaky and weak on invariants.
- Decision: add project-owned `InstalledAppFileRootsOutcome` and `TemplateDirectoryFileRootsOutcome` observations. Their ready payloads have private fields and own request lowering/result-to-update conversion. The discovery driver observes roots, asks the host only to walk files, then applies one stage-level `SourceFilesUpdate`.
- Why this remains in scope: `djls-project` owns sequencing, root/predicate/partition policy, and Source File Inventory update construction; CLI/server only own cancellation, walking, observation, and apply callbacks.
- Verification impact: discovery-run tests prove stage order, milestones, enrichment-last behavior, and cancellation before/after file load; server/CLI tests prove host integration.

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
- `cargo test -p djls-project discovery_run` — passed, proves Django Discovery Run stage/milestone/cancellation behavior.
- `cargo test -p djls-project source_files` — passed after stage-level partition patch-set changes.
- `cargo test -p djls-server startup` — passed, proves LSP startup generation, stale snapshot, progress, and non-blocking observation behavior.
- `cargo test -p djls --test check` — passed, proves CLI check integration.
- `cargo check --all-targets` — passed after Phase 3/4 host integration.
- `rg "LoadingPlan|LoadingEffects|run_loading_plan|phase3|NodeId|MilestoneId|NodeTerminalStatus|MilestoneTerminalStatus" crates/djls-project crates/djls-server crates/djls` — no matches.
- `rg "build_source_roots|first_party_discovery_files_request|first_party_source_files_load_request|merge_first_party_source_file_patch|merge_partitioned_source_file_patch|FirstPartySourceFilePatch|PartitionedSourceFilePatch" crates/djls/src crates/djls-server/src -g '*.rs'` — no matches.

## Handoff notes
- Final audit should confirm the committed change ID and that the worktree is clean after commit.
- `SourceFilesApplyDecision` now has private fields and no public constructor; `DjangoDatabase::apply_source_files` is the production mutation boundary for applying `decision.next_inventory()`.
- `djls-project` no longer has public modules; test helpers are curated crate-root exports behind the `testing` cfg/feature instead of `pub mod testing`.
- `validate_template_file` falls back to runtime template libraries when project-specific libraries are unavailable, preserving diagnostics before or without project facts.
