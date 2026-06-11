# Memo: PR #659 extraction vocabulary vs. ruff/ty — reconciliation and recommendation

- **Subject**: PR #659 "add djls-project settings extraction walker"
  (https://github.com/joshuadavidthomas/django-language-server/pull/659),
  source commit `731d353b`, implementing
  `plans/006-create-djls-project-settings-recognizer.md`
- **Question**: is the "facts" vocabulary (`SettingsFacts`, `StringListFact`,
  `TemplateBackendFact`, `Knowledge`, `Reason`) faithful to the ruff/ty
  inspiration, or a regression toward the old PR-#606/#626 shape?
- **Date**: 2026-06-10
- **Amendment applied**: PR #659 was amended at source commit `c6bd8ac2` to
  apply this memo's recommendation: `DjangoSettings`, `StringListSetting`,
  `TemplateBackend`, closed `Reason`, `extractor`/`SettingsExtractor`, and
  no public crate exports until plan 007 has a real external consumer.
  The analysis below preserves citations to the original `731d353b` shape
  that prompted the review.
- **Verdict in one line**: the *structure* is a faithful ruff/ty port with one
  documented, justified deviation; the *naming* is partly inherited from the
  deleted old-PR shape — drop the "facts" vocabulary from the type layer
  entirely (`SettingsFacts` → `DjangoSettings`, `StringListFact` →
  `StringListSetting`, `TemplateBackendFact` → `TemplateBackend`), keep
  `Knowledge`, and centralize `Reason` as a closed enum. Amend the PR before
  merge.
- **Revision note**: the first draft of this memo recommended keeping
  `SettingsFacts` and keeping `Reason` as a free string; both calls were
  revised after review discussion (see §7 for the corrected reasoning).

## 1. What the PR actually implements

Verified by reading every file at `731d353b` (`jj file show -r 731d353b …`):

- `crates/djls-project/src/extraction/walker.rs` (1,339 lines, 26 unit
  tests): a bounded statement walker over `ruff_python_parser::parse_module`
  output. Module-level statements only; recognizes `INSTALLED_APPS`
  assignment/`+=`/`+`-chain/`.append`/`.extend`/`.insert`/`.remove`,
  the `TEMPLATES` literal shape (BACKEND/DIRS/APP_DIRS/OPTIONS
  libraries+builtins), `TEMPLATES[i]["DIRS"]` mutations, star-import layering
  via callback, and `if`/`elif`/`else` with a private
  `Truthiness { AlwaysTrue, AlwaysFalse, Ambiguous }` evaluator
  (walker.rs:643-666). Ambiguous branches walk all arms, join the branch
  environments, and demote written names to `Partial`. Reassignment clears
  prior state (the dunder_all `update_origin` rule). Unrecognized writes
  latch the affected name to `Unknown` — per-name, not per-module.
- `crates/djls-project/src/extraction/paths.rs` (144 lines): closed-grammar
  path micro-evaluator (`Path(__file__)`, `.parent`, `/ "lit"`, `.joinpath`,
  `os.path.join`/`dirname`, `str(...)`), everything else
  `PathValue::Unknown`.
- `crates/djls-project/src/extraction/facts.rs`: the boundary types
  (inventory in §2).
- Purity holds: the manifest depends only on `camino`, `ruff_python_ast`,
  `ruff_python_parser`, `rustc-hash` — no salsa, no I/O, no djls-source, no
  djls-semantic. `extract_settings(&str, &Utf8Path, &mut dyn
  StarImportResolver) -> SettingsFacts` is the whole contract.

This matches plan 006 step for step, including the plan's STOP-condition
boundaries (no list-method emulation, no alias tracking, no fourth
`Knowledge` variant).

## 2. The terminology and output model it exposes

From `facts.rs` at `731d353b` (line numbers at that rev):

| Type | Line | Shape |
|---|---|---|
| `Knowledge` | :6 | `enum { Known, Partial, Unknown }` |
| `Reason` | :14 | `struct { message: String }` |
| `StringListFact` | :29 | `{ values: Vec<String>, knowledge: Knowledge, reasons: Vec<Reason> }` |
| `TemplateBackendFact` | :70 | `{ backend, dirs: Vec<PathValue>, app_dirs, libraries, builtins, knowledge, reasons }` |
| `SettingsFacts` | :105 | `{ installed_apps: StringListFact, template_backends: Vec<TemplateBackendFact>, templates_knowledge: Knowledge }` |
| `PathValue` | :113 | `enum { Resolved(Utf8PathBuf), Unknown(Reason) }` |
| `StarImport` / `StarImportResolver` | :120/:126 | caller-supplied recursion |
| `SettingsEnv` | :134 | walker working state, `into_facts()` finisher |

Crate doc (`extraction.rs`): "Static Extraction for Django project facts."
Every extracted datum carries a tri-state confidence plus zero or more
human-readable reason strings. Tests assert on `knowledge` and
`reasons.len()` — never on message text.

## 3. How ruff/ty model the comparable problems

All verified by direct reads of `reference/ruff/`:

**ruff's `__all__` extraction** (`ruff_python_semantic/src/model/all.rs`):
`extract_dunder_all_names(&self, stmt) -> (Vec<DunderAllName>, DunderAllFlags)`
(:73-78). `DunderAllName { name: &str, range: TextRange }` (:25-31) — a plain
domain datum, no wrapper. Caveats travel as bitflags
(`DunderAllFlags { INVALID_FORMAT, INVALID_OBJECT }`, :12-19): extraction
keeps going past a bad element, data + flags return together, and *rules*
downstream interpret the flags into diagnostics.

**ty's `__all__` extraction** (`ty_python_semantic/src/dunder_all.rs`):
`dunder_all_names(db, file) -> Option<FxHashSet<Name>>` (:15-17, a salsa
query with `cycle_initial = None`). Walker state is
`{ origin: Option<DunderAllOrigin>, invalid: bool, names: FxHashSet<Name> }`
(:27-44). One unrecognized idiom sets `invalid`; `into_names()` then returns
`None` with only a `tracing::debug!` line (:198-209). All-or-nothing, silent
failure.

**ty's module resolver** (`ty_module_resolver`): `resolve_module(db, file,
name) -> Option<Module>` (resolve.rs:57-66) — silent `None`, no error
payload. `Module` is `enum { File(FileModule), Namespace(NamespacePackage) }`
(module.rs:14-19) — variants encode *what was found*, never "unresolved with
a reason". `SearchPathInner`'s seven variants (path.rs:419-427) encode
origin/priority, not confidence. Where ty genuinely needs partial knowledge
it mints a *named domain tri-state*: `TypeshedVersionsQueryResult { Exists,
DoesNotExist, MaybeExists }` (typeshed.rs:114-161) and `Truthiness {
AlwaysTrue, AlwaysFalse, Ambiguous }` (ty_python_core/src/lib.rs:947-955).
The resolver even ships the philosophy comment plan 006 quotes
(resolve.rs:1515-1521): syntax-only, can be fooled, "better than nothing".

**What does not exist anywhere in ruff/ty** (swept by a thorough subagent
search, spot-verified by my own reads): no `Fact`/`Facts` type or API
vocabulary; no generic confidence wrapper attached to extracted data; no
per-datum human-readable reason strings. `Reason` *does* appear as a type
name — but only as small rule-local enums (e.g.
`ruff_linter/src/rules/pyupgrade/rules/outdated_version_block.rs:84-88`,
`enum Reason { AlwaysTrue, AlwaysFalse, Invalid }`) whose variants select a
diagnostic message at the reporting layer. Explanations are rendered
diagnostics or debug logs, never fields on the data.

## 4. Where the PR aligns with ruff/ty

- **The walker mechanics are a port, and a good one**: per-name invalid
  latch, reassignment-clears-state, recognized mutation-call set,
  module-level-only walking, statically-decided branch arms — each maps to a
  cited dunder_all technique, and I verified each against the reference.
- **`Truthiness` is ty's exact shape and name**, privately re-implemented
  (correct: djls pins only `ruff_python_ast`/`ruff_python_parser`, so it
  cannot import `ty_python_core`).
- **The partial-not-bail policy is ruff's model, not a violation of ty's.**
  Where ty bails to `None`, ruff's `extract_dunder_all_names` keeps the good
  elements and flags the bad ones. The walker does the ruff thing, says so in
  its module doc (walker.rs:1-8), and justifies it: a settings list with one
  `env(...)` entry is common, and a Partial list beats no list. `Knowledge` +
  `reasons` is functionally `(data, flags)` with a coarser rollup.
- **The pure boundary is ty's own posture**: source in, answers out, the
  star-import recursion inverted into a callback so the future salsa query
  (plan 007) owns I/O and cycles — mirroring `dunder_all_names` being the
  tracked query with `cycle_initial = None`.

## 5. Where it diverges, and the provenance of each divergence

The question was whether "facts" came from the old PRs rather than from
ruff/ty. The honest answer is layered:

- **It definitively did not come from ruff/ty** (§3).
- **"Facts" as *prose* is repo-canonical, not PR residue.** `CONTEXT.md`
  (the glossary AGENTS.md designates as canonical terminology, added in PR
  #620, 2026-05-19) defines **Project Facts** as *the* term — "_Avoid_:
  Project Model, Project Context, Project Knowledge, Project State"
  (CONTEXT.md:17-19) — and defines **Static Extraction** as deriving Project
  Facts from source (CONTEXT.md:33-34). The crate doc's "Static Extraction
  for Django project facts" is glossary-conformant.
- **The `-Fact` type suffix and free-string `Reason` are old-shape
  inheritance.** Plan 001:62 records the deleted `static_model.rs` defining
  `Confidence`, `Fact<T>`, `Reason`, `ImportRoot`, `ResolvedModule` — the
  PR-#606-era vocabulary. The research doc called the 4-state `Fact<T>`
  "mostly behaviorless ceremony"
  (`docs/agents/static-discovery-groundwork/research.md:24`). The new code
  fixed the *structure* (concrete per-setting structs, no generic wrapper, no
  behaviorless variants) but kept the *names*.
- **`Knowledge` is neither ruff/ty's nor the old PRs' — it is the
  engagement's own prescribed cure.** research.md:45 diagnosed
  "three-and-a-half competing 'not known yet' representations" as a root
  cause of #606's failure and prescribed a single one. A two-state
  `Knowledge { Known, Unknown }` already lives in shipped, serialized code
  (`djls-semantic/src/project/symbols.rs:156`, the `active_knowledge` field
  on `TemplateLibraries`); plan 006 makes the three-state canonical version
  and plan 007 Step 1 migrates semantic onto it. Each variant has distinct
  consumer behavior today and in the landed plans (Known → trust; Partial →
  use but demote derived knowledge / suppress strict diagnostics, see plan
  008's `active_knowledge != Known` gates; Unknown → fall back to
  introspection) — it passes the no-behaviorless-variants rule.
- **`Reason { message: String }` is the one genuinely precedent-free
  choice.** ruff/ty never attach strings to data; ruff uses bitflags +
  rule-local `Reason` *enums* at the diagnostic layer; ty uses silent `None`
  + `tracing::debug`.

## 6. Does broader ruff/ty module-resolution code support or contradict the PR's terminology?

Per item:

- **Tri-state confidence: supported.** `TypeshedVersionsQueryResult` is
  literally a knowledge tri-state (`MaybeExists` ≅ `Partial`), `Truthiness`
  another. ty's pattern is *per-domain named* tri-states rather than one
  shared enum; djls sharing one `Knowledge` across settings is a deliberate
  divergence justified by research.md:45 (the proliferation of bespoke
  "not known yet" types is what sank #606) and by uniform downstream gating.
- **"Fact" type names: unsupported.** ty names extraction outputs as domain
  data (`Module`, `DunderAllName`, `FxHashSet<Name>`); nothing carries a
  generic knowledge-vocabulary suffix.
- **Per-datum reason strings: contradicted.** Resolution failure is `None`;
  explanation lives in logs/diagnostics, never in the returned value.
- **Pure-walker + caller-owned resolution: strongly supported**
  (resolve.rs:1515-1521; `dunder_all_names` as the salsa seam).

## 7. Recommended direction for PR #659

**Amend before merge** — not because the PR repeats #606 (it does not; the
bounded contract held and the structure is right), but because this is the
last moment renames are free: the crate has zero consumers, and plan 007
(unexecuted) will freeze these names into salsa query signatures, semantic
call sites, and three plan documents.

1. **Rename `TemplateBackendFact` → `TemplateBackend`** and
   **`StringListFact` → `StringListSetting`.** These are domain objects (one
   `TEMPLATES` entry; one watched string-list setting), and ruff/ty name
   such things as what they are. The `-Fact` suffix is static_model.rs
   residue and adds nothing the field types don't already say. (Plan 018
   already follows the domain-noun style with `InactiveLibraries`.)
2. **Rename `SettingsFacts` → `DjangoSettings`.** The struct is concretely
   the static stand-in for Django's own settings object
   (`from django.conf import settings`) — name it what it is, like ty's
   `Module`. The glossary's "Project Facts" governs prose, not type names,
   and it was written in the same era as the old attempts (PR #620, between
   #606 and #626) — it is not evidence that types should carry the suffix.
   Bare `Settings` is ruled out concretely: `djls_conf::Settings`
   (`crates/djls-conf/src/lib.rs:72`) is already imported in
   `crates/djls-db/src/settings.rs` and `db.rs` — exactly the files plan 007
   wires the extraction output through — so the unprefixed name would force
   `as`-aliases at every seam. No existing `DjangoSettings` collides.
3. **Keep `Knowledge { Known, Partial, Unknown }`** unchanged, for the §5/§6
   reasons. Do not rename it to anything resolution-flavored; it is the
   single shared representation research.md prescribed.
4. **Convert `Reason` to a closed enum** (keep the name — ruff's rule-local
   `Reason` enums are direct precedent). The earlier draft kept the string on
   a no-behaviorless-variants argument, but that rule discriminates against
   enums whose *consumers* treat variants identically — here there are no
   branching consumers at all, so string and enum are equally behaviorless
   and the tiebreaker is organization. Centralized wins on every axis:
   - The enum *is* the failure-mode catalog — the negative space of the
     supported-shapes contract that plan 006's maintenance notes say must
     stay documented. Scattered string literals leave that catalog implicit.
   - The 25 inline strings at `731d353b` already drift toward near-duplicates
     ("string list operand is not statically supported" vs "string list
     element is not a string literal") and embed context the structure
     already carries (the reason sits in `installed_apps.reasons`, so the
     "INSTALLED_APPS." message prefixes are redundant). They collapse to
     roughly ten kinds: `SyntaxErrors`, `UnresolvedStarImport`,
     `UnsupportedAssignment`, `UnsupportedMutation`, `NonLiteralElement`,
     `NonLiteralKey`, `UnsupportedValue` (recognized key, wrong literal
     shape), `DictUnpack`, `AmbiguousCondition`,
     `UnsupportedPathExpression`. Granularity is the executor's call;
     payload-free and `Copy` is the target (branch joins currently clone
     `Vec<Reason>` of `String`s).
   - Tests can assert *which* reason instead of `reasons.len()` — a stronger
     contract with no brittle string matching.
   - A future consumer (plan 018's S-code mapping is the likely first) gets
     stable identities for free instead of a string-to-code migration.
   Rendering stays a `Display` impl producing the current messages. The
   write-only contract still applies until a consumer legitimately branches:
   gate behavior on `Knowledge`, render reasons in logs/diagnostics.
5. Mechanical fallout inside the crate: `into_facts()` → `into_settings()`;
   `extract_settings` keeps its name (it returns `DjangoSettings`); crate doc
   stays glossary-conformant prose ("Static Extraction for Django project
   facts" describes the activity, not the types); update the type-table doc
   comments; walker tests swap `reasons.len()` assertions for variant
   assertions where it sharpens them.

This keeps the PR aligned with ruff/ty where ruff/ty have an opinion
(structure, bounds, purity, domain-noun data types, no generic wrappers) and
diverges only where djls has a documented, repo-specific justification
(shared tri-state; reason strings for a user-facing LSP).

## 8. Concrete next steps (ordered)

1. **Amend PR #659** (executor task, small): the three renames from
   §7.1–7.2, the `Reason` enum conversion from §7.4, `into_facts()` →
   `into_settings()`, doc-comment touch-ups. Verify:
   `cargo test -q -p djls-project`, `just clippy`, `just fmt --check`,
   `rg -n "Fact" crates/djls-project/src/` returns no matches.
2. **Update plan 006** Step 2 type listing and §"Done criteria" prose to the
   amended names, and note the amendment in its Status block (it is DONE;
   record "amended at `<rev>`: component types renamed per
   `plans/memo-pr659-extraction-vocabulary.md`").
3. **Record the decision in `plans/README.md`** reconciliation log (entry
   drafted alongside this memo) so future plans don't reintroduce the
   suffix.
4. **Re-point the downstream plans** (advisor pass, before plan 007
   executes): plan 007 references `SettingsFacts` and the query names
   `settings_facts_for_file` / `django_settings_facts` (007:31,96,186-187,
   201-202,211-213) — retype to `DjangoSettings` and rename the queries to
   match (`django_settings_for_file` / `django_settings` is the natural
   pairing); plan 008's drift check names `django_settings_facts` (008:10)
   and plan 018's names it too (018:14) — update both. `Knowledge`
   references are untouched; prose mentions of "facts" stay (glossary
   vocabulary). Neither plan mentions `StringListFact`/`TemplateBackendFact`
   (verified).
5. **Future-shape guard**: when plan 007 wires the salsa side, keep reasons
   out of gating logic (gate on `Knowledge` only), and when plan 015 moves
   the registration scanner into `extraction/`, its outputs should follow
   the same convention — domain-noun types, shared `Knowledge`, no new
   confidence vocabulary.

## Appendix: claims and where each was verified

- PR contents: `jj file show -r 731d353b` for all five crate files +
  manifest.
- ruff flags model: `reference/ruff/crates/ruff_python_semantic/src/model/all.rs:12-78`.
- ty bail model: `reference/ruff/crates/ty_python_semantic/src/dunder_all.rs:15-209`.
- ty resolver: `reference/ruff/crates/ty_module_resolver/src/resolve.rs:50-95,1505-1530`;
  `module.rs:14-19`; `path.rs:419-427`; `typeshed.rs:114-161`.
- `Truthiness`: `reference/ruff/crates/ty_python_core/src/lib.rs:947-955`.
- Old vocabulary provenance: `plans/001-delete-static-scaffolding.md:33,62,181`;
  `docs/agents/static-discovery-groundwork/research.md:24,45`.
- Glossary: `CONTEXT.md:17-39` (added in PR #620 per `jj log -r 'files("CONTEXT.md")'`).
- Live `Knowledge`: `crates/djls-semantic/src/project/symbols.rs:156`.
- `Settings` name collision: `crates/djls-conf/src/lib.rs:72`
  (`pub struct Settings`), imported in `crates/djls-db/src/settings.rs` and
  `crates/djls-db/src/db.rs`; `rg -n "DjangoSettings" crates/` → no matches.
- "No Fact/Knowledge-wrapper/Reason-string anywhere in ruff/ty": thorough
  subagent sweep across `reference/ruff/crates/`, with every load-bearing
  citation re-read directly before use here.
