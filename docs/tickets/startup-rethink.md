# Rethink DJLS startup around a cheap project catalog

## Summary
Rework DJLS startup so the server becomes responsive quickly, eagerly builds a cheap catalog of project and file facts, and defers deeper Django semantics to lazy queries or background enrichment. The new model should take inspiration from rust-analyzer: load the shape of the project first, then let semantic work happen from Salsa-friendly inputs.

## Problem / opportunity
Startup currently feels wrong in both directions. DJLS does too much around session creation and early initialization, especially work that should not block editor readiness, but it also does too little upfront to understand the shape of the Project. The server lacks an early inventory of important files and categories such as settings candidates, templates, models, templatetag modules, package/config metadata, and path-scoped Django Environment candidates.

This is an opportunity to make startup simpler, faster, and more structurally useful: first build a cheap lay of the land, then use that foundation for Project Facts, static extraction, introspection, diagnostics, and IDE features.

## Desired outcome
DJLS has a startup model where the LSP handshake and initial responsiveness are not blocked by deep Django semantics. Startup eagerly records cheap file and project facts, represents ambiguity instead of forcing one project shape too early, and creates a foundation for lazy or background work to enrich Project Facts over time.

## Scope
- Define the target startup contract for DJLS, inspired by rust-analyzer.
- Identify what currently happens before, during, and after session creation, `initialize`, and `initialized`.
- Shape an eager catalog of cheap project/file facts: file identity, local/library classification, file kind, settings candidates, template candidates, model-bearing module candidates, templatetag modules, config/package metadata, ignored/excluded state, and Django Environment candidates.
- Design for multiple path-scoped Django Environments. Early implementation slices may defer ambiguous environment-specific behavior, but must not invent a single global default environment.
- Move runtime-backed Project Introspection out of the startup critical path conceptually, treating it as background enrichment with degraded-mode behavior.
- Establish which facts should be Salsa inputs, tracked derived facts, runtime state, or cached hints.
- Define readiness, cache, invalidation, and test expectations for the new model.

## Non-goals
- Do not eagerly validate every template at startup.
- Do not eagerly build the full Model Graph.
- Do not require full static settings extraction as part of this first slice.
- Do not require tag/filter extraction or deep Python AST analysis during startup.
- Do not assume one workspace root maps to exactly one Project or one Django Environment.
- Do not make runtime Django, Python interpreter availability, or Project Introspection part of the startup critical path.
- Do not treat cache hits as a replacement for a cheap fresh filesystem/catalog pass.

## Success criteria
- There is a documented target startup model that separates LSP readiness, cheap catalog construction, lazy semantic queries, and background enrichment.
- The current startup path is mapped well enough to identify which work should move out of session construction or early initialization.
- The proposed catalog names the minimum useful Project Facts and file facts DJLS should know eagerly.
- The design preserves ambiguity around multiple settings modules and Django Environment candidates instead of collapsing too early.
- The plan explains how Salsa inputs, durability, invalidation, and caches should support the startup model.
- The test strategy covers fast readiness, cheap inventory construction, multi-environment ambiguity, cache staleness, and degraded introspection.

## Acceptance examples
- Given a workspace with no warm cache, when DJLS starts, then the server can complete LSP initialization without waiting for runtime-backed Project Introspection.
- Given a Django project with templates, models, templatetag modules, and settings candidates, when the startup catalog pass completes, then DJLS has cheap facts describing those files without deep semantic validation.
- Given a workspace with multiple settings modules, when startup cataloging runs, then DJLS records the ambiguity as Django Environment candidates rather than assuming a single global settings module.
- Given Python or Django introspection fails, when the cheap catalog succeeds, then DJLS can still operate in a degraded mode and enrich Project Facts later if runtime data becomes available.
- Given cached Project Facts exist, when DJLS starts, then the cache may seed behavior but a fresh cheap catalog pass remains authoritative.

## Constraints and assumptions
- DJLS should follow a rust-analyzer-style contract: handshake quickly, build structural inputs eagerly, compute deeper semantics lazily or in background tasks.
- The eager catalog should rely on cheap heuristics, path conventions, and lightweight metadata, not runtime Django execution.
- Cheap package/config metadata may be eager when it helps classify the Project, similar in spirit to rust-analyzer using Cargo metadata.
- Salsa should remain the core mechanism for inputs, derived facts, incrementality, and durability boundaries.
- Project Facts, Django Discovery, Django Environment, Template, Template Directory, and related terms should follow `CONTEXT.md`.
- `docs/agents/startup-rethink/questions.md` is the research agenda for this ticket.
- `docs/agents/startup-rethink/research.md` contains completed evidence for current DJLS startup behavior, rust-analyzer comparison, catalog gaps, Salsa/invalidation, caching/readiness, and migration/test risks.
- `thoughts/shared/research/rust-analyzer-init-session.md` is prior art for rust-analyzer's startup model.

## Open questions
### Resolve before next stage
- No additional research blockers are known; use the completed research artifact as the starting point for design discussion.

### Defer to later stage
- [design-discussion] What is the exact startup phase split between minimal `initialize`, post-`initialized` catalog work, lazy semantic queries, and background enrichment?
- [design-discussion] What is the catalog data model and ownership boundary between Workspace, Project, Django Environment candidates, source roots, and file facts?
- [design-discussion] How should background catalog/enrichment work avoid holding the shared `Session` lock across expensive filesystem, introspection, or extraction work?
- [design-discussion] How should the existing static project model and static Django Environment discovery be wired into Project Facts without collapsing ambiguity too early?
- [design-discussion] How should partial readiness, degraded mode, cache seeding, and late-arriving Project Facts be exposed through the LSP layer?
- [structure-outline] Which modules and crates should change once the design is chosen?
- [create-plan] What characterization tests and migration steps should guard the startup rewrite?

## Likely next move
design-discussion — research is complete, so the next useful step is to choose the startup phase model, catalog shape, Salsa boundaries, and LSP readiness policy before implementation planning.
