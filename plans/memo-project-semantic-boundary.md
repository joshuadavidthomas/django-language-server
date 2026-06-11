# Memo: the djls-project / djls-semantic boundary after plan 015

- **Subject**: what remains in `djls-semantic` after PR #668
  (plan 015, branch `plan-015-move-project-model`, head `735cea66`)
  and whether the crate seam matches the intended architecture
- **Question**: should mechanical Python/source recognition that still
  lives in `djls-semantic/src/python/` move into `djls-project`, or is
  the current "produces semantic vocabulary → stays in semantic" rule
  the right boundary?
- **Date**: 2026-06-11
- **Verdict in one line**: the current boundary classifies by *output
  vocabulary*; the codebase's own structure already argues for
  classifying by *activity* — everything under
  `djls-semantic/src/python/` is mechanical source observation that
  sits below the semantic trait, reads no template, and should move to
  `djls-project`, leaving djls-semantic as the layer that fuses
  observed facts into *project meaning* — expressed through template
  analysis today, through Python-file features tomorrow. Plan 015
  needs no change (it
  shipped the right move for its scope); plan 017 needs amending; the
  move itself should be a new plan sequenced after 015 merges and
  before 016/017.

## 1. Current State

### 1.1 What PR #668 moved

Post-015, `djls-project` (~7,400 lines) owns the project model and the
*first two* static source recognizers:

| Area | Files | What it recognizes |
|---|---|---|
| Settings extraction | `extraction/extractor.rs` (1,499), `extraction/settings.rs` (284), `extraction/paths.rs` (138) | `INSTALLED_APPS`/`TEMPLATES` from settings modules, including `if`-branch truthiness evaluation, star-import recursion, and a path-expression micro-evaluator |
| Registration scanning | `extraction/registry.rs` (661) | `@register.tag` / `@register.filter` / call-style registrations → `RegistrationInfo`/`RegistrationKind` |
| Shared AST helpers | `extraction/ext.rs` (80) | `ExprExt` literal extraction |
| Parse query | `parse.rs` (35) | `parse_python_module(db, File)` — the salsa-cached ruff parse |
| Derivation queries | `settings.rs` (1,188), `resolve.rs` (1,303), `templates.rs` | `template_dirs`, `template_libraries`, `model_modules`, `templatetag_modules`, `project_template_files` |

`djls-semantic` keeps everything template-side (tags fusion, structure,
scoping, validation, resolution, offset) — *plus* roughly 10,000 lines
under `src/python/`:

| Area | Lines | Input → Output |
|---|---|---|
| `python.rs` | 1,096 | salsa queries `extract_tag_rules`/`extract_filter_arities`/`extract_block_specs` over `(File, ModulePath)`; `analyze_helper` (interned, cycle-recovered); non-salsa `extract_rules(source, module_path)` |
| `python/analysis/` (9 files) | ~5,250 | compile-function body → `TagRule` (abstract interpretation: token-split tracking, `if cond: raise` guard negation, option loops, match arms; `guards.rs` 1,238, `statements.rs` 1,249) |
| `python/blocks/` (5 files) | ~1,512 | compile-function body → `BlockSpec` (`parser.parse(...)` / `skip_past` / `next_token` pattern recognition) |
| `python/models/` (3 files) | ~1,705 | `models.py` body → `ModelGraph` (class/inheritance/relation recognition, fixed-point base resolution); `compute_model_graph(db, project)` |
| `python/signature.rs` | 204 | `simple_tag`/`inclusion_tag` signature → `TagRule` |
| `python/filters.rs` | 252 | filter signature → `FilterArity` |
| `python/registry.rs` | 93 | `RegistrationKindExt` bridge routing `djls_project::RegistrationKind` to the extractors above |
| `python/types.rs` | 522 | `TagRule`, `BlockSpec`, `FilterArity`, `SymbolKey`, `ExtractionResult` — pure data, serde |

### 1.2 The consumption topology (verified, not assumed)

Three structural facts about this subtree:

1. **It already sits below the semantic trait.** `analyze_helper` takes
   `&dyn djls_source::Db` (`python.rs:85`); the three `extract_*`
   queries likewise need only `parse_python_module`. The single
   semantic-`Db` import in the subtree is `python/models.rs:10`, and
   `compute_model_graph` uses nothing from it beyond what
   `djls_project::Db` provides (`model_modules`, `python/models.rs:21`).
   Nothing in `python/` reads `tag_specs()`, `template_libraries()`, or
   any other semantic accessor. The subtree is semantic *in location
   only*.

2. **No template-side code reads a Python AST, and no Python-side code
   reads a template.** `scoping/` and `validation/` consume only
   already-extracted facts (`TagSpecs`, `FilterAritySpecs`,
   `TemplateLibraries`, `SymbolIndex`). The only crossings are the two
   fusion queries: `compute_tag_specs` calls `extract_block_specs` /
   `extract_tag_rules` (`tags.rs:49,54`) and
   `compute_filter_arity_specs` calls `extract_filter_arities`
   (`filters.rs:87`). The seam Josh describes already exists as a
   *call boundary*; it just doesn't coincide with the crate boundary.

3. **All ruff usage in djls-semantic is confined to `src/python/`**
   (`rg -l ruff_python crates/djls-semantic/src/` hits nothing outside
   it). If the subtree moves, djls-semantic drops `ruff_python_ast` and
   `ruff_python_parser` from its manifest entirely — the boundary
   becomes machine-checkable: *only djls-project parses Python*.

One more data point: `ModelGraph` has **zero feature consumers
today**. The `Db::model_graph()` accessor exists (`db.rs:38`,
implemented at `djls-db/src/db.rs:176-180`), but no IDE feature,
validator, or scoping rule reads it — its only callers are djls-db's
cache tests, djls-bench, and the corpus test. That is deliberate: it is
groundwork for future features that operate on Python files directly
(models.py, settings.py, and other typical Django files), not an
orphan (maintainer, 2026-06-11). Which makes it a derived Django fact
built to serve *multiple* future surfaces — exactly the category
`AGENTS.md:32` assigns to djls-project ("derived Django facts"), and
all the more reason it belongs below every layer that will consume it.

### 1.3 The boundary rule as currently written

Plan 015's Out-of-scope section is explicit:

> The spec-extraction analyses (`python/analysis/`, `python/blocks/`,
> `python/models/`, `python/types.rs`, `python/signature.rs`,
> `python/filters.rs`) — they produce semantic vocabulary
> (`TagRule`/`BlockSpec`/`FilterArity`) and STAY in djls-semantic
> permanently (2026-06-10 crate-count review).

Plan 017 restates it in its maintenance notes and builds Step 3 on it
(move the extraction queries *sideways* into
`src/python/queries.rs`). The shipped docs encode it too:
`AGENTS.md:33` lists "Python spec extraction" as a djls-semantic
responsibility, and `ARCHITECTURE.md:93` describes it as a semantic
concern ("Ruff-based analysis of templatetag and model Python source
after djls-project has resolved which modules matter").

Two things are worth being honest about regarding that rule's
provenance:

- The 2026-06-10 crate-count review decided a *crate-count* question —
  don't mint `djls-extraction`/`djls-python` crates — not a seam
  question. The registration scanner then moved down because plan 008's
  derivation in djls-project would otherwise call *up* into semantic (a
  cycle). The "semantic vocabulary" rationale was written after the
  cycle forced the minimal move; it rationalizes where the line landed
  rather than deriving it.
- The rule's classifier is "what the output is named", and it produces
  visible inconsistencies, inventoried next.

## 2. Expected End State

The desired seam, restated as an operational test rather than a slogan:

- **`djls-project`**: *what did the source mechanically say?* Parsing,
  AST/source recognition, filesystem discovery, and source-to-fact
  extraction whose output is an **observed fact** — a distilled,
  judgment-free description of what a file contains or does. Observed
  facts may use Django-domain words (they already do:
  `TemplateLibraries`, `TemplateSymbol`, `RegistrationKind`,
  `InstalledAppsSetting`).
- **`djls-semantic`**: *what do those facts mean?* The
  **project-meaning** layer — not "template meaning" (maintainer
  correction, 2026-06-11). It fuses observed facts with built-in
  knowledge and configuration, decides availability and validity, and
  attaches spans and diagnostics. Template analysis is how that meaning
  is expressed today; features over Python files (models.py,
  settings.py, other typical Django files) are how it grows tomorrow,
  and they land in this same layer.

The classifier is the **activity**, not the output's vocabulary and not
the source kind: a computation that produces a judgment-free
description of what source contains or does — output invariant to how
the server will later judge it — is project-layer observation. A
computation that decides validity, availability, or diagnostics is
meaning.

This also matches the repo's own integration-boundaries rule
(anti-corruption layer at the seam): ty is not actually a precedent for
keeping this in semantic — ty analyzes Python *to report on Python*, so
its analysis machinery is its semantic layer. DLS analyzes Python *to
report on templates*; the Python work here is observation of a foreign
source, which is precisely what the lower layer is for. The honest ty
analogy for `djls-semantic` is the meaning layer, not the Python
walkers. (Planned future features that operate on Python files
directly — see §6 Q4 — fit this without strain once djls-semantic is
read as the *project*-meaning layer, which is the maintainer's
framing: Python-file meaning lands in djls-semantic above the same
observed facts; observation still doesn't move up.)

## 3. Mismatch

### 3.1 Classification of what remains

| Area | Classification | Evidence |
|---|---|---|
| `python/analysis/`, `python/blocks/`, `python/signature.rs`, `python/filters.rs` | **Project** (mechanical recognition) | Input is a `StmtFunctionDef`; output describes the compile function's observable behavior ("raises unless `len(bits) == 2`", "parses until `endfoo`/`empty`", "signature takes an optional arg"). No template is consulted; no diagnostic is decided. The interpretation happens elsewhere: `validation/arguments.rs` turns rule violations into S114 with spans; `tags.rs` decides precedence against builtins. |
| `python/models/` | **Project**, and the clearest case | Observed facts about Django model classes; zero template content; zero feature consumers today, by design — it is groundwork for Python-file features (§1.2). A "derived Django fact" in the AGENTS.md sense, structurally parallel to `template_libraries`, and a fact base meant to be shared by template features and future Python-file features alike — which is only possible from the lower layer. |
| `python/types.rs` (`TagRule`, `BlockSpec`, `FilterArity`, `SymbolKey`, `ExtractionResult`) | **Project** (the recognizers' output language) | Same epistemic category as `DjangoSettings`/`TemplateBackend`/`RegistrationInfo`, which already live in `djls-project/src/extraction/settings.rs`. Output types travel with their producer — the inverse (walkers down, vocabulary up) is a dependency cycle and not an option (§4, Option D). |
| `python/registry.rs` bridge | **Project** | It dispatches on `djls_project::RegistrationKind` — a foreign type, which is why plan 015 had to convert the inherent impl into the `RegistrationKindExt` extension trait (orphan rule). Moving it down dissolves the workaround back into an inherent impl. When a seam forces an extension trait *on your own domain object*, the seam is cutting through the object. |
| `python.rs` extraction queries (`extract_tag_rules` etc., `analyze_helper`) | **Project** | Already `&dyn djls_source::Db`-shaped (§1.2); they are the salsa wrappers over the recognizers, exactly the pattern `djls-project/src/settings.rs` already uses for `extract_settings`. |
| `compute_tag_specs`, `builtin_tag_specs`, `TagSpecs`/`TagSpec`/`TagRole` (`tags/`) | **Semantic** | This is judgment: hand-curated builtin specs, merge precedence (builtins ← extracted ← config fallback, `tags.rs:46-62`), and the `TagRole` taxonomy that drives outline/navigation. The fusion is where observed facts become meaning. |
| `compute_filter_arity_specs` (`filters.rs`) | **Semantic** | Same fusion shape, including the Django-order last-wins policy. |
| `structure/` (`TemplateTree`, `TagIndex`, opaque, outline) | **Semantic** | Mechanical, yes — but mechanical over the *template* AST, driven by `TagSpecs` (a semantic fusion product). The desired seam is about Python/project source mechanics; template-AST plumbing in service of template meaning is semantic-side by both classifiers. (Whether tree-building could someday sink into `djls-templates` is a separate, recorded-deferred question that this memo does not reopen.) |
| `scoping/`, `validation/`, `resolution.rs`, `offset.rs` | **Semantic** | Availability, validity, navigation — the brain. |
| `ExtractedDiagnosticMessage` and friends (inside `TagRule`) | **Ambiguous, travels with `TagRule`** | It captures the Python exception's message template — an observation — but exists purely to improve diagnostic wording. It is data, not behavior; it rides with its struct. |

### 3.2 Where the current rule contradicts itself

- **`extractor.rs` vs `guards.rs`.** The settings extractor evaluates
  `if`-branch truthiness, joins branch environments, demotes knowledge,
  and micro-evaluates `Path(__file__).parent / "templates"` — analysis
  of comparable sophistication to the guard-negation walk in
  `python/analysis/guards.rs`. One lives in djls-project as a "static
  source recognizer"; the other lives in djls-semantic as "semantic
  vocabulary production". The actual distinguisher is *which crate
  needed to call it first*, an accident of plan 008's derivation
  needing registrations but not specs.
- **`RegistrationKind` is cut in half.** Its definition and scanner
  live in djls-project; its behavior (`symbol_kind`, `extract`,
  `extract_tag_rule`…) lives in a semantic-side extension trait. Plan
  015's own maintenance note flags this seam as the one to watch
  ("any growth there is a sign the scanner/bridge line was drawn
  wrong") — the line is already carrying that strain.
- **"Semantic vocabulary" proves too much.** `TemplateLibraries`,
  `TemplateSymbol`, `TemplateSymbolKind`, `LibraryName` are all
  template-domain vocabulary and all moved to djls-project in this very
  PR, because they are *observed* facts. The settled precedent is
  already: observed template-domain facts live project-side;
  interpreted meaning lives semantic-side. `TagRule` ("this compile
  function rejects fewer than 2 bits") is an observed fact by that
  precedent.
- **The docs strain to describe it.** `ARCHITECTURE.md:98` has to say
  semantic's `python/` "consumes Ruff ASTs and registration facts from
  `djls-project::extraction`" — a lower layer's parse artifacts flowing
  *up* into the meaning crate so it can do more lower-layer work on
  them. Under the desired seam that sentence disappears: ASTs never
  leave djls-project; only facts cross the boundary.

## 4. Options

### Option A — Status quo, sharpen the docs

Keep the output-vocabulary rule; document it deliberately in
ARCHITECTURE.md ("spec extraction stays in semantic because its outputs
are the semantic layer's input language") and strike the inconsistency
by reframing `extraction/` in djls-project as "the recognizers
djls-project's own queries need".

- **Pros**: zero churn; plan 017 executes as written; no test/snapshot
  relocation; PR #668 closes the structural track.
- **Cons**: the boundary cannot be stated in one sentence without
  naming the exception; djls-semantic remains ~2/3 Python machinery
  (the exact condition plan 015's "Why this matters" complained about:
  "Today two-thirds of djls-semantic is not template semantics");
  semantic keeps ruff in its manifest forever; the orphan-rule bridge
  trait persists; `ModelGraph` — a consumer-less derived Django fact —
  stays in the meaning crate; every future recognizer forces the same
  "who calls it first" coin flip.

### Option B — Move the whole `python/` subtree into djls-project

All of `analysis/`, `blocks/`, `models/`, `signature.rs`, `filters.rs`,
`types.rs`, the registry bridge, and the extraction queries from
`python.rs` move down. djls-semantic keeps the two fusion queries
(`compute_tag_specs`, `compute_filter_arity_specs`) and everything
template-side; both import `TagRule`/`BlockSpec`/`FilterArity` from
`djls_project` the same way they already import `TemplateLibraries`.

Sub-design points (decided here so a future plan doesn't have to):

- **Two recognizer tiers, honestly labeled.** The plan-006 purity
  firewall (`rg "salsa|djls_source" crates/djls-project/src/extraction/`
  → no matches) must survive. The pure recognizers (`blocks/`,
  `signature.rs`, `filters.rs`, `models/extract.rs`, `types.rs`) can
  join `extraction/`. But `analysis/` is *not* pure — `CallContext`
  threads `&dyn djls_source::Db` so `analyze_helper` can resolve helper
  calls through the interned salsa query (`python.rs:85`,
  `analysis/calls.rs`). It and the tracked queries land *outside*
  `extraction/`, as a sibling module (e.g. `specs.rs` + `specs/`
  beside `settings.rs`), mirroring how `settings.rs` wraps the pure
  extractor today. The firewall keeps its exact current meaning;
  djls-project simply gains a second salsa-assisted recognizer the way
  it already has one for settings.
- **Trait accessors.** `Db::model_graph()` (and nothing else) moves
  consideration: the cleanest shape is to keep fusion accessors
  (`tag_specs`, `filter_arity_specs`) on the semantic trait and let
  `model_graph` follow `template_libraries`' precedent — a
  djls-project query fronted wherever its eventual consumer needs it.
  Since it has no consumers today, the accessor can simply move down
  the trait stack with the query; nothing breaks.
- **API width.** djls-project's façade gains `TagRule`, `BlockSpec`,
  `FilterArity`, `SymbolKey`, `ExtractionResult`, `ModelGraph`, the
  three `extract_*` queries, `compute_model_graph`, and `extract_rules`
  (the non-salsa entry that djls-bench (`specs.rs:211-216`,
  `benches/extraction.rs:14`) and the corpus tests call). That is real
  but bounded growth, and plan 017's export-audit discipline (every
  `pub use` needs an external consumer) applies on arrival. Nothing
  semantic-side is re-exported through djls-project or vice versa — the
  no-shim rule holds.
- **Mechanical fallout** (sized, not prescribed): the 13 golden
  `djls_semantic__python__tests__golden_*.snap` files and
  `tests/corpus.rs`/`tests/corpus_models.rs` move (snapshot renames
  with byte-identical content, the plan-017 Step 2 technique);
  `djls-corpus` becomes a djls-project dev-dependency; djls-bench
  re-points `djls_semantic::extract_*` → `djls_project::`; djls-db's
  incrementality tests re-point imports (salsa ingredient names are
  function names, so `was_executed("extract_filter_arities")`
  assertions survive unchanged); CHANGELOG, ARCHITECTURE.md §"Python
  Static Analysis", AGENTS.md crate lines, and CONTEXT.md glossary
  entries update.

- **Pros**: the boundary becomes one sentence ("djls-project observes
  source; djls-semantic decides meaning") *and* one manifest check
  (only djls-project depends on ruff); the orphan-rule bridge dissolves
  (net code deletion); `ModelGraph` sits with its category;
  djls-semantic finally matches its AGENTS.md billing with no trailing
  clause; future recognizers (e.g. plan 018's environment library scan)
  have an unambiguous home.
- **Cons**: ~10k lines of churn in an M/L mechanical move immediately
  after 015, while 016/017 are queued; djls-project grows to ~17k
  lines and "project model" must honestly stretch to "project model +
  source observation" (the AGENTS.md line already says "static source
  recognizers", so the stretch is in degree, not kind); review burden
  for rename-detection on a deep subtree.

### Option C — Move only `python/models/`

`ModelGraph` is the unambiguous misfit (derived Django fact, no
consumers, no template content, already `ProjectDb`-shaped). Move it;
defer the tag/filter/block spec machinery.

- **Pros**: small, immediately coherent, removes the clearest wart.
- **Cons**: perpetuates the inconsistent rule for the 8k-line majority;
  ruff stays in semantic's manifest; the bridge trait stays; if B
  happens later, the ecosystem pays for two migrations of the same
  kind. Worth doing only if B is *rejected*, not as a stepping stone.

### Option D — Move the walkers, keep the vocabulary in semantic

Recorded to kill it: impossible. djls-project code cannot return
djls-semantic types without inverting the dependency. Wherever the
producer lives, its output types live at or below it. This is why "but
`TagRule` is semantic vocabulary" cannot decide the question — the
vocabulary's home is determined by its producer's home, and the real
question is only where the producer belongs.

## 5. Recommendation

**Option B**, executed as a new plan (next free number), sequenced:

1. **After PR #668 merges.** Do not widen #668 — it is a validated
   structure-only move with its own gates; this is a second, separable
   structure-only move.
2. **Before plans 016 and 017** (both still TODO). 016 wires the shared
   test database against final crate homes — landing it before the move
   means rewiring it after; 017's Step 3 (`python/queries.rs`) is
   *mooted* by the move (the queries leave the crate instead of moving
   sideways), its Step 4 export audit list changes, and its
   out-of-scope/maintenance notes assert the rule this memo overturns.

Consequential edits this recommendation implies (for the plan author,
not for now):

- **Amend plan 017**: drop Step 3, re-scope Step 4's candidate list,
  delete the "stays in djls-semantic permanently" maintenance note, and
  re-anchor the lib.rs line-count expectations (the python re-exports
  leave with the subtree).
- **Plan 015 needs no amendment** — it is shipped history; record the
  boundary re-decision in `plans/README.md`'s reconciliation log
  instead, superseding the 2026-06-10 "permanently" note with this
  memo's rationale.
- **Docs move with the code**: AGENTS.md's djls-semantic line drops
  "Python spec extraction" and its billing shifts from "Django
  template meaning" to **Django project meaning** (template analysis
  as its current expression — maintainer framing, 2026-06-11);
  djls-project's line gains spec extraction alongside its existing
  "static source recognizers"; ARCHITECTURE.md's "Python Static
  Analysis" section (`:157-171`) relocates under djls-project and the
  §1.3-quoted sentence about ASTs flowing upward disappears.
- **Adopt the activity classifier in CONTEXT.md** so the next boundary
  call doesn't re-litigate: *observed source facts → djls-project;
  project meaning (fusion, validity, availability, diagnostics) →
  djls-semantic; only djls-project parses Python.*

Why B over A, in the end: plan 015's stated purpose was to make
djls-semantic "finally what its name and AGENTS.md claim". Option A
leaves the majority of the crate outside that claim and keeps a
boundary that can only be explained by narrating plan 008's dependency
needs. Option B makes the boundary self-explaining and
machine-checkable, at the cost of one more mechanical move performed at
the cheapest moment it will ever have (016/017 not yet started, the
subtree verified trait-clean, all crossings already funneled through
two fusion queries).

## 6. Open Questions

1. **Module naming in djls-project.** Does the salsa-assisted
   recognizer tier live at `specs.rs`/`specs/` (this memo's working
   name), or under a broader `analysis/`? Related: does `extraction/`
   eventually want internal grouping (`extraction/settings*`,
   `extraction/registry`, plus the pure spec recognizers) once it
   approaches 5k lines? Naming should be settled in the plan, not
   during execution.
2. **`extract_rules`' public status.** The non-salsa convenience entry
   exists for corpus snapshots and benches. Does it stay `pub` on
   djls-project's façade, or do corpus/bench callers move close enough
   (post-016 djls-testing) that it can narrow? Interacts with plan
   016's corpus consolidation.
3. **`SymbolKey` vs `names.rs`.** `SymbolKey` (module path + name +
   kind) overlaps in spirit with djls-project's validated name newtypes.
   Merge into `names.rs` on arrival, or keep it with `types.rs` as
   extraction-output vocabulary? Small, but it decides whether the move
   is purely mechanical or includes one micro-unification.
4. **Will Python-file meaning need AST access in the meaning layer?**
   Two points are settled by maintainer framing (2026-06-11):
   `ModelGraph` and its accessor stay — deliberate groundwork for
   features that operate on Python files directly (models.py,
   settings.py, and other typical Django files) — and that meaning
   lands in djls-semantic, because the crate is the *project*-meaning
   layer, not a template-meaning layer. The residual question is
   narrower: when those features arrive (hover, completions,
   diagnostics *in* a Python file), can observed facts carry enough
   structure for them (spans/positions, the way `ModelDef` already
   carries its line), or will the meaning layer need an offset→context
   map over Python ASTs — the analogue of `offset.rs` — which would
   soften the "only djls-project parses Python" manifest check from an
   invariant into a default? Doesn't affect this memo's
   recommendation; worth deciding when the first Python-file feature
   is designed.
5. **`structure/` and djls-templates.** Out of scope here, but the
   recorded-deferred wish to move template-tree building into
   djls-templates becomes the *only* remaining "mechanical plumbing in
   semantic" conversation once `python/` moves. Worth re-examining only
   after this boundary settles.
