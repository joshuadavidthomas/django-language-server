# Issue payload inventory and simplification candidates

This note inventories the `Issue`-style payloads in the current `startup-rethink` stack. It focuses on payloads attached to resolved, ready, inventory, or outcome structs where the outer state often already decides behavior.

The purpose is diagnostic. No simplification is decided here; the rough plan at the end records likely follow-up directions to expand before editing.

Line numbers refer to the current working tree at the time this note was written.

## Reading frame

The pattern to watch for is:

- an outer state drives behavior, such as `Ready`, `Unavailable`, `Deferred`, `Selected`, `Applied`, or `Unindexed`;
- an inner `Issue` enum records a reason;
- downstream code either ignores the reason, tests only that a reason exists, or forwards it without interpreting it.

That shape can be useful when the reason is user-facing, affects stage status, crosses a real boundary, or protects an invariant. It is suspect when it only preserves optimistic/debug detail that no behavior uses.

`InstalledAppResolution` was the comparison point: production code needed the resolved package/AppConfig shape, but the detailed unresolved variants were mostly collapsed into a generic gap or skipped.

## Summary table

| Payload | Location | Attached to | Current usage | Simplification pressure |
|---|---:|---|---|---|
| `WorkspaceRootIssue` | `crates/djls-workspace/src/file_loader.rs:82` | `FilesForRootsResult.root_issues` | Workspace adapter result, mapped into project issues. | Low; real boundary payload. |
| `ProjectRootDiscoveryIssue` | `crates/djls-project/src/root_discovery.rs:175` | `RootDiscoveryInput.issues`, `ProjectRootDiscoveryIssues` | Presence makes root discovery degraded/unavailable; variants mostly carried. | Medium; prune unconstructed variants first. |
| `InterpreterDiscoveryIssueKind` | `crates/djls-project/src/root_discovery.rs:220` | `ProjectRootDiscoveryIssue::InterpreterDiscoveryFailed` | No construction found. | High; likely dead/speculative. |
| `EnvFileLoadIssueKind` | `crates/djls-project/src/root_discovery.rs:227` | `ProjectRootDiscoveryIssue::EnvFileLoadFailed` | Constructed by env-file loading; carried as detail. | Low/medium. |
| `SourceFilesIssue` | `crates/djls-project/src/source_files.rs:88` | source inventory, patches, updates, readiness, apply results | Core readiness/apply issue type. Some variants drive behavior. | Low overall; prune dead variants. |
| `SourceFileMaterializationIssue` | `crates/djls-project/src/source_files.rs:847` | `SourceFileSetMaterialized.issues` | `djls-db` materialization error payload, immediately converted to `SourceFilesIssue`. | Medium; maybe collapse into seam-local mapping. |
| `ProjectLayoutIssue` | `crates/djls-project/src/layout.rs:19` | `ProjectLayoutIndexOutcome`, `SettingsModuleCandidatesOutcome` | Forwarded to settings candidates; outer outcome drives behavior. | Medium. |
| `SettingsCandidateIssue` | `crates/djls-project/src/settings/candidates.rs:82` | `SettingsCandidateOutcome::Ready { issues }` | Presence affects environment outcome/degraded status; variants not matched. | Medium. |
| `EnvironmentCandidatesIssue` | `crates/djls-project/src/environments.rs:83` | `DjangoEnvironmentCandidatesOutcome` | Outer outcome/status drives behavior; some variants forwarded. | Medium/high for dead variants. |
| `EnvironmentSelectionIssue` | `crates/djls-project/src/environments.rs:103` | `EnvironmentSelection::{Unknown, Ambiguous}` | Forwarded into semantic template lookup; not interpreted downstream. | Medium/high after template lookup simplification. |
| `SettingsIssue` | `crates/djls-project/src/settings/composition.rs:91` | `DjangoSettings.issues`, `TemplateSettingsResolution.issues`, `PartialListSegment.issue` | Segment issues drive unknown settings-dir/app entries; top-level fields look mostly unobserved. | Medium/high. |
| `PythonSourceIndexIssue` | `crates/djls-project/src/python/source.rs:122` | `PythonSourceIndexOutcome::Unindexed` | Variants map to discovery status. | Low; behavior-driving. |
| `ModuleNameIssue` | `crates/djls-project/src/python/source.rs:135` | `PyModuleNameResolution::Unknown` | Mostly diagnostic/test-facing; production cares resolved vs unknown. | High. |
| `StaticValueIssue` | `crates/djls-project/src/python/source.rs:344` | `StaticValue::Unknown`, `StaticValueSegment.issue` | Internal provenance for settings/static extraction. | Medium/high if settings issues collapse. |
| `TemplateTagLibraryIssue` | `crates/djls-project/src/templates/inventory.rs:153` | `TemplateTagLibraryResolution::{Unresolved, Ambiguous}` | Consumers skip unresolved/ambiguous; inner issue detail unused. | High; close to old unresolved resolution detail. |
| `ProjectEnrichmentIssue` | `crates/djls-project/src/enrichment.rs:21` | `ProjectEnrichment::Unresolved` | Top-level variants affect stage status; nested details mostly carried. | Low/medium; prune unconstructed nested variants. |
| `TemplateLookupIssue` | `crates/djls-semantic/src/resolution.rs:14` | `TemplateLookupResult::Deferred` | IDE callers ignore reason; public payload. | Medium/high, but public API consideration. |
| `TemplateInventoryIssue` | `crates/djls-semantic/src/resolution.rs:21` | `TemplateLookupIssue::Inventory` | Relabels template directory states; no downstream variant behavior found. | High if `TemplateLookupIssue` changes. |

## Detailed inventory

### `WorkspaceRootIssue`

Location: `crates/djls-workspace/src/file_loader.rs:82`.

Attached to: `FilesForRootsResult.root_issues`.

Produced by: `load_files_for_roots` when a requested source root is missing or unreadable.

Consumed by: `djls-project::source_files` maps it into `SourceFilesIssue` at the workspace/project boundary.

Assessment: this is a real anti-corruption boundary. Workspace owns filesystem preflight facts; project maps them into domain readiness. Keep unless the whole file-loading result shape changes.

### `ProjectRootDiscoveryIssue`

Location: `crates/djls-project/src/root_discovery.rs:175`.

Attached to:

- `RootDiscoveryInput.issues`;
- `RootDiscoveryUpdate.issues`;
- `ProjectRootDiscoveryIssues`;
- `ProjectRootDiscovery::Unavailable { issues }`.

Produced by: config loading, env-file loading, duplicate env-var resolution, no-workspace-root fallback, and fixtures.

Consumed by: `djls-db` materializes issues onto Salsa inputs and computes `has_issues`; discovery maps any issues on ready roots to a degraded root-discovery stage.

Assessment: the collection is behavior-relevant by presence. Most variants are carried detail rather than behavior-driving. Keep the concept, but prune dead variants and consider whether ready roots need rich issue payloads or only degraded state plus reporting.

### `InterpreterDiscoveryIssueKind`

Location: `crates/djls-project/src/root_discovery.rs:220`.

Attached to: `ProjectRootDiscoveryIssue::InterpreterDiscoveryFailed`.

Produced by: no construction found in the current tree.

Consumed by: no behavior found.

Assessment: likely speculative. It looks like future-facing interpreter diagnostics, but interpreter discovery currently returns an `Interpreter` shape rather than this issue. Candidate for deletion with `InterpreterDiscoveryFailed` unless upcoming work needs it immediately.

### `EnvFileLoadIssueKind`

Location: `crates/djls-project/src/root_discovery.rs:227`.

Attached to: `ProjectRootDiscoveryIssue::EnvFileLoadFailed`.

Produced by: `load_env_file_outcome` for configured missing env files, parse failures, and I/O failures.

Consumed by: root discovery carries it; discovery status uses issue presence, not the specific kind.

Assessment: plausible diagnostic payload. It does not currently affect behavior by variant. Keep if root discovery issues are intended to become user-facing; otherwise it could collapse to an env-file issue without a nested kind.

### `SourceFilesIssue`

Location: `crates/djls-project/src/source_files.rs:88`.

Attached to:

- `SourceFileInventory::Unavailable { issue }`;
- `SourceRootsPlan.issues`;
- source-file patch/update/apply structs;
- `SourceFilePartitionReadiness`;
- `SourceFilesApplyResult`;
- `SourceFilesApplied.issues`.

Produced by: root construction, workspace root issue mapping, source-file materialization mapping, partition conflict detection, installed app gaps, fixtures.

Consumed by:

- `SourceFilesIssue::NotLoaded` maps to deferred source inventory / Python source indexing;
- `SourceFilesIssue::PartitionConflict` is specifically tolerated by `first_fatal_update_issue`;
- other variants usually become readiness/result payloads.

Assessment: this is a core domain issue type. Do not collapse wholesale. Prune unused variants such as `TemplateDirectoryGap`, and keep checking whether `SourceFilesApplied.issues` is useful outside tests/fake hosts.

### `SourceFileMaterializationIssue`

Location: `crates/djls-project/src/source_files.rs:847`.

Attached to: `SourceFileSetMaterialized.issues`.

Produced by: `djls-db` maps `SourceFileSetData` invariant errors into this enum.

Consumed by: `SourceFilesUpdate::decide_apply` converts the first materialization issue into a `SourceFilesIssue` and fails the apply decision.

Assessment: this is a narrow database/project seam. It is defensible because `djls-db` owns Salsa materialization while `djls-project` owns apply decisions. It is also a candidate for simplification if the seam can return `SourceFilesIssue` directly without leaking storage mechanics into project decisions.

### `ProjectLayoutIssue`

Location: `crates/djls-project/src/layout.rs:19`.

Attached to:

- `ProjectLayoutIndexOutcome::{Absent, Unavailable}`;
- `SettingsModuleCandidatesOutcome::LayoutUnavailable`;
- `SettingsCandidateIssue::LayoutUnavailable`.

Produced by: project layout indexing when source inventory is absent or unavailable.

Consumed by: settings candidate discovery forwards it. Behavior is driven by outer outcome, not by matching specific layout issue variants outside layout tests.

Assessment: plausible but pass-through. Could simplify if settings candidates only need “layout unavailable” rather than nested reason detail. Keep if root/source availability reasons will be surfaced.

### `SettingsCandidateIssue`

Location: `crates/djls-project/src/settings/candidates.rs:82`.

Attached to: `SettingsCandidateOutcome::Ready { candidates, issues }`.

Produced by: layout unavailable and invalid configured/env module names.

Consumed by: environment candidate discovery checks whether issues exist and wraps them in `EnvironmentCandidatesIssue`. Specific variants are not matched in production.

Assessment: presence is behavior-relevant; variant detail is mostly reporting/provenance. Candidate for simplification if invalid module names do not need structured reporting.

### `EnvironmentCandidatesIssue`

Location: `crates/djls-project/src/environments.rs:83`.

Attached to: `DjangoEnvironmentCandidatesOutcome`.

Produced by: no settings candidates, settings candidate issues, unavailable settings candidates. `AmbiguousSettingsCandidates` appears unconstructed.

Consumed by: discovery status uses the outer `DjangoEnvironmentCandidatesOutcome`; environment selection wraps unavailable/deferred issues; semantic lookup can receive them indirectly.

Assessment: the outcome enum is useful. Some inner variants are pass-through or dead. `AmbiguousSettingsCandidates` is a deletion candidate unless ambiguity is implemented next.

### `EnvironmentSelectionIssue`

Location: `crates/djls-project/src/environments.rs:103`.

Attached to: `EnvironmentSelection::{Unknown, Ambiguous}`.

Produced by: no candidate for file, multiple candidates for file, candidate discovery unavailable/deferred. `NoEnvironmentCandidates` appears unconstructed.

Consumed by: semantic template resolution wraps selection issues in `TemplateLookupIssue::Environment`; IDE callers ignore deferred reasons.

Assessment: mostly forwarded diagnostic detail. Candidate for simplification after deciding whether `TemplateLookupResult::Deferred` should expose structured reasons.

### `SettingsIssue`

Location: `crates/djls-project/src/settings/composition.rs:91`.

Attached to:

- `DjangoSettings.issues`;
- `TemplateSettingsResolution.issues`;
- `PartialListSegment.issue`.

Produced by: environment lookup failures, settings module resolution failures, parse failures, unsupported static values, unsupported list operations, unresolved imports.

Consumed by:

- `PartialListSegment.issue` is used by template directory inventory to produce `UnknownSettingsDir`;
- installed app root discovery treats unknown segments as installed app gaps;
- top-level `DjangoSettings.issues` and `TemplateSettingsResolution.issues` do not appear to have meaningful production consumers.

Assessment: mixed. The partial-list “unknown segment” concept is real, but the structured `SettingsIssue` variants leak static-evaluation mechanics upward. Candidate for reducing segment payloads to known/unknown unless user-facing settings diagnostics are planned.

### `PythonSourceIndexIssue`

Location: `crates/djls-project/src/python/source.rs:122`.

Attached to: `PythonSourceIndexOutcome::Unindexed`.

Produced by: source inventory unavailable, layout unavailable, or no Python files.

Consumed by: discovery maps variants to stage statuses: no Python files is skipped, not loaded is deferred, other unavailable states are unavailable.

Assessment: keep. This is a good example of issue variants carrying domain status differences that behavior uses.

### `ModuleNameIssue`

Location: `crates/djls-project/src/python/source.rs:135`.

Attached to: `PyModuleNameResolution::Unknown(ModuleNameIssue)` inside `PythonSourceModel`.

Produced by: non-Python files, paths outside import roots, invalid module names.

Consumed by: tests and diagnostic/debug shape. Production code generally cares whether a module name resolved.

Assessment: close to the old unresolved-resolution detail smell. Candidate for `PyModuleNameResolution::Unknown` or `Option<PyModuleName>` if the wrapper does not encode additional behavior.

### `StaticValueIssue`

Location: `crates/djls-project/src/python/source.rs:344`.

Attached to:

- `StaticValue::Unknown { issue }`;
- `StaticValueSegment.issue`.

Produced by: unsupported expressions, unsupported dictionary keys, spread/list elements, and expression-shape fallback.

Consumed by: settings composition maps static-value issues into `SettingsIssue`; tests assert unknown segments exist.

Assessment: useful inside static extraction, but probably too detailed once it leaves that boundary. Candidate for keeping internally while collapsing settings-facing payloads.

### `TemplateTagLibraryIssue`

Location: `crates/djls-project/src/templates/inventory.rs:153`.

Attached to: `TemplateTagLibraryResolution::{Unresolved, Ambiguous}`.

Produced by: resolving configured template library aliases.

Consumed by:

- `resolved_files` skips unresolved/ambiguous libraries;
- `loadable_template_libraries` skips unresolved/ambiguous libraries;
- no production code matches `NotFound` versus `Ambiguous` issue payloads.

Assessment: strongest match for the `InstalledAppResolution` unresolved-detail problem. The outer resolution state is useful; the inner issue payload is not currently behavior-driving. Candidate for replacing with payload-free `Unresolved` and `Ambiguous` variants.

### `ProjectEnrichmentIssue`

Location: `crates/djls-project/src/enrichment.rs:21`.

Attached to: `ProjectEnrichment::Unresolved`.

Produced by: runtime enrichment request construction and inspector execution.

Consumed by: discovery status distinguishes `InspectorFailed` from runtime unavailable/fixture unresolved.

Assessment: top-level variants are behavior-relevant. Nested details are mostly carried. `RuntimeUnavailableKind::DjangoImportFailed` appears unconstructed and is a likely deletion candidate.

### `TemplateLookupIssue`

Location: `crates/djls-semantic/src/resolution.rs:14`.

Attached to: `TemplateLookupResult::Deferred { issue }`.

Produced by: environment selection failure, template inventory unready/unavailable/stale, invalid template names.

Consumed by: IDE hover/navigation callers currently ignore the reason and return `None`. Tests generally assert `Deferred { .. }` rather than a specific issue.

Assessment: public/debug payload more than behavior. Because it is exported by `djls-semantic`, simplification should be deliberate. Candidate for collapsing deferred reasons unless near-term diagnostics need them.

### `TemplateInventoryIssue`

Location: `crates/djls-semantic/src/resolution.rs:21`.

Attached to: `TemplateLookupIssue::Inventory`.

Produced by: `inventory_issue` maps `TemplateDirectoryEntry` states to `Deferred`, `Unavailable`, `Stale`, or `UnknownSettingsDir`.

Consumed by: no downstream per-variant behavior found.

Assessment: likely over-abstracted. It relabels project inventory states into semantic lookup states, then nobody interprets them. Candidate for removal with `TemplateLookupIssue` simplification.

## Rough simplification plan

### Pass 1: Delete dead or symmetric-only shapes

Status: implemented.

Start with changes that should not affect behavior:

- remove `TemplateDirectoryFileRoots.issues` if it is still always empty;
- remove `SourceFilesIssue::TemplateDirectoryGap` if still unconstructed;
- remove `ProjectRootDiscoveryIssue::InterpreterDiscoveryFailed` and `InterpreterDiscoveryIssueKind` if no construction is planned in this PR;
- remove `EnvironmentCandidatesIssue::AmbiguousSettingsCandidates` if ambiguity is still unimplemented;
- remove `EnvironmentSelectionIssue::NoEnvironmentCandidates` if still unconstructed;
- remove `RuntimeUnavailableKind::DjangoImportFailed` if still unconstructed.

Implementation notes:

- Removed all listed dead variants/types.
- Removed `TemplateDirectoryFileRoots.issues` and changed template-directory discovery status to derive from the source-file apply result only.
- Followed Hickey's advisory feedback by removing the dead `issues` parameter from `PartitionedSourceFilePatchSet::configured_template_directories(...)` instead of passing `Vec::new()` by convention.
- Validation after the main deletion pass: `cargo check --all-targets`, `just fmt --check`.

### Pass 2: Collapse unresolved-detail payloads that are only skipped

Status: implemented.

These are most like the old `InstalledAppResolution` unresolved variants:

- change `TemplateTagLibraryResolution::{Unresolved, Ambiguous}` to payload-free variants;
- consider changing `PyModuleNameResolution::Unknown(ModuleNameIssue)` to `Unknown`, or replacing the wrapper with `Option<PyModuleName>` if no invariant is lost.

Implementation notes:

- Removed `TemplateTagLibraryIssue`.
- Removed `TemplateTagLibraryResolution::{Unresolved, Ambiguous}` entirely instead of keeping payload-free skipped states.
- Skipped unresolved configured template-library aliases during template tag inventory construction.
- Removed the private `TemplateTagLibrarySource` taxonomy after Hickey flagged it as debug-only.
- Changed `PyModuleNameResolution::Unknown(ModuleNameIssue)` to payload-free `Unknown` and removed `ModuleNameIssue`.
- Addressed Hickey follow-up findings by deduplicating static loadable library names and template directory entries at inventory construction.
- Validation: `cargo check --all-targets`, `just fmt --check`.

### Pass 3: Reassess semantic deferred reasons

Status: implemented.

Decide whether `TemplateLookupResult::Deferred` needs a structured reason as public API.

If not:

- remove `TemplateLookupIssue` from the deferred result;
- remove `TemplateInventoryIssue`;
- keep only enough state to distinguish found, not found, and deferred.

If yes:

- define the user-facing reason set explicitly and stop forwarding project-internal issue enums through semantic lookup.

Implementation notes:

- Chose the simpler public shape: `TemplateLookupResult::Deferred` is now a unit variant.
- Removed `TemplateLookupIssue` and `TemplateInventoryIssue` from `djls-semantic`.
- Replaced semantic inventory reason mapping with `inventory_is_unready(...) -> bool`.
- Updated IDE callers and semantic tests to match the unit deferred variant.
- Addressed Hickey's must-fix finding by removing the remaining reason-shaped `Deferred { .. }` pattern from `TemplateLookupResult::ok`.
- Left invalid raw template names mapped to `Deferred`; Hickey flagged this as advisory because lookup callers already ignore deferred reasons. A later API cleanup could move raw-string parsing to the caller seam.
- Validation: `cargo check --all-targets`, `just fmt --check`.

### Pass 4: Simplify settings/static-value issue propagation

Status: implemented.

Separate internal extraction uncertainty from project-facing inventory state.

Likely direction:

- keep `StaticValueIssue` inside Python/static extraction if helpful;
- turn `PartialListSegment<T>` into a domain shape such as known/unknown rather than known plus arbitrary `SettingsIssue`;
- remove unobserved `DjangoSettings.issues` / `TemplateSettingsResolution.issues` fields unless they are wired to diagnostics.

Implementation notes:

- Removed `SettingsIssue`.
- Removed unobserved `DjangoSettings.issues` and `TemplateSettingsResolution.issues` fields.
- Replaced value-plus-issue `PartialListSegment<T>` with `PartialListSegment::{Known, Unknown}`.
- Kept `StaticValueIssue` internal to Python/static extraction; it no longer propagates into settings, template inventory, or semantic lookup state.
- Made `TemplateDirectoryEntry::UnknownSettingsDir` payload-free.
- Added a payload-free unknown state on `TemplateSettingsResolution` so whole unknown `TEMPLATES` values keep template lookup deferred instead of becoming authoritative not-found.
- Made `SourceFilesIssue::InstalledAppGap` payload-free so unknown installed-app segments no longer synthesize an empty string entry.
- Added targeted coverage for whole unknown `TEMPLATES` values.
- Hickey review found no remaining must-fix/major findings after these adjustments.
- Validation: `cargo check --all-targets`, `cargo test -p djls-project template_inventory_preserves_unknown_templates_value`, `just fmt --check`.

### Pass 5: Preserve issue enums that encode real boundary/status decisions

Status: reviewed; no additional cleanup required.

Do not simplify these without a stronger design:

- `WorkspaceRootIssue`, because it belongs at the workspace/project boundary;
- `SourceFilesIssue`, because source readiness and apply behavior use it;
- `PythonSourceIndexIssue`, because stage status uses its variants;
- `ProjectEnrichmentIssue`, because top-level variants map to different discovery statuses.

For these, limit cleanup to dead variants or payload details that are demonstrably unused.

Implementation notes:

- Rechecked the kept issue enums after the earlier dead-variant cleanup.
- `WorkspaceRootIssue`, `SourceFilesIssue`, `PythonSourceIndexIssue`, and `ProjectEnrichmentIssue` still encode live boundary or status decisions.
- `RuntimeUnavailableKind` and `InspectorFailureKind` variants are all constructed by runtime-enrichment code.
- Hickey review found no remaining pass-5 cleanup required.

## Final validation

- `cargo check --all-targets`
- `cargo test --all-targets`
- `cargo clippy --all-targets --all-features --benches -- -D warnings`
- `just fmt --check`
