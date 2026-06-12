# Plan 018: Distinguish "unknown" from "not in INSTALLED_APPS" with an evidence-backed environment scan

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: This plan builds on plans 007 and 008 and is
> written before they have executed. Before starting verify ALL of:
> 1. `rg -n "fn template_libraries" crates/` finds a `#[salsa::tracked]`
>    derived query (plan 008 landed) — note its file; this plan calls it
>    `<libraries module>` throughout.
> 2. `rg -n "fn django_settings" crates/` finds the Django settings query
>    (plan 007 landed).
> 3. `rg -n "enum Knowledge" crates/djls-project/` finds the unified
>    3-state enum with a `Partial` variant (plans 006/007).
> 4. `rg -n "S118|S119|S121|NotInInstalledApps" crates/` returns **no
>    matches** (plan 002's deletion still holds; nothing has squatted on the
>    retired codes).
> 5. `rg -n "fn registration_modules" crates/` finds the method on
>    `TemplateLibraries`.
>
> If any check fails, STOP and report which one. If plan 015 has executed,
> the project model lives in `crates/djls-project/` instead of
> `crates/djls-semantic/src/project/` — all `project/` paths below shift
> accordingly (use the rg locations from checks 1–5 as ground truth); the
> validator paths (`validation.rs`, `validation/scoping.rs`, `errors.rs`)
> stay in djls-semantic either way.
>
> **Drift adaptation (ratified 2026-06-12)**: the tri-state enum shipped as
> `StaticKnowledge` (exported from `djls_project::extraction`), and the field
> on `TemplateLibraries` is `knowledge`, not `active_knowledge`. Read every
> `Knowledge` in this plan as `StaticKnowledge` and every `active_knowledge`
> as `TemplateLibraries::knowledge`; drift check 3's pattern is
> `enum StaticKnowledge`. The shipped name is correct — it scopes the
> confidence to static extraction, not runtime Django truth — so do NOT
> rename the code to match this plan. One further drift:
> `registration_modules()` returns empty only under `Unknown` (the plan said
> "unless Known"). Do not "fix" that; this plan already forbids using
> `registration_modules()` for the inactive-set subtraction — iterate
> loadable and builtins directly.

## Status

- **Priority**: P2
- **Effort**: M
- **Risk**: MED (new user-visible diagnostics; additive, but honesty rules are the contract)
- **Depends on**: plans/007, plans/008 (hard); plans/009 recommended first (pure deletion, avoids churn overlap); plans/015 soft (paths move — see drift check)
- **Category**: direction (static Django discovery — UX follow-up)
- **Planned at**: jj/git commit `7671145d`, 2026-06-10
- **Execution status**: source-complete locally at `83a40294` +
  `1504d527` + `98136c83`; not pushed/merged

## Execution record — local source stack (2026-06-12)

Implemented as three source commits:

1. `83a40294` — `add inactive template library scan`
2. `1504d527` — `feat: restore not-in-INSTALLED_APPS diagnostics from static facts`
3. `98136c83` — `test: cover inactive template library diagnostics end-to-end`

Drift adaptation applied as planned: every planned `Knowledge` reference maps
to `StaticKnowledge`, and `TemplateLibraries::knowledge` is the active-set
gate. No code was renamed.

Implementation notes:

- Added `crates/djls-project/src/environment.rs` with
  `InactiveLibrary`, `InactiveLibraries`, and
  `inactive_template_libraries(db, project)`.
- The scan walks every search path using the same root-kind walk policy and
  nested non-first-party exclusion rule as model discovery, keeps only Django
  `templatetags/*.py` package shapes, and subtracts modules already present in
  active `TemplateLibraries`.
- Restored S118/S119/S121 in `ValidationError` and upgraded only the
  `StaticKnowledge::Known` unknown-tag/filter/library paths. Partial and
  Unknown still suppress absence claims.
- Added e2e coverage for inactive `django.contrib.flatpages` with
  `{% load flatpages %}` → S121 and `{% get_flatpages as pages %}` → S118.
- Added docs and changelog entries for the restored static diagnostics.

Divergences recorded:

- `crates/djls-project/src/sync.rs` was added to the source scope after a
  Lamport review found a real stale-state trace: root revision bumps reran
  inactive discovery, but unchanged per-file revisions could keep parsed
  inactive-library symbols stale. `refresh_external_data` now also bumps every
  discovered templatetag candidate path via `templatetag_candidate_paths`, and
  `refresh_external_data_updates_inactive_template_library_symbols` covers the
  old counterexample.
- The invalidation test is behavioral rather than event-log based: it edits an
  inactive templatetag file, calls `refresh_external_data`, and proves the new
  tag appears. This directly covers the required state transition.
- The literal guard `rg -n "Discovered|discovery_knowledge" crates/` has one
  unrelated pre-existing match in `crates/djls-project/src/templates.rs`:
  `tracing::debug!("Discovered {} total template origins", ...)`. The old
  `discovery_knowledge` field and old discovered-library machinery remain
  absent.

Validation passed on the final stack:

- `cargo test -q`
- `just test`
- `just e2e`
- `just clippy`
- `just fmt --check`
- `just lint`
- Targeted checks: `cargo test -q -p djls-project inactive`,
  `cargo test -q -p djls-project discover_templatetag`,
  `cargo test -q -p djls-project refresh_external_data_updates_inactive_template_library_symbols`,
  `cargo test -q -p djls-semantic`
- Review: Lamport re-review reported no must-fix findings after the refresh
  fix.

**Review verdict (2026-06-12): approved.** Independently re-verified on the
stack head `98136c83`: `cargo test -q`, `just clippy`, `just fmt --check`, and
`just e2e` (27 passed, including the new flatpages S121/S118 assertions) all
exit 0; the guard sweeps confirm `discovery_knowledge` has no matches, the
sole literal `Discovered` match is the pre-existing template-origin debug log,
`InactiveLibraries` carries no knowledge field, and `registration_modules()`
is not used for the subtraction (active set = loadable modules + builtins,
read directly). The honesty gating is structurally sound: `Unknown` early-
returns, the `Partial` guard arms are untouched, and the inactive-candidate
upgrade lives only inside arms reachable under `Known`. The two divergences —
the `sync.rs` refresh fix (real stale-state bug, behavioral test covers the
counterexample) and the behavioral rather than event-log invalidation test —
are both improvements over the plan text. The executor's rewordings of done
criteria 2 and 7 were checked against the code and are factually accurate;
ratified (note for future runs: prefer reporting a criterion as failed-as-
written plus a divergence note, and let review amend the wording). The two
`pub(crate)` visibility widenings in `resolve.rs`/`settings.rs` are wiring
within the planned module scope. Remaining: push and PR when Josh says go.

## Why this matters

"Unknown tag" and "you installed the package but forgot INSTALLED_APPS" are
different user situations that deserve different diagnostics. Plan 002
deleted the S118/S119/S121 `NotInInstalledApps` diagnostics because nothing
ever produced their data — the "Discovered library" half of
`TemplateLibraries` was populated only by test fixtures. Plan 002's
maintenance note explicitly sanctioned this plan: *"if that work wants these
diagnostics back, it should reintroduce them fed by extraction facts, not
resurrect this plumbing."* After plan 008, DJLS derives the **active**
library set statically from `INSTALLED_APPS`; this plan adds the missing
half of the evidence — a scan of the search paths for template-tag libraries
that exist in the environment but are **not** activated — and reintroduces
the three diagnostics with the same codes and messages, now backed by real
facts. The result: `{% load flatpages %}` stops being a bare "unknown
library" and becomes "Add 'django.contrib.flatpages' to INSTALLED_APPS".

## Design constraints (read before coding)

These are the honesty rules. They are the contract, not suggestions:

1. **Positive evidence only.** The environment scan upgrades messages; it
   never suppresses or asserts absence. If the scan misses a library (odd
   layout, unreadable dir), the user gets today's S108/S111/S120 — correct,
   just less helpful. Because incompleteness is safe by construction, the
   new fact type carries **no `Knowledge` field** — no consumer would ever
   branch on it (this repo rejects behaviorless type distinctions).
2. **Emit only under `Knowledge::Known`.** "This library is not active" and
   "this app is not in INSTALLED_APPS" are absence claims over the active
   set and the installed-apps list. Under `Partial` or `Unknown` neither is
   provable, so the new diagnostics are never emitted there. (Plan 008
   Step 3 already suppresses the absence-claim diagnostics S108/S111/S120
   under `Partial`; this plan's lookups live inside those same gated paths.)
3. **Soundness by subtraction, not by app matching.** The inactive set is
   `(everything the scan found) − (modules in the active TemplateLibraries)`.
   Under `Known`, plan 008 guarantees every installed app's libraries are in
   the active set (any resolution failure demotes to `Partial`), so the
   remainder is *provably* "exists but not activated by this project". No
   fragile string-matching of scan results against `INSTALLED_APPS` entries.
4. **No dual-source merge.** Do NOT add a `Discovered` variant back to
   `LibraryStatus`, do NOT add fields to `TemplateLibraries`, do NOT touch
   `apply`-style merge paths. The inactive set is a **separate fact** with a
   single consumer (the validator). That separation is the lesson of plan
   002 — the old design died because it merged a fake source into the real
   type.
5. **No runtime, no fallback, no cache.** The scan is a Salsa tracked query
   over the same search paths and filesystem trait everything else uses.

## Current state

(Excerpts verified at `7671145d`. Plans 007/008 will have reshaped the
gating and the library producer by execution time — the drift check at the
top, plus per-step notes, cover that. Validator/error excerpts below are
content-stable except where noted.)

### What the user sees today

- `crates/djls-semantic/src/errors.rs:117-139` — live diagnostic codes:
  S100–S103, S108–S117, S120, S122, S123. **S118, S119, S121 are retired**
  (deleted by plan 002 at commit `3913dce8`) and reserved for this plan.
- The deleted variants, from `jj file show -r 6bc3b07d
  crates/djls-semantic/src/errors.rs` — reintroduce these shapes and
  messages verbatim (same codes, same user-facing contract):

  ```rust
  #[error("Add '{app}' to INSTALLED_APPS to use tag '{tag}'")]
  TagNotInInstalledApps {
      tag: String,
      app: String,
      load_name: String,
      span: Span,
  },

  #[error("Add '{app}' to INSTALLED_APPS to use filter '{filter}'")]
  FilterNotInInstalledApps {
      filter: String,
      app: String,
      load_name: String,
      span: Span,
  },

  #[error("Add '{app}' to INSTALLED_APPS to use template tag library '{name}'")]
  LibraryNotInInstalledApps {
      name: String,
      app: String,
      candidates: Vec<String>,
      span: Span,
  },
  ```

  Code mapping was `S118` / `S119` / `S121`; `primary_span()` returned the
  `span` field for all three.

### The validator and scoping rules

- `crates/djls-semantic/src/validation.rs:49-73` — `TemplateValidator`
  caches `template_libraries: &'a TemplateLibraries` from
  `db.template_libraries()` and threads `active_knowledge` into the scoping
  rules at `:128-174`. This plan adds one more cached reference (the
  inactive set) the same way.
- `crates/djls-semantic/src/validation/scoping.rs:19-57` —
  `check_tag_scoping_rule`: early-returns unless
  `active_knowledge == Knowledge::Known`, then matches
  `symbols.check(name)`; the `TagAvailability::Unknown` arm emits
  `ValidationError::UnknownTag` (S108). `check_filter_scoping_rule`
  (`:61-97`) mirrors it for filters (S111).
  `check_load_libraries_rule` (`:100-135`) flags `{% load %}` names missing
  from `template_libraries.loadable` as `UnknownLibrary` (S120).
  **Plan 008 Step 3 converts the early returns into a three-way
  Known/Partial/Unknown policy** — this plan's additions slot into the
  `Known` arm only.

### The library types (post-plan-002 shape)

- `crates/djls-semantic/src/project/symbols.rs:65-82` — `LibraryStatus`
  has exactly `Active { module, origin }` and `Builtin { module }`;
  `TemplateLibrary { name: LibraryName, status, symbols }`. Do not extend
  either (constraint 4).
- `symbols.rs:180-186` — `TemplateLibraries { active_knowledge, loadable:
  BTreeMap<LibraryName, Vec<TemplateLibrary>>, builtins:
  BTreeMap<PyModuleName, TemplateLibrary>, builtin_order }`.
- `symbols.rs:208-224` — `registration_modules()` collects loadable +
  builtin modules but returns `Vec::new()` unless knowledge is `Known`.
  For the subtraction in Step 1, iterate `loadable` values' `.module()`
  and the `builtins` keys **directly** so the inactive set is well-defined
  regardless of knowledge (the Known-gate belongs to the validator, not
  the fact).
- `symbols.rs:57-63` — `LibraryOrigin { app: PyModuleName, module:
  PyModuleName, path: Utf8PathBuf }` — precedent for typing the owning app
  as `PyModuleName`.

### The scan machinery already exists

- `crates/djls-source/src/fs.rs:131-142` — the `FileSystem` trait:
  `read_to_string`, `exists`, `is_file`, `is_dir`,
  `walk_entries(root, &WalkOptions)`.
- `fs.rs:26-72` — `WalkOptions` with constructors `project()` (respects
  ignore files) and `library_search_path()` (unrestricted). Use the same
  per-root-kind choice the model walker makes.
- `crates/djls-semantic/src/project/resolve.rs:149-206` — `model_modules`:
  the exemplar tracked query. It touches every search-path root's
  `revision` (`:155-163`) so watcher/refresh bumps re-run the query,
  computes `excluded_paths` so first-party roots don't double-walk nested
  site-packages (`:165-175`), and resolves file→module collisions by
  longest search path (`:177-199`). **Mirror all three behaviors.**
- `resolve.rs:280-356` — `discover_model_files_excluding`: the exemplar
  walker. Walks a root, filters entries by structural shape (here:
  `models.py` or `.py` under a `models/` package with `__init__.py`),
  applies `excluded_roots`, maps relative paths to `ModulePath`, returns a
  sorted Vec. The new walker is a sibling of this function.
- **Cost precedent**: `model_modules` already walks *every* search path
  (site-packages included) in production today. The new scan adds a second
  walk of the same roots with the same options — same asymptotics, Salsa-
  cached, re-run only on root-revision bumps. No new performance class.
- Symbol extraction seam (plan 013):
  `crate::python::collect_registrations_from_body` → `RegistrationInfo` /
  `RegistrationKind` (pure, db-free), paired with the cached
  `parse_python_module(db, File)` — the same pairing plan 008 Step 1 uses
  for active libraries. Do not re-implement registration detection.

### Severity and docs

- Severity is config-driven, not per-variant:
  `crates/djls-conf/src/diagnostics.rs:52-66` `get_severity(code)` does
  exact-then-longest-prefix lookup with default `Error`
  (`test_get_severity_default` at `:95-99`). New codes need **no severity
  code** — users can already tune them by prefix (docs show `"S12" =
  "warning"`).
- `docs/template-validation.md` is hand-written (no cog markers). It
  documents S120 at `:46` and `:77-81` and the partial-knowledge
  suppression list at `:132`. This plan adds the three restored codes and
  amends the suppression section.

### The e2e fixture has a ready-made test case

`tests/project/djls_test/settings.py:35-44` — `INSTALLED_APPS` contains
`django.contrib.admin/auth/contenttypes/sessions/messages/staticfiles/humanize`
and `djls_app`. **`django.contrib.flatpages` is absent**, and every Django
install ships `django/contrib/flatpages/templatetags/flatpages.py`
(registering the `get_flatpages` tag). So in the e2e venv:
`{% load flatpages %}` must produce S121 naming
`django.contrib.flatpages`, and an unloaded `{% get_flatpages %}` must
produce S118 — both with zero fixture-package installation.

## Commands you will need

| Purpose      | Command                          | Expected on success |
|--------------|----------------------------------|---------------------|
| Build        | `cargo build -q`                 | exit 0              |
| Test (crate) | `cargo test -q -p djls-semantic` | exit 0, all pass    |
| Test (all)   | `cargo test -q`                  | exit 0, all pass    |
| Rust matrix  | `just test`                      | exit 0 (cargo tests via nox — does NOT run tests/e2e) |
| E2E suite    | `just e2e` (or `nox -s e2e`)     | exit 0 — pytest-lsp over tests/project |
| Lint         | `just clippy`                    | exit 0, no warnings |
| Format       | `just fmt`                       | exit 0              |
| Hooks        | `just lint`                      | exit 0              |

## Scope

**In scope** (the only files you should modify):
- The `<libraries module>` from drift-check 1 and/or a new sibling file
  (e.g. `project/environment.rs` — `folder.rs` convention) for the scan
  query + walker + `InactiveLibraries` type
- `crates/djls-semantic/src/project.rs` / `crates/djls-semantic/src/lib.rs`
  (or djls-project equivalents post-015) — module wiring + any needed
  re-export
- `crates/djls-semantic/src/db.rs` (and the `SemanticDb` impl site, e.g.
  `crates/djls-db/src/db.rs`) — only if the validator reaches the new query
  through the db trait; prefer calling the tracked query directly if the
  validator already holds a `ProjectDb`
- `crates/djls-semantic/src/validation.rs`
- `crates/djls-semantic/src/validation/scoping.rs`
- `crates/djls-semantic/src/errors.rs`
- Test fixtures/helpers for the new tests (testing module per plan 016's
  layout if it has landed; `crates/djls-semantic/src/testing.rs` otherwise)
- `tests/project/templates/` (one new template) and
  `tests/e2e/test_diagnostics.py`
- `docs/template-validation.md`
- `CHANGELOG.md` (user-facing: three diagnostics restored, now
  statically derived)

**Out of scope** (do NOT touch, even though they look related):
- `LibraryStatus`, `TemplateLibrary`, `TemplateLibraries` — no new
  variants, no new fields (constraint 4).
- Plan 008's `template_libraries` derivation and its Partial-gating policy
  in scoping — this plan adds arms inside the `Known` path only; the
  Partial/Unknown behavior is plan 008's contract and must not change.
- Hover, completions, code actions — no completions for inactive
  libraries, no "add to INSTALLED_APPS" quick fix. Deferred (see
  Maintenance notes).
- `model_modules` / `discover_model_files_excluding` — mirror them; do not
  refactor them to share code in this plan.
- The corpus, benches, and the startup track files.

## Version-control workflow

jj repo — never run mutating `git` commands. Commit per step group;
suggested messages: `"add inactive template library scan"`,
`"feat: restore not-in-INSTALLED_APPS diagnostics from static facts"`.
Do NOT push.

## Steps

### Step 1: The environment scan walker

Sibling to `discover_model_files_excluding` (same module or the new
`environment.rs`):

```rust
fn discover_templatetag_files(
    fs: &dyn FileSystem,
    base_dir: &Utf8Path,
    root_kind: FileRootKind,
    excluded_roots: &[Utf8PathBuf],
) -> Vec<(PyModuleName /* owning app */, LibraryName, Utf8PathBuf)>
```

Walk with the same per-root-kind `WalkOptions` choice as
`discover_model_files_excluding` (`resolve.rs:298-301`). Keep a file entry
iff ALL of:
- extension is `py`, file stem does not start with `_`;
- parent directory is named `templatetags` and contains `__init__.py`
  (`fs.exists`) — Django's structural requirement;
- the `templatetags` directory's parent is itself a package directory with
  `__init__.py` strictly below `base_dir` (the owning app package).

The owning app module = the app package dir's path relative to `base_dir`,
joined with dots (e.g. `django/contrib/flatpages` →
`django.contrib.flatpages`). The library load name = the file stem
(`LibraryName::parse` — skip entries it rejects). Apply `excluded_roots`
exactly as the model walker does. Sort the result deterministically.

**Verify**: unit test in the same file (use `InMemoryFileSystem`, model
after `discover_model_files`' tests in `resolve.rs`): a fake root with
`pkg_a/templatetags/{__init__.py,foo.py,_private.py}`,
`pkg_b/templatetags/bar.py` (no `__init__.py` in templatetags), and
`loose/templatetags/baz.py` where `loose/` has no `__init__.py` → exactly
one result: `(pkg_a, foo, .../foo.py)`.
`cargo test -q -p djls-semantic discover_templatetag` → pass.

### Step 2: The `inactive_template_libraries` query

In the same module:

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InactiveLibrary {
    pub name: LibraryName,     // load name, e.g. "flatpages"
    pub app: PyModuleName,     // what to add to INSTALLED_APPS
    pub module: PyModuleName,  // <app>.templatetags.<stem>
    pub tags: Vec<String>,
    pub filters: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InactiveLibraries {
    pub by_name: BTreeMap<LibraryName, Vec<InactiveLibrary>>,
}

#[salsa::tracked(returns(ref))]
pub(crate) fn inactive_template_libraries(
    db: &dyn ProjectDb,
    project: Project,
) -> InactiveLibraries
```

Composition:
1. Iterate `project.search_paths(db)`, touching each root's `revision`
   exactly as `model_modules` does (`resolve.rs:155-163`), with the same
   first-party `excluded_paths` computation (`:165-175`). Collect
   candidates via Step 1's walker; dedup by file path with
   longest-search-path precedence (`:177-199`).
2. Subtract the active set: drop any candidate whose `module` equals an
   active module — iterate `template_libraries(db, project)`'s `loadable`
   values' `.module()` and its `builtins` keys directly (NOT
   `registration_modules()`, which empties itself under non-Known).
3. For each survivor: `db.get_or_create_file(&path)` → cached
   `parse_python_module(db, file)` → `collect_registrations_from_body`
   (the plan-008 pairing). Keep it only if a `Library` registration is
   found; collect tag/filter names into `tags` / `filters` (sorted).
4. Build `by_name`; within a name, sort candidates by `(app, module)` for
   deterministic diagnostics.

Add lookup helpers on `InactiveLibraries` (plain methods, no new types):

```rust
pub fn library_candidates(&self, name: &LibraryName) -> &[InactiveLibrary];
pub fn tag_candidates(&self, tag: &str) -> Vec<&InactiveLibrary>;     // sorted by (app, name)
pub fn filter_candidates(&self, filter: &str) -> Vec<&InactiveLibrary>;
```

(If profiling during this plan shows the linear `tag_candidates` scan
matters, precompute two `BTreeMap<String, Vec<usize>>` indexes at
construction — but do not add them speculatively.)

**Verify**: fixture test (use the fixture builder from plan 014, or
`TestDatabase` + `InMemoryFileSystem` directly): a project with
`INSTALLED_APPS = ["myapp"]` where the search paths also contain
`crispy/templatetags/crispy.py` registering one tag — the query returns
one `InactiveLibrary { name: crispy, app: crispy, .. }` with that tag, and
`myapp`'s own library does NOT appear (subtraction).
`cargo test -q -p djls-semantic inactive` → pass.

### Step 3: Restore the three error variants

In `errors.rs`: add the three variants exactly as excerpted in "Current
state" (same field names, same `#[error(...)]` messages). Add `code()`
arms `S118`/`S119`/`S121` and the three `span` lines in `primary_span()`.

**Verify**: `cargo build -q` → exit 0;
`rg -n "S118|S119|S121" crates/djls-semantic/src/errors.rs` → three code
arms.

### Step 4: Thread the inactive set through the validator

- `validation.rs`: alongside the existing
  `template_libraries: &'a TemplateLibraries` field (`:49`), cache
  `inactive: &'a InactiveLibraries` (fetched once, next to
  `db.template_libraries()` at `:64` — through whichever access path the
  post-008 validator uses for the active set; match it). Pass it to the
  three scoping rules.
- `scoping.rs`, inside the `Knowledge::Known` arms only:
  - `check_tag_scoping_rule`, `TagAvailability::Unknown` arm: if
    `inactive.tag_candidates(name)` is non-empty, emit
    `TagNotInInstalledApps { tag, app, load_name, span: full_span }` from
    the **first** candidate (the `(app, module)` sort makes this
    deterministic) instead of `UnknownTag`; otherwise `UnknownTag` as
    today. The `Available`/`Unloaded`/`AmbiguousUnloaded` arms are
    untouched — an *active* but unloaded library always wins over an
    inactive candidate.
  - `check_filter_scoping_rule`: mirror, with
    `FilterNotInInstalledApps`.
  - `check_load_libraries_rule`: when a load name is not in
    `template_libraries.loadable`, consult
    `inactive.library_candidates(&name)`: non-empty → emit
    `LibraryNotInInstalledApps { name, app: first.app, candidates: all
    apps in order, span }` instead of `UnknownLibrary`; empty →
    `UnknownLibrary` as today.
  - Under the post-008 `Partial` policy these code paths are already
    suppressed (absence claims); make no change there. If after plan 008
    the `Partial` arm still emits any of S108/S111/S120, that contradicts
    plan 008's Step 3 — STOP and report rather than layering on top.

**Verify**: `cargo build -q` → exit 0, then Step 5's tests.

### Step 5: Unit tests for the policy

In the scoping/validation test module (model after the existing
`UnknownTag`/`UnloadedTag` tests; use the plan-014 fixture builder), cover
at minimum:

1. Unknown load name + inactive candidate → S121 (and **no** S120).
2. Unknown load name, no candidate → S120 (unchanged).
3. Unknown tag + inactive candidate → S118 (and no S108); message names
   the app and the load name.
4. Unknown tag, no candidate → S108 (unchanged).
5. Unknown filter + inactive candidate → S119.
6. Tag available from an *active* unloaded library AND an inactive one →
   S109 `UnloadedTag` (active wins; no S118).
7. `active_knowledge = Partial`, inactive candidate exists → **no
   diagnostic at all** (neither S120 nor S121; honesty rule 2).
8. Two inactive apps providing the same load name → S121 lists both in
   `candidates`, `app` is the lexicographically first.

**Verify**: `cargo test -q -p djls-semantic` → exit 0 including the 8 new
tests.

### Step 6: E2E against the real fixture project

1. Confirm `rg -n "flatpages" tests/project/djls_test/settings.py` returns
   nothing (still uninstalled). If it matches, pick another Django contrib
   app that ships `templatetags/` and is absent from `INSTALLED_APPS` —
   verify by listing `django/contrib/*/templatetags/` in the e2e venv —
   and substitute it below.
2. Add `tests/project/templates/not_in_installed_apps.html` containing a
   `{% load flatpages %}` line and (in a separate template region) an
   unloaded `{% get_flatpages as pages %}` usage.
3. In `tests/e2e/test_diagnostics.py`, following the existing
   test pattern (see the unloaded-`static` assertions referenced around
   `test_diagnostics.py:12`), assert: the load line yields exactly one
   diagnostic with code `S121` whose message contains
   `django.contrib.flatpages`; the tag line yields `S118` (not `S108`).
4. Check whether any existing e2e test asserts a *whole-project* diagnostic
   count or template inventory that the new template file disturbs; adjust
   only such counts, nothing else.

**Verify**: `just e2e` → exit 0 including the new assertions. Also re-run
the full gates: `cargo test -q`, `just test`, `just clippy`, `just fmt`,
`just lint` → all exit 0.

### Step 7: Docs and changelog

- `docs/template-validation.md`: restore rows/sections for S118, S119,
  S121 describing the **static** evidence ("the library exists on the
  project's Python search paths but its app is not in INSTALLED_APPS");
  update the partial-knowledge suppression list (`:132` area) to state
  that S118/S119/S121 are likewise suppressed under partial knowledge,
  and why (absence claims need a complete active set). Hand-edit — this
  file has no cog blocks (verified).
- `CHANGELOG.md`: user-facing entry per repo conventions.

**Verify**: `rg -n "S118|S119|S121" docs/template-validation.md` → rows
present.

## Test plan

- Step 1 walker unit test (structural filtering: `__init__.py` rules,
  underscore stems, app attribution).
- Step 2 query fixture tests (subtraction soundness; symbols collected).
- Step 5's eight policy tests — these encode the honesty contract and are
  the heart of the plan.
- Step 6 e2e: real Django, real venv, `flatpages` end-to-end for both the
  library-level and tag-level diagnostics.
- Invalidation (one test, event-log pattern like the plan-008 rewrites):
  creating/deleting a `templatetags/` module under a search root and
  bumping that root's revision re-runs `inactive_template_libraries`.

## Done criteria

Machine-checkable. ALL must hold:

- [x] `rg -n "S118|S119|S121" crates/djls-semantic/src/errors.rs` shows the three code arms
- [x] Old discovered-library machinery stayed dead: `discovery_knowledge` has no matches; the only literal `Discovered` match is an unrelated pre-existing template-origin debug log in `crates/djls-project/src/templates.rs`
- [x] `rg -n "Knowledge" crates/djls-project/src/environment.rs` shows no `Knowledge` field on `InactiveLibraries` (honesty rule 1; the validator gate lives in scoping.rs)
- [x] `cargo test -q` exits 0, including the 8 policy tests + walker + query + invalidation tests
- [x] `just test` exits 0 AND `just e2e` exits 0 (including the new flatpages assertions)
- [x] `just clippy`, `just fmt --check`, `just lint` all exit 0
- [x] Modified files are in the intended project/semantic/test/docs scope plus `crates/djls-project/src/sync.rs` for the reviewer-found refresh invalidation fix (`jj diff --stat` recorded in execution report)
- [x] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- Any drift-check item at the top fails.
- The post-008 scoping code does not match the Known/Partial/Unknown
  three-way shape this plan assumes (report the actual shape).
- e2e shows S121/S118 for a library that **is** active in the fixture
  project — that is a subtraction-soundness violation in Step 2 and means
  an active module string didn't match the scan's module attribution;
  report the exact module pair, do not paper over it with name matching.
- The scan misattributes an owning app for a namespace package (a
  templatetags parent without `__init__.py` that Django nonetheless
  loads). Exclude such candidates (rule: no `__init__.py`, no claim) and
  report it; do not guess.
- `just e2e` wall-time regresses noticeably after adding the scan (the
  model_modules precedent says it shouldn't; if it does, report numbers
  rather than adding caching).
- You need a new field on `TemplateLibraries` or a new `LibraryStatus`
  variant to make anything work — that's the plan-002 anti-pattern;
  report instead.

## Maintenance notes

- **Natural follow-up (deferred)**: a code action on S121/S118/S119 that
  edits `settings.py` to append the app — needs the settings-extraction
  span data from plan 006 to know where `INSTALLED_APPS` ends. Also
  deferred: completions for inactive library names in `{% load %}` (would
  pair with the code action).
- The first-candidate choice in S118/S119 (lexicographic `(app, module)`)
  is arbitrary when multiple inactive apps provide the same tag. If users
  report confusion, the fix is including all candidates in the message
  (the S121 variant already carries `candidates` for exactly this).
- Reviewers: scrutinize Step 4's policy edits against the honesty rules —
  the only acceptable behavior change is S108→S118, S111→S119, S120→S121
  *upgrades* under `Known`; any diagnostic appearing under `Partial`/
  `Unknown` that didn't before is a bug.
- If plan 015 lands after this one, the scan module moves with the rest of
  the project model into `djls-project` — it has no semantic-crate
  dependencies beyond the registry seam (whose scanner half 015 moves into
  djls-project's extraction module), so the move stays mechanical.
- Watcher integration (deferred "File watching" item in
  `plans/README.md`): the query already depends on root revisions, so
  installing a new package into the venv will be picked up as soon as
  watching bumps search-path roots — no extra work here.
