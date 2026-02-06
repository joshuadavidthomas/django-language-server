# Template Validation Port: Program Roadmap

**Last updated:** 2026-02-06

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
- [`.agents/plans/2026-02-05-m5-extraction-engine.md`](plans/2026-02-05-m5-extraction-engine.md) (M5 plan; done)
- [`.agents/plans/2026-02-05-m6-rule-evaluation.md`](plans/2026-02-05-m6-rule-evaluation.md) (M6 plan; done — **except** `ExtractedRule` evaluation, see M8)
- [`.agents/plans/2026-02-06-m8-extracted-rule-evaluation.md`](plans/2026-02-06-m8-extracted-rule-evaluation.md) (M8 plan; ready to implement)
- `.agents/plans/YYYY-MM-DD-m9-tagspec-simplification.md` (M9 plan; pending — small scope, after M8)

**Next action:** generate M8 plan, then implement. M9 follows immediately after M8.

---

## Program Milestones (Vertical Slices)

Milestones below are copied/condensed from the charter, with explicit dependencies.

Statuses: **backlog** -> **planning** -> **ready** -> **in progress** -> **done**

### M1 - Payload shape + `{% load %}` library name correctness

**Status:** done

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

**Status:** done

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

**Status:** done

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

**Status:** done

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

**Status:** done

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

**Status:** done (partial — see M8 for the gap)

**Depends on:** M3-M5

**Delivered:**

- ✅ `{% if %}` / `{% elif %}` expression syntax validation (Pratt parser, S114)
- ✅ Filter arity validation from extraction (S115, S116)
- ✅ Opaque region handling (`{% verbatim %}` etc.) from extraction
- ✅ Block structure derived from extraction (end tags, intermediates)
- ❌ **`ExtractedRule` evaluation was deferred** — the core argument validation rules
  (`MaxArgCount`, `LiteralAt`, `ChoiceAt`, `ExactArgCount`, etc.) are stored on `TagSpec.extracted_rules`
  but nothing reads them. The old hand-crafted `args` + `validate_argument_order` path is still
  doing all argument validation. See M8.

**Plan:** [`.agents/plans/2026-02-05-m6-rule-evaluation.md`](plans/2026-02-05-m6-rule-evaluation.md)

**Post-mortem:** The M6 plan's "What We're NOT Doing" section contained _"ContextualRule/ExtractedRule
evaluation: Deferred (complex preconditions)"_ — this contradicted the charter's intent that M6 would
deliver "Rich argument validation" powered by extracted rules. Combined with M5's _"Immediate
builtins.rs removal: Keep as fallback"_, this created a dual-system architecture where extraction
results are computed but never used for argument validation.

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

**Status:** done

**Depends on:** M1-M6

**Deliverables:**

- ✅ Documentation explaining runtime-inventory + load-scoping model
- ✅ Known limitations of AST-derived rule mining documented
- ✅ Severity configuration for "unknown/unloaded" diagnostics
- ✅ GitHub issue template for "Template validation mismatch"

### M8 - Extracted rule evaluation (complete replacement of static argument validation)

**Status:** ready

**Depends on:** M5, M6

**Plan:** [`.agents/plans/2026-02-06-m8-extracted-rule-evaluation.md`](plans/2026-02-06-m8-extracted-rule-evaluation.md)

**Why:** This is the missing piece that completes the charter's core goal. M5 built the extraction
engine — rules ARE extracted correctly from Python AST (golden tests prove it). M6 wired expression
validation, filter arity, and opaque regions. But the core `ExtractedRule` conditions are stored on
every `TagSpec.extracted_rules` and **nothing reads them**. The old hand-crafted `args` field +
`validate_argument_order()` is still doing all argument validation. This milestone replaces that path.

**Scope (post-planning — absorbs most of what was originally M9):**

M8 does NOT keep the old system as a "safety net" — that's the deferral pattern that created the
dual-system mess. Instead, M8 delivers the complete replacement in one milestone:

1. **Argument structure extraction** from Python AST (new pass in `djls-extraction`):
   - `simple_tag`/`inclusion_tag`: directly from function signature (same pattern as `extract_filter_arity`)
   - Manual `@register.tag`: reconstruct from `ExtractedRule` conditions + AST tuple unpacking / indexed access
2. **`ExtractedRule` evaluator** in `djls-semantic` that validates tag arguments against extracted conditions
3. **Wire evaluator into pipeline** — extracted rules are primary, NO fallback to old `args`
4. **Extracted args → completions/snippets** — `ExtractedArg` converts to `TagArg`, populates `TagSpec.args` from extraction instead of hand-crafted values
5. **Remove old system** — strip hand-crafted `args:` from `builtins.rs`, remove `EndTag.args`/`IntermediateTag.args`, simplify `merge_block_spec`
6. **Corpus template validation tests** — port prototype's `test_corpus_templates.py` and `test_real_templates.py` to Rust: validate real templates from Django 4.2-6.0, Wagtail, allauth, crispy-forms, Sentry, NetBox against extracted rules. Zero false positives. **This is the proof.**

**What stays after M8:**

- `TagSpec.args` field — now populated from extraction, used by completions/snippets
- `TagArg` enum — used by extraction→completion conversion and user config escape hatch
- `validate_argument_order()` — reachable ONLY via user-config `djls.toml` `args` definitions (not builtins)
- `builtins.rs` block structure (end tags, intermediates, module mappings) — compile-time baseline

**What goes:**

- ~973 lines of hand-crafted `args:` values in `builtins.rs` (replaced by extraction)
- `EndTag.args` and `IntermediateTag.args` fields
- The `merge_block_spec` "preserve existing args" guards
- The old path being reachable for any builtin tag

**Key implementation details:**

- **Index offset:** Extraction indices include tag name (index 0). Parser `bits` excludes it. Evaluator adjusts: extraction index N → `bits[N-1]`.
- **Negation semantics:** `negated: true` = error when condition is NOT met (e.g., `LiteralAt{value:"in", negated:true}` = error when bits[1] != "in").
- **Opaque rules:** `RuleCondition::Opaque` = silently skip, never error.
- **Error messages:** Use `ExtractedRule.message` (Django's original text) in S117 `ExtractedRuleViolation` diagnostic.

### M9 - User config tagspec simplification (evaluate and clean up `djls.toml` args)

**Status:** backlog

**Depends on:** M8

**Why:** M8 removes all hand-crafted builtin `args` and replaces them with extraction-derived
argument specs. The only remaining consumer of the `TagArg`-based validation path is user-defined
`args` in `djls.toml` tagspec config — an escape hatch for tags that extraction can't handle
(dynamic registration, metaprogramming, unusual decorator wrappers).

With extraction handling the vast majority of cases, this escape hatch may be overly complex.
The current TOML `args` format (`ArgKindDef`, `TagArgDef`, choices, counts, etc.) mirrors the
internal `TagArg` enum and is confusing for users. A simpler mechanism may be more appropriate.

**Scope (small — M8 did the heavy lifting):**

- Evaluate whether user-config `args` should be simplified to a basic ignore/override mechanism
  (e.g., `[diagnostics.ignore]` rules, or `[tagspecs.overrides]` with just tag name → skip validation)
- If simplified: remove `TagArg` enum entirely, remove `validate_argument_order()`, remove
  `ArgKindDef`/`TagArgDef` types from `djls-conf`, simplify `TagSpec.args` to `Option<Vec<ExtractedArg>>`
- If kept as-is: document the format, add examples, ensure it works correctly alongside extraction
- Clean up any remaining dead code from the M8 transition
- Update user-facing documentation for tagspec configuration

**Deliverables:**

- Decision on tagspec config format (simplify vs keep)
- Implementation of chosen approach
- Documentation updates
- Test cleanup for any removed types

### Plan prompt

```text
/implementation_plan

Task: M9 "User config tagspec simplification" — evaluate whether the `djls.toml` `args` config
format should be simplified now that M8's extraction handles all builtin and most third-party
tag validation. The `TagArg`-based user config is the only remaining consumer of the old
validation path.

Read fully:
- [`.agents/plans/2026-02-06-m8-extracted-rule-evaluation.md`](plans/2026-02-06-m8-extracted-rule-evaluation.md)

Also inspect current code (post-M8):
- `crates/djls-conf/src/tagspecs.rs` — `TagArgDef`, `ArgKindDef` types for user config
- `crates/djls-semantic/src/templatetags/specs.rs` — `TagArg` enum, `From<TagArgDef>` conversion
- `crates/djls-semantic/src/arguments.rs` — `validate_argument_order` (only user-config path)
- User-facing docs for tagspec configuration

Plan must decide:
- Is the `TagArg`-based user config escape hatch worth its complexity, or should it be replaced
  with a simpler mechanism (e.g., per-tag diagnostics ignore, or per-tag extracted-rule overrides)?
- If simplified: what replaces it? What's the migration path for existing `djls.toml` files?
- If kept: what documentation/examples are needed to make it usable?

Output file:
- [`.agents/plans/YYYY-MM-DD-m9-tagspec-simplification.md`](plans/YYYY-MM-DD-m9-tagspec-simplification.md)
```

---

## Documents To Generate (Remaining)

1. ~~`.agents/plans/YYYY-MM-DD-m8-extracted-rule-evaluation.md`~~ — done: [`.agents/plans/2026-02-06-m8-extracted-rule-evaluation.md`](plans/2026-02-06-m8-extracted-rule-evaluation.md)
2. `.agents/plans/YYYY-MM-DD-m9-tagspec-simplification.md` — after M8 is implemented (small scope)

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

## Lessons Learned

### Plan review must verify charter alignment (2026-02-06)

Two throwaway lines in M5 and M6 plans contradicted the charter's core intent:

- M5: _"Immediate builtins.rs removal: Keep as fallback; extraction enriches/overrides"_
  (charter says "replaces", not "enriches")
- M6: _"ContextualRule/ExtractedRule evaluation: Deferred (complex preconditions)"_
  (charter says M6 delivers "Rich argument validation" via extracted rules)

These slipped through plan review and caused the entire M5-M6 implementation to build a dual-system
architecture where extraction results are computed but never used for argument validation. The old
hand-crafted system remained active. Result: two parallel validation paths, merge bugs at the
boundary (e.g., `merge_block_spec` clobbering `endblock`'s argument definitions), and the charter's
core goal unmet.

**Takeaway:** When reviewing plans, explicitly check each "What We're NOT Doing" / "Keep as fallback"
line against the charter. A plan that defers the thing the charter says to deliver is a plan that
diverges.

---

## Open Decisions / Inputs Needed (Tracked)

From [`.agents/rfcs/2026-02-05-rfc-extraction-placement.md`](rfcs/2026-02-05-rfc-extraction-placement.md):

- ~~Which Ruff SHA to pin initially.~~ (decided: 0.9.10)
- Refresh triggers: user command vs timer vs watchers (what calls `db.refresh_inspector()`).
- Eager vs lazy extraction: extract everything at refresh vs on-demand per module, with caching.
- Durability levels for the "big inputs" (config/extracted rules likely HIGH; inventory likely MEDIUM/LOW).

From M8/M9 planning:

- Should TOML tagspec `args` config be simplified to a basic ignore/override mechanism, or kept as-is?
  (Current format is overly flexible and confusing. With extraction handling most cases, a simpler
  escape hatch like `[diagnostics.ignore]` may be more user-friendly. Deferred — not blocking M8/M9.)
