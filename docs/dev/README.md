# Dev Docs

Development planning and research docs. The canonical program roadmap
lives at [`.agents/ROADMAP.md`](../../.agents/ROADMAP.md) — these docs
support specific milestones and refactoring efforts.

## Extraction Crate Refactor (M10)

Refactoring `djls-python` from agent-written procedural code to
type-driven Rust with Salsa integration. The algorithms (interprocedural
abstract interpretation with bounded inlining) are correct — the
implementation and representation are changing.

| Doc | Purpose |
|-----|---------|
| [extraction-architecture-map.md](extraction-architecture-map.md) | Current state: 9 responsibilities, 4 consumers, data flow, the HelperCache problem |
| [extraction-refactor-research.md](extraction-refactor-research.md) | Problems identified from PR #394 review + Ruff/ty codebase study |
| [extraction-refactor-plan.md](extraction-refactor-plan.md) | Phased migration: 7 phases + parallel type track, risks, execution notes |
| [extraction-type-driven-vision.md](extraction-type-driven-vision.md) | Destination: domain types, target architecture, known complications |
| [extraction-test-strategy.md](extraction-test-strategy.md) | Replace fabricated test snippets with corpus-sourced real Django code |

**Reading order**: architecture map → research → vision → plan

## Ruff/ty Reference

Patterns and files from the Ruff codebase that inform the extraction
refactor. Ruff is the closest analog — large-scale Python AST analysis
in Rust with Salsa caching.

| Doc | Purpose |
|-----|---------|
| [ruff-patterns-reference.md](ruff-patterns-reference.md) | Concrete code examples: narrowing, type inference, semantic model, diagnostics |
| [ruff-reading-list.md](ruff-reading-list.md) | Guided reading order through Ruff source files for deep dives |

## Other

| Doc | Purpose |
|-----|---------|
| [corpus-refactor.md](corpus-refactor.md) | `djls-corpus` crate: add `Corpus` struct, clap CLI, consolidate test databases |
