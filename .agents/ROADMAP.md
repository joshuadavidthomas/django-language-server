# Template Validation Port: Program Roadmap

**Last updated:** 2026-02-05

This file is the single "what are we doing next?" index for porting the Python `template_linter/` prototype into Rust `django-language-server` (djls).

It is intentionally outcome-driven and document-linked. Implementation details live in per-milestone plans under `.agents/plans/`.

---

## North Star (End Result)

djls provides **runtime-aware, `{% load %}`-scoped template validation** where:

- The **Python inspector** reports authoritative runtime inventory (what tags/filters exist, which `{% load %}` libraries exist, builtins ordering).
- Rust (with Ruff AST) provides **rules/semantics enrichment** (how to validate usage), without runtime execution, and with **Salsa correctness** (no stale diagnostics).

Source of truth for requirements:

- [`.agents/charter/2026-02-05-template-validation-port-charter.md`](charter/2026-02-05-template-validation-port-charter.md)

---

## Documents (What Exists Now)

### Research (completed)

- [`.agents/research/2026-02-04_template-introspection-pipeline.md`](research/2026-02-04_template-introspection-pipeline.md)
- [`.agents/research/2026-02-04_tagspecs-flow-analysis.md`](research/2026-02-04_tagspecs-flow-analysis.md)
- [`.agents/research/2026-02-04_template-filters-analysis.md`](research/2026-02-04_template-filters-analysis.md)
- [`.agents/research/2026-02-04_load-tag-library-scoping.md`](research/2026-02-04_load-tag-library-scoping.md)
- [`.agents/research/2026-02-04_semantic-validation-orchestration.md`](research/2026-02-04_semantic-validation-orchestration.md)
- [`.agents/research/2026-02-04_salsa-settings-invalidation.md`](research/2026-02-04_salsa-settings-invalidation.md)
- [`.agents/research/2026-02-04_python-ast-parsing-rust.md`](research/2026-02-04_python-ast-parsing-rust.md)
- [`.agents/research/2026-02-04_template-linter-integration-seams.md`](research/2026-02-04_template-linter-integration-seams.md)

### Program charter (requirements + milestones)

- [`.agents/charter/2026-02-05-template-validation-port-charter.md`](charter/2026-02-05-template-validation-port-charter.md)

### RFCs (architecture decisions)

- [`.agents/rfcs/2026-02-05-rfc-extraction-placement.md`](rfcs/2026-02-05-rfc-extraction-placement.md) (recommends `djls-extraction` crate; extraction keyed by `SymbolKey`. Note: its earlier "new Salsa inputs" discussion is superseded by the M2 direction to fold inspector/config into `Project` and avoid new global inputs.)

### Implementation plans (per milestone)

- [`.agents/plans/2026-02-05-m1-payload-library-name-fix.md`](plans/2026-02-05-m1-payload-library-name-fix.md) (M1 plan; ready to implement)
- [`.agents/plans/2026-02-05-m2-salsa-invalidation-plumbing.md`](plans/2026-02-05-m2-salsa-invalidation-plumbing.md) (M2 plan; ready to implement)
- [`.agents/plans/2026-02-05-m3-load-scoping.md`](plans/2026-02-05-m3-load-scoping.md) (M3 plan; ready to implement)
- [`.agents/plans/2026-02-05-m4-filters-pipeline.md`](plans/2026-02-05-m4-filters-pipeline.md) (M4 plan; ready to implement)
- [`.agents/plans/2026-02-05-m5-extraction-engine.md`](plans/2026-02-05-m5-extraction-engine.md) (M5 plan; ready to implement)
- [`.agents/plans/2026-02-05-m6-rule-evaluation.md`](plans/2026-02-05-m6-rule-evaluation.md) (M6 plan; ready to implement)

**Next action:** start implementation at **M1**, then proceed in order (M2 → M3 → M4 → M5 → M6), running tests after each major change.

---

## Program Milestones (Vertical Slices)

Milestones below are copied/condensed from the charter, with explicit dependencies.

Statuses: **backlog** -> **planning** -> **ready** -> **in progress** -> **done**

### M1 - Payload shape + `{% load %}` library name correctness

**Status:** ready

**Why:** fixes a correctness bug and establishes the canonical inventory payload shape.

**Plan:** [`.agents/plans/2026-02-05-m1-payload-library-name-fix.md`](plans/2026-02-05-m1-payload-library-name-fix.md)

**Deliverables:**

- Inspector payload preserves:
    - `libraries: {load_name -> module_path}` (keys preserved)
    - `builtins: [module_path, ...]` (ordered)
    - `templatetags[]` items with provenance + `defining_module`
- Rust `TemplateTags` model includes top-level `libraries`/`builtins`.
- `{% load %}` completions show load-names (e.g. `i18n`, `static`), not module paths.

### M2 - Salsa invalidation plumbing (no stale inventory/spec/rules)

**Status:** ready

**Depends on:** M1 payload shape (so inventory can become an input)

**Why:** prevents building M3/M4/M5 on invisible/stale dependencies.

**Plan:** [`.agents/plans/2026-02-05-m2-salsa-invalidation-plumbing.md`](plans/2026-02-05-m2-salsa-invalidation-plumbing.md)

**Deliverables (conceptual):**

- **Hard constraint:** do **not** "explode" Salsa inputs. Prefer the Ruff/Rust-Analyzer pattern: a _small_ number of semantically-meaningful, _large_ inputs updated via setters.
- **Target:** keep djls at **two** primary inputs (`File`, `Project`) by extending `Project` to hold:
    - semantic config needed for validation (`tagspecs`, `diagnostics`, strictness knobs)
    - the authoritative inspector snapshot (tags/filters inventory + `libraries`/`builtins` + health)
- Replace untracked `Arc<Mutex<Settings>>` reads inside semantic code with **Salsa-visible** reads from the `Project` input (no invisible dependencies).
- Provide an explicit refresh path (`db.refresh_inspector()` or equivalent) that:
    1. queries Python (side effect), 2) updates `Project` input fields (setters), 3) relies on Salsa invalidation for downstream recomputation.
- **Future (M5+):** prefer extracted rules as _derived tracked queries_ over Python module `File` sources (so edits invalidate naturally) rather than introducing another global "rev counter" input.

#### Plan prompt

```text
/implementation_plan

Task: M2 "Salsa invalidation plumbing" - eliminate stale template diagnostics by making external data
sources explicit Salsa inputs with an explicit refresh/update path.

Read fully (source of truth):
- [`.agents/charter/2026-02-05-template-validation-port-charter.md`](charter/2026-02-05-template-validation-port-charter.md)
- [`.agents/rfcs/2026-02-05-rfc-extraction-placement.md`](rfcs/2026-02-05-rfc-extraction-placement.md) (extraction placement; Salsa integration notes
  may need updating to match the "minimal inputs" constraint)
- [`.agents/research/2026-02-04_salsa-settings-invalidation.md`](research/2026-02-04_salsa-settings-invalidation.md)
- [`.agents/research/2026-02-04_semantic-validation-orchestration.md`](research/2026-02-04_semantic-validation-orchestration.md)
- [`.agents/plans/2026-02-05-m1-payload-library-name-fix.md`](plans/2026-02-05-m1-payload-library-name-fix.md) (payload shape assumptions)

Plan must decide and specify:
- **Hard constraint:** keep Salsa inputs minimal - prefer **extending the existing `Project` input**
  (Ruff-style "big input") rather than adding new global `#[salsa::input]` types.
- Explicitly remove the untracked `Arc<Mutex<Settings>>` → semantic pipeline dependency by making
  semantic-relevant config **Salsa-visible** (via `Project` fields).
- Treat inspector results the same way: *Python produces facts*, djls **stores those facts** in
  `Project` input fields via setters (no `#[salsa::tracked]` wrapper around `inspector::query`).
- The refresh/update mechanism and API surface (e.g. `db.refresh_inspector()`), explicitly stating
  "Salsa invalidates; refresh path performs side effects".
- Which tracked queries depend on which inputs (dependency graph).
- Tests that prove invalidation works (config change, refresh change) and how to run them.
  Prefer the rust-analyzer test idiom: use salsa events + `ingredient_debug_name` rather than brittle
  substring matching of `Debug` strings.

Output file:
- [`.agents/plans/2026-02-05-m2-salsa-invalidation-plumbing.md`](plans/2026-02-05-m2-salsa-invalidation-plumbing.md)
```

### M3 - `{% load %}` scoping infrastructure (diagnostics + completions)

**Status:** ready

**Depends on:** M1, M2

**Plan:** [`.agents/plans/2026-02-05-m3-load-scoping.md`](plans/2026-02-05-m3-load-scoping.md)

**Deliverables:**

- Position-aware tag/filter availability at cursor position:
    - builtins always available
    - libraries available only after `{% load %}`
    - `{% load X from Y %}` selective imports supported
- Diagnostics + completions are scoped when inspector is healthy; conservative fallback when not:
    - validation: emit no S108/S109/S110 if inspector unavailable
    - completions: skip availability filtering if inspector unavailable (show all known tags)
    - library completions: empty if inspector unavailable (no libraries known)
- Selective-vs-full load semantics are explicit (full load overrides/clears selective imports).
- Span/position semantics are explicit: byte offsets matching `djls_source::Span`, and availability boundary is `load_stmt.span.end() <= position`.
- TagSpecs interaction is explicit: skip structural tags (openers/closers/intermediates) because those are validated by block/argument validation, not load scoping.

### Plan prompt

```text
/implementation_plan

Task: M3 "Load scoping infrastructure" - position-aware `{% load %}` scoping for tags and filters in
diagnostics + completions (builtins always available; `{% load %}` libraries only after load; support
`{% load X from Y %}` selective imports).

Read fully:
- [`.agents/charter/2026-02-05-template-validation-port-charter.md`](charter/2026-02-05-template-validation-port-charter.md)
- [`.agents/research/2026-02-04_load-tag-library-scoping.md`](research/2026-02-04_load-tag-library-scoping.md)
- [`.agents/research/2026-02-04_template-introspection-pipeline.md`](research/2026-02-04_template-introspection-pipeline.md)
- [`.agents/research/2026-02-04_semantic-validation-orchestration.md`](research/2026-02-04_semantic-validation-orchestration.md)
- [`.agents/plans/2026-02-05-m1-payload-library-name-fix.md`](plans/2026-02-05-m1-payload-library-name-fix.md)
- [`.agents/plans/2026-02-05-m2-salsa-invalidation-plumbing.md`](plans/2026-02-05-m2-salsa-invalidation-plumbing.md)

Plan must include:
- Data model for "available symbols at position" (builtins + loaded libs).
- Where it is computed (semantic vs IDE) and how it is cached/invalidation-safe (ties into M2).
- Exact behavior for unknown/collisions per charter (inspector healthy vs unavailable).
- Tests: scoping boundaries (before/after load), selective imports, completions filtering.

Output file:
- [`.agents/plans/2026-02-05-m3-load-scoping.md`](plans/2026-02-05-m3-load-scoping.md)
```

### M4 - Filters pipeline (inventory-driven; scoped; parsing breakpoint)

**Status:** ready

**Depends on:** M1, M2, M3

**Plan:** [`.agents/plans/2026-02-05-m4-filters-pipeline.md`](plans/2026-02-05-m4-filters-pipeline.md)

**Deliverables:**

- Inspector provides authoritative filter inventory (grouped by provenance).
- `djls-templates` parses filters into structured representation with spans.
- Filter completions in `{{ x|... }}` respect scoping (builtins + loaded libs).
- Unknown filter diagnostics (post-M3 semantics).
- **Planned type evolution (breaking change in M4):** switch the `Project` field carrying inspector data from a tags-only shape (`TemplateTags`) to a unified inventory shape (`InspectorInventory`) that includes both tags and filters. This is implemented _as part of M4_; M1-M3 are implemented against the tags-only shape.

### Plan prompt

```text
/implementation_plan

Task: M4 "Filters pipeline" - filter inventory-driven completions + unknown-filter diagnostics, with
load scoping correctness, and a structured filter representation in `djls-templates`.

Read fully:
- [`.agents/charter/2026-02-05-template-validation-port-charter.md`](charter/2026-02-05-template-validation-port-charter.md)
- [`.agents/research/2026-02-04_template-filters-analysis.md`](research/2026-02-04_template-filters-analysis.md)
- [`.agents/research/2026-02-04_load-tag-library-scoping.md`](research/2026-02-04_load-tag-library-scoping.md)
- [`.agents/plans/2026-02-05-m1-payload-library-name-fix.md`](plans/2026-02-05-m1-payload-library-name-fix.md)
- [`.agents/plans/2026-02-05-m3-load-scoping.md`](plans/2026-02-05-m3-load-scoping.md)

Plan must include:
- The explicit "breakpoint" for changing filter representation (`Vec<String>` → structured w/ spans),
  including which crates/files are touched and how tests will be updated.
- How inspector filter inventory will be represented (provenance consistent with M1).
- How scoping applies to filters (reuse M3 infrastructure).
- Tests: parsing edge cases, completions in `{{ x| }}`, unknown filter diagnostics, scoping checks.

Output file:
- [`.agents/plans/2026-02-05-m4-filters-pipeline.md`](plans/2026-02-05-m4-filters-pipeline.md)
```

### M5 - Rust extraction engine (`djls-extraction`) for rule enrichment

**Status:** ready

**Depends on:** M1, M2 (and ideally M3)

**Plan:** [`.agents/plans/2026-02-05-m5-extraction-engine.md`](plans/2026-02-05-m5-extraction-engine.md)

**Deliverables:**

- New crate `djls-extraction` using `ruff_python_parser` (SHA-pinned).
- Extraction keyed by `SymbolKey { registration_module, name, kind }` to avoid collisions.
- Extraction output cached via Salsa (prefer **derived tracked queries over Python module `File`** sources; avoid introducing a new global `ExtractedRules` input unless it's the only practical way).
- Corpus/full-source extraction tests (Rust-native corpus tooling), with a **temporary** Python parity oracle during the port that is explicitly deleted after M6 parity is achieved.

### Plan prompt

```text
/implementation_plan

Task: M5 "Extraction engine (rules enrichment)" - implement `djls-extraction` using Ruff AST to mine
validation semantics from Python registration modules, keyed by SymbolKey to avoid collisions, and
cache results via Salsa (prefer derived tracked queries over Python module `File` sources; avoid new
global inputs unless strictly necessary).

Read fully:
- [`.agents/charter/2026-02-05-template-validation-port-charter.md`](charter/2026-02-05-template-validation-port-charter.md)
- [`.agents/rfcs/2026-02-05-rfc-extraction-placement.md`](rfcs/2026-02-05-rfc-extraction-placement.md)
- [`.agents/research/2026-02-04_python-ast-parsing-rust.md`](research/2026-02-04_python-ast-parsing-rust.md)
- [`.agents/research/2026-02-04_tagspecs-flow-analysis.md`](research/2026-02-04_tagspecs-flow-analysis.md)

Plan must decide/specify:
- Initial Ruff SHA pinning strategy and update process.
- Exact crate boundary (`djls-extraction`) public API (pure: `source -> rules`).
- Where module->path resolution + file IO live (outside `djls-extraction`).
- How extraction is triggered/refreshed (eager vs lazy; ties into M2 refresh path).
- Tests: golden extraction fixtures + integration wiring into semantic layer.

Output file:
- [`.agents/plans/2026-02-05-m5-extraction-engine.md`](plans/2026-02-05-m5-extraction-engine.md)
```

### M6 - Rule evaluation + expression validation

**Status:** ready

**Depends on:** M3-M5

**Deliverables:**

- Apply extracted rules to templates (TemplateSyntaxError-derived constraints).
- Block structure derived from extraction (end tags, intermediates, opaque blocks).
- `{% if %}` / `{% elif %}` expression syntax validation.

**Plan:** [`.agents/plans/2026-02-05-m6-rule-evaluation.md`](plans/2026-02-05-m6-rule-evaluation.md)

### Plan prompt

```text
/implementation_plan

Task: M6 "Rule evaluation + expression validation" - apply extracted rules to templates (argument
constraints, block structure, opaque blocks) and add `{% if %}` / `{% elif %}` expression syntax
validation (operator/operand).

Read fully:
- [`.agents/charter/2026-02-05-template-validation-port-charter.md`](charter/2026-02-05-template-validation-port-charter.md)
- [`.agents/research/2026-02-04_tagspecs-flow-analysis.md`](research/2026-02-04_tagspecs-flow-analysis.md)
- [`.agents/research/2026-02-04_semantic-validation-orchestration.md`](research/2026-02-04_semantic-validation-orchestration.md)
- [`.agents/plans/2026-02-05-m3-load-scoping.md`](plans/2026-02-05-m3-load-scoping.md)
- [`.agents/plans/2026-02-05-m5-extraction-engine.md`](plans/2026-02-05-m5-extraction-engine.md)
- `template_linter/PORTING.md` (behavioral reference for rule categories)

Plan must include:
- How extracted rules map onto existing semantic validation stages (arguments, blocks, diagnostics).
- Where expression validation lives and how it reports errors (tests first).
- Test strategy: fixtures + snapshots; avoid false positives on Django core templates.

Output file:
- [`.agents/plans/2026-02-05-m6-rule-evaluation.md`](plans/2026-02-05-m6-rule-evaluation.md)
```

### M7 - Documentation + issue reporting (post-port hardening)

**Status:** backlog

**Depends on:** M1-M6

**Why:** template validation is ultimately heuristic/static compared to Python runtime behavior; we need clear docs + a high-signal repro path for gaps we discover in the wild.

**Deliverables:**

- Documentation update that explains:
    - the new runtime-inventory + load-scoping model (what's validated, when it's conservative)
    - known limitations of AST-derived rule mining (what we can/can't infer)
    - how to configure severities for "unknown/unloaded" diagnostics during adoption
- GitHub issue template for "Template validation mismatch" with a strict repro checklist:
    - environment/version info
    - minimal template snippet
    - relevant `{% load %}` statements + library names
    - `djls.toml` (tagspecs/diagnostics) excerpt
    - inspector snapshot / debug logs collection instructions
- Link the issue template from docs (and `CONTRIBUTING.md` if appropriate).

### Plan prompt

```text
/implementation_plan

Task: M7 "Docs + issue reporting" — after the port is complete, update documentation to reflect the
new template validation behavior and add a high-signal issue template for reporting mismatches
between djls static validation and Django runtime behavior.

Read fully:
- [`.agents/charter/2026-02-05-template-validation-port-charter.md`](charter/2026-02-05-template-validation-port-charter.md)
- [`.agents/rfcs/2026-02-05-rfc-extraction-placement.md`](rfcs/2026-02-05-rfc-extraction-placement.md)
- [`.agents/plans/2026-02-05-m3-load-scoping.md`](plans/2026-02-05-m3-load-scoping.md)
- [`.agents/plans/2026-02-05-m4-filters-pipeline.md`](plans/2026-02-05-m4-filters-pipeline.md)
- [`.agents/plans/2026-02-05-m5-extraction-engine.md`](plans/2026-02-05-m5-extraction-engine.md)
- [`.agents/plans/2026-02-05-m6-rule-evaluation.md`](plans/2026-02-05-m6-rule-evaluation.md)

Also inspect current docs + repo meta:
- `README.md`
- `docs/configuration/index.md` (diagnostic codes + severity config)
- `docs/configuration/tagspecs.md` (existing "open an issue" link)
- `.mkdocs.yml` (nav placement)
- `.github/` (existing templates; add if missing)

Plan must include:
- Where the documentation updates live (which existing pages to update vs adding a new page).
- Explicit language about static-analysis limits (what's authoritative from inspector vs inferred from
  AST mining), and what users should expect when custom tags do validation dynamically.
- A GitHub issue form (YAML) for template-validation mismatches with a concrete repro checklist and
  copy/paste commands for collecting debug output (versions, config, inspector snapshot).
- Minimal tests/verification for the docs/template changes (lint/build docs if applicable).

Output file:
- `.agents/plans/YYYY-MM-DD-m7-docs-and-issue-template.md`
```

---

## Documents To Generate (Remaining)

These are the next "paperwork" outputs to keep the program coordinated:

1. `.agents/plans/YYYY-MM-DD-m7-docs-and-issue-template.md`

Optional (only if we feel lost again):

- `.agents/rfcs/YYYY-MM-DD-rfc-milestone-decomposition.md` (turns milestones into a dependency graph with exact "touch points" per crate)

---

## Execution Workflow (How We Implement Without Chaos)

- Implement one milestone per PR (or a tightly-coupled pair if unavoidable).
- Every milestone must have:
    - automated tests for any new code paths
    - a deterministic story for ordering (completions, maps) to avoid flakiness
    - clear "inspector healthy vs unavailable" behavior when it affects diagnostics
- Run tests after major changes:
    - crate-local while iterating (e.g. `cargo test -p djls-project`, `cargo test -p djls-ide`)
    - broader `cargo test` before landing

---

## Open Decisions / Inputs Needed (Tracked)

From [`.agents/rfcs/2026-02-05-rfc-extraction-placement.md`](rfcs/2026-02-05-rfc-extraction-placement.md):

- Which Ruff SHA to pin initially.
- Refresh triggers: user command vs timer vs watchers (what calls `db.refresh_inspector()`).
- Eager vs lazy extraction: extract everything at refresh vs on-demand per module, with caching.
- Durability levels for the "big inputs" (config/extracted rules likely HIGH; inventory likely MEDIUM/LOW).
