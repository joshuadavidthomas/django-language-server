# Research Agenda: startup-rethink

## Research goal
Clarify how DJLS startup should move toward a rust-analyzer-style model: fast LSP initialization, cheap eager project/file cataloging, Salsa-friendly inputs, and lazy or background Django semantics. The research should give a future design enough evidence to redraw the boundary between session creation, workspace discovery, Project Facts, Django Environment discovery, caches, and runtime-backed enrichment.

## Research questions

### Current startup behavior
- What work currently happens before, during, and immediately after `Session` creation, LSP `initialize`, and `initialized`, and which parts block editor readiness?
- Which current startup tasks are eager semantic work, external I/O, filesystem discovery, cache loading, subprocess management, or plain input construction?
- Where does DJLS currently do too much lazily because it lacks a complete inventory of project files, settings candidates, templates, models, templatetag modules, or configuration files?

### Rust-analyzer comparison
- Which rust-analyzer startup concepts map cleanly to DJLS concepts: workspace discovery, project folders, VFS source roots, file IDs, local/library roots, crate graph inputs, cache priming, and async follow-up work?
- Which rust-analyzer concepts should not be copied because Django projects lack an exact equivalent to Cargo metadata, crate roots, or module ownership?
- What is rust-analyzer's practical boundary between eager structural cataloging and lazy semantic computation, and what analogous boundary should DJLS use?

### File and project catalog shape
- What minimal per-file facts should DJLS eagerly record at startup: path, source root, local/library classification, file kind, template name candidate, Python module candidate, settings candidate, model-bearing module candidate, templatetag module candidate, package/config metadata, and ignored/excluded state?
- What project-level or source-root-level facts can be derived cheaply from filenames, directory layout, package metadata, and known Django conventions without parsing or executing project code?
- How should the catalog represent uncertainty, ambiguity, and multiple candidates without prematurely choosing a single Django Environment or settings module?

### Django Environment discovery
- How should startup discover and represent path-scoped Django Environment candidates, including settings modules, manage.py files, pyproject/setup metadata, apps, template directories, and Python package roots?
- What should be the default selection behavior when multiple settings modules or environment candidates exist?
- Which facts belong to the Project as a whole, which belong to a Django Environment, and which belong to individual files or source roots?

### Salsa and invalidation model
- Which catalog facts should become Salsa inputs, tracked derived queries, or non-Salsa runtime state?
- What durability split should DJLS use for first-party files, external/library files, package metadata, cached introspection results, and user configuration?
- How should file creation, deletion, rename, open-buffer overlays, and settings changes invalidate the catalog without forcing unrelated semantic recomputation?

### Introspection, static extraction, and background enrichment
- Which existing Project Introspection tasks can be removed from the startup critical path and run as background enrichment?
- Which source-derived work is cheap enough to run during startup, and which extraction or semantic work must remain request-driven or background-only?
- How should DJLS behave when Python, Django, the configured interpreter, or runtime introspection fails after the cheap catalog succeeds?

### Caching and readiness
- What cache entries are safe to use as temporary startup seeds, and what fresh filesystem/catalog pass must remain authoritative?
- What cache invalidation keys or freshness checks are needed for project layout, settings candidates, external dependencies, introspection data, and static extraction results?
- How should the server communicate partial readiness, degraded mode, background refresh progress, and late-arriving Project Facts to LSP clients?

### Migration and tests
- What existing code paths would need to move out of session construction or eager initialization into catalog construction, lazy queries, or background tasks?
- What tests would prove startup no longer blocks on expensive Django/project semantics while still building a useful file/project inventory?
- What fixtures or corpus cases should research use to validate multi-environment discovery, ambiguous settings modules, template directory inference, model discovery, templatetag discovery, external packages, and cache staleness?

## Scope boundaries
- Do not design the full replacement architecture yet; first gather evidence from the current DJLS codebase, rust-analyzer's startup model, and representative Django project layouts.
- Do not require full static settings extraction, model graph construction, template validation, or tag/filter extraction to happen during startup.
- Do not assume a single workspace root maps to a single Project or a single Django Environment.
- Do not make runtime-backed Project Introspection part of the startup critical path.
- Do not treat cache hits as a substitute for an authoritative cheap fresh catalog pass.

## Open assumptions
- Startup should follow a rust-analyzer-style contract: complete the LSP handshake quickly, then build project/file inputs and deeper facts in the background or lazily.
- The eager startup catalog should be cheap and heuristic: enough to know the lay of the land, not enough to validate every template or fully understand every Python module.
- Cheap package/config metadata may be eager when it helps classify the project, similar in spirit to rust-analyzer using Cargo metadata.
- DJLS should design for multiple path-scoped Django Environments. Early implementation slices may defer ambiguous environment-specific behavior, but must not choose one global default environment.
- Project Introspection should become background enrichment with a degraded-mode story when runtime Django is unavailable.
- DJLS should use Salsa-friendly inputs, source-root-like categorization, and durability boundaries as the foundation for the new startup model.
