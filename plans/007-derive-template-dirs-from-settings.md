# Plan 007: Wire settings extraction into Salsa and derive template directories statically

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat 922cc4d7..HEAD -- crates/djls-semantic/src/project crates/djls-semantic/src/resolution.rs crates/djls-db/src`
> Plans 001–006 are prerequisites and HAVE changed these files since the
> planned-at SHA — that is expected. What must be true before starting:
> `crates/djls-project` exists with its `extraction` module and passing
> tests (plan 006); `Project` has no
> `template_files` field (plan 004); the project handle is a plain field
> (plan 003). Verify each; if any prerequisite is missing, STOP.

## Status

- **Priority**: P1
- **Effort**: L
- **Risk**: MED-HIGH
- **Depends on**: plans/003, plans/004, plans/006, plans/013 (renamed probe), plans/014 (fixtures + goldens) (001 transitively)
- **Category**: direction (static Django discovery)
- **Planned at**: commit `922cc4d7`, 2026-06-10

## Why this matters

This is the step where the language server learns template directories from
`settings.py` **source** instead of asking a Python subprocess. The chain
becomes: settings module file → tracked parse → `djls_project::extraction` walker →
`SettingsFacts` → derived `template_dirs` query — every link a Salsa query
over `File` inputs, so editing `settings.py` invalidates exactly the right
things. The runtime inspector's `template_dirs` query and the
`TemplateDirs::Unknown/Known` input plumbing are deleted (clean break; no
`source|runtime` config toggle — that dual path tripled PR #606's pipeline).

## Current state

(After plans 001–006; line numbers below are from the planned-at SHA and may
have shifted — match on content.)

- `crates/djls-semantic/src/project/input.rs:182-184` — the input field to
  delete, and its enum at `:18-36`:

  ```rust
  /// Template-directory introspection state.
  pub enum TemplateDirs { #[default] Unknown, Known(Vec<Utf8PathBuf>) }
  ...
  /// Template directories reported by project introspection.
  #[returns(ref)]
  pub template_dirs: TemplateDirs,
  ```

- `crates/djls-semantic/src/project/sync.rs:95-143` — the inspector push to
  delete: `TemplateDirsRequest` / `TemplateDirsResponse` /
  `refresh_template_dirs` (queries the subprocess, compares, sets input).
  Called from `refresh_external_data` (`sync.rs:43`).

- `crates/djls-semantic/src/project/input.rs:172-174` — the settings module
  name is already on the input:

  ```rust
  /// Django settings module (e.g., "myproject.settings")
  #[returns(ref)]
  pub django_settings_module: Option<String>,
  ```

- Module-path → file resolution: plan 013 renamed the generic probe to
  `module_file_in_search_path` (formerly `templatetag_module_file`,
  `crates/djls-semantic/src/project/resolve.rs:208-225`) — it probes
  `<search_path>/<a/b/c>.py` then `<a/b/c>/__init__.py`. The
  settings-module resolver below CALLS it (do not copy-paste it) against
  `project.search_paths(db)`.

- Consumers of `template_dirs` today:
  - `SemanticDb::template_dirs()` (`crates/djls-semantic/src/db.rs:16`,
    impl at `crates/djls-db/src/db.rs:146-153`) — returns
    `Option<Vec<Utf8PathBuf>>`
  - `TemplateOrigins` stores a `TemplateDirs` for "tried paths" reporting
    (`crates/djls-semantic/src/resolution.rs:110-113`, used at `:130-140`)
  - `project_template_files` (created by plan 004) reads it to walk
  - `rg -n "template_dirs" crates/ --no-heading` for the full live list

- Semantic's `Knowledge` (`project/symbols.rs:169-172`) is
  `{ Known, Unknown }`; `djls_project::extraction::Knowledge` (plan 006)
  adds `Partial`.

- Cycle recovery: **already proven in this repo on the exact pinned salsa
  (0.26.2)** — `crates/djls-semantic/src/python.rs:115-118` uses
  `#[salsa::tracked(cycle_initial=analyze_helper_cycle_initial, cycle_fn=analyze_helper_cycle_recover)]`
  with helper fns at `python.rs:149-165`. Copy THAT shape, not ty's.
  Salsa 0.26 specifics (verified in the macro source): `cycle_initial=`
  **alone** is valid and gives fixpoint semantics — since this plan's
  policy is "cycle → empty/Unknown facts" (no iteration), omit `cycle_fn`
  entirely. Callback contract: `cycle_initial(db, salsa::Id, ...one param
  per query input) -> Output` — `settings_facts_for_file(db, file, project)`
  has TWO inputs, so a closure form needs four parameters
  (`|_, _, _, _| ...`). Constraint: `no_eq` cannot combine with `cycle_fn`.
  ty's analogous query for reference: `dunder_all_names`
  (`reference/ruff/crates/ty_python_semantic/src/dunder_all.rs:15-16`,
  cross-module recursion at `:155-163`).

## Commands you will need

| Purpose      | Command                          | Expected on success |
|--------------|----------------------------------|---------------------|
| Build        | `cargo build -q`                 | exit 0              |
| Test (crate) | `cargo test -q -p djls-semantic` | exit 0, all pass    |
| Test (db)    | `cargo test -q -p djls-db`       | exit 0, all pass    |
| Test (all)   | `cargo test -q`                  | exit 0, all pass    |
| Rust matrix  | `just test`                      | exit 0 (cargo tests via nox — does NOT run tests/e2e) |
| E2E suite    | `just e2e` (or `nox -s e2e`)     | exit 0 — the parity gate (needs venv; if unavailable, note it) |
| Lint         | `just clippy`                    | exit 0, no warnings |
| Format       | `just fmt`                       | exit 0              |
| Hooks        | `just lint`                      | exit 0              |

## Scope

**In scope** (the only files you should modify):
- `crates/djls-semantic/src/project/settings.rs` (create — the queries)
- `crates/djls-semantic/src/project/{input,sync,resolve,symbols,templates}.rs`
- `crates/djls-semantic/src/project.rs`, `src/lib.rs` (exports),
  `src/db.rs` (trait signature), `src/resolution.rs`
- `crates/djls-semantic/Cargo.toml` (add djls-project — this establishes
  the end-state `djls-semantic → djls-project` dependency edge; plan 015
  later moves the project model down into that crate)
- `crates/djls-db/src/db.rs` (SemanticDb impl)
- `crates/djls-semantic/src/testing.rs` and affected tests across
  djls-semantic / djls-db / djls-ide
- `tests/` e2e expectations if template-dir-dependent

**Out of scope** (do NOT touch, even though they look related):
- `template_libraries` and its inspector refresh — plan 008.
- The inspector itself (`introspector.rs`, the cache, server phase-1) —
  plans 008/009. After this plan the inspector still runs but its
  `template_dirs` query is simply never sent.
- Adding a `django_discovery = "source" | "runtime"` config toggle —
  explicitly rejected. Static is THE path.
- `INSTALLED_APPS`-driven app templatetags / libraries — plan 008 (this plan
  only consumes `installed_apps` for APP_DIRS template directories).

## Git workflow

jj repo — no mutating `git`. Commit per step group, e.g.
`"add settings facts queries"`, `"refactor: derive template dirs from settings facts"`.
Do NOT push.

## Steps

### Step 1: Unify `Knowledge`

Delete the `Knowledge` enum from `project/symbols.rs:169-172` and replace
every use with `djls_project::extraction::Knowledge` (re-export it from
`crate::project` at the same path consumers already import). Existing
`!= Knowledge::Known` gating sites (`validation/scoping.rs:31,87,140`)
compile unchanged and treat `Partial` as not-Known — behavior-preserving for
now (plan 008 gives `Partial` its real semantics).

**Verify**: `cargo test -q` → all pass.

### Step 2: Resolve the settings module to a `File`

In new `crates/djls-semantic/src/project/settings.rs`:

```rust
#[salsa::tracked]
pub(crate) fn settings_module_file(db: &dyn ProjectDb, project: Project) -> Option<File> {
    let dsm = project.django_settings_module(db).as_deref()?;
    // probe each search path via module_file_in_search_path (plan 013's
    // rename of the generic probe in resolve.rs): first hit wins;
    // depend on root revisions like model_modules does (resolve.rs:155-163).
    ...Some(db.get_or_create_file(&path))
}
```

**Verify**: a unit test with `TestDatabase` + `InMemoryFileSystem` containing
`/proj/myproject/settings.py` and DSM `"myproject.settings"` resolves to that
file; a missing module returns `None`.

### Step 3: The extraction query with star-import recursion

In `settings.rs`:

```rust
#[salsa::tracked(returns(ref), cycle_initial = ..., cycle_fn = ...)]
pub(crate) fn settings_facts_for_file(db: &dyn ProjectDb, file: File, project: Project)
    -> djls_project::extraction::SettingsFacts
{
    let source = file.source(db);            // tracked read
    let path = file.path(db);
    let mut resolver = SalsaStarImports { db, project, base: path };
    djls_project::extraction::extract_settings(source.as_str(), path, &mut resolver)
}
```

`SalsaStarImports::resolve`: turn the `StarImport` (level + module) into an
absolute module path (relative levels resolve against the current file's
module location — pop one path segment per dot, mirroring how
`ModuleName::from_import_statement` behaves in ty), resolve to a `File` via
the same search-path probing as Step 2, and recurse through
`settings_facts_for_file` — returning that result's env/facts. Cycle
recovery: `cycle_initial=` alone, seeding empty/Unknown `SettingsFacts` —
copy the in-repo helper-fn shape from `python.rs:149-165` (see Current
state for the exact salsa-0.26 callback contract; no `cycle_fn` needed
since we don't iterate).

Then the project-level entry point:

```rust
#[salsa::tracked(returns(ref))]
pub(crate) fn django_settings_facts(db: &dyn ProjectDb, project: Project)
    -> djls_project::extraction::SettingsFacts
{ /* settings_module_file → settings_facts_for_file; None → all-Unknown facts */ }
```

**Verify**: `TestDatabase` tests: base/prod split settings
(`from .base import *` + `INSTALLED_APPS += [...]`) produce merged Known
facts; a self-importing cycle does not hang or panic; editing the base file's
content (bump revision with new source) re-runs extraction (event-log test
pattern from `crates/djls-db/src/db.rs:458-489`).

### Step 4: Derive template directories

In `settings.rs`:

```rust
#[salsa::tracked(returns(ref))]
pub(crate) fn template_dirs(db: &dyn ProjectDb, project: Project)
    -> (Vec<Utf8PathBuf>, Knowledge)
```

Composition (Django semantics — keep this order):
1. For each backend in `template_backends` whose `backend` is
   `"django.template.backends.django.DjangoTemplates"` or unknown-but-sole:
   its resolved `DIRS` entries, in order.
2. If `app_dirs == Some(true)`: for each entry in `installed_apps.values`,
   resolve the app to a directory on the search paths (probe
   `<root>/<app/path>` as a package directory; entries that look like
   AppConfig paths — contain `.apps.` or end in `Config` — resolve their
   parent module; unresolvable entries add a `Reason` and demote to
   `Partial`), and include `<app_dir>/templates` when it exists, after all
   `DIRS` (Django checks DIRS before app dirs).
3. Resulting `Knowledge` = the weakest of the contributing facts
   (`installed_apps.knowledge` only matters when `app_dirs` is true).
4. `PathValue::Unknown` entries: skip + demote to `Partial`.

**Verify**: unit tests — DIRS-only project; APP_DIRS with two apps (one
first-party package dir, one site-packages package dir under an
InMemoryFileSystem); AppConfig-string entry; unresolvable app → Partial.

### Step 5: Swap consumers and delete the old plumbing

- `SemanticDb::template_dirs()` (trait `djls-semantic/src/db.rs:16`, impl
  `djls-db/src/db.rs:146-153`): change the signature to return what
  consumers actually need. Survey first (`rg -n "template_dirs" crates/`):
  if all consumers want the dirs list, return
  `Option<Vec<Utf8PathBuf>>` built from the query (None when no project);
  keep the trait shape minimal rather than leaking `Knowledge` where nothing
  branches on it.
- `resolution.rs`: `TemplateOrigins.template_dirs: TemplateDirs` field →
  `Vec<Utf8PathBuf>` (the "tried paths" listing at `:130-140` just needs the
  dirs; `.as_known()` disappears). `template_origins` (`:150-172`) reads the
  new query.
- `project_template_files` (plan 004's query in `project/templates.rs`):
  read the new `template_dirs` query instead of the input field.
- Delete: `TemplateDirs` enum + input field + builder arg (`input.rs`),
  `refresh_template_dirs` + its request/response types + the call
  (`sync.rs:43,95-143`), the `TemplateDirs` re-export (`project.rs:17`), and
  any lib.rs re-export. Update all `Project::builder` call sites (one fewer
  arg).

**Verify**: `cargo build -q` → exit 0;
`rg "TemplateDirs|refresh_template_dirs" crates/` → no matches;
`cargo test -q` → failures only in tests being updated this step.

### Step 6: Test sweep + e2e

Update `TestDatabase` (its `template_dirs()` fixture override returns `None`
today — keep an override seam OR route fixtures through real files in the
InMemoryFileSystem via plan 014's `ProjectFixture` builder; prefer the
builder for new tests, keep the override for untouched ones). Run the full
suite, then the e2e suite.

**Verify**: `cargo test -q` → all pass. `just e2e` → e2e against
`tests/project` passes — the real proof: the fixture project's template
dirs derived from its actual `settings.py` must match the inspector-era
results, including plan 014's enriched fixture (non-empty
`DIRS = [BASE_DIR / "templates"]` plus APP_DIRS app template dirs). The
golden fixture `tests/fixtures/django-facts/django-5.2.json` (plan 014)
lists the expected dirs with `${PROJECT}`/`${SITE_PACKAGES}` placeholders —
add a comparison test against its `template_dirs` entry (same venv-gated
mechanism plan 008 Step 5 describes). If they differ, see STOP conditions.

## Test plan

- New unit tests listed in Steps 2–4 (settings resolution, star-import
  layering, cycle safety, dirs composition, APP_DIRS resolution).
- One incrementality test: edit settings source → `template_dirs` re-runs;
  unrelated file edit → it does not (event-log pattern,
  `djls-db/src/db.rs:236-244`).
- E2E (`just test`): unchanged expectations — the same diagnostics and
  template resolution as the inspector produced for `tests/project`.

## Done criteria

Machine-checkable. ALL must hold:

- [ ] `rg "TemplateDirs|refresh_template_dirs|template_dirs\(db\)" crates/djls-semantic/src/project/input.rs crates/djls-semantic/src/project/sync.rs` returns no matches
- [ ] `rg "enum Knowledge" crates/djls-semantic/` returns no matches (single definition in djls-project's extraction module)
- [ ] `cargo test -q` exits 0 and `just test` exits 0
- [ ] `just e2e` exits 0 — the parity gate (or documented as environment-unavailable with everything else green)
- [ ] `just clippy` exits 0
- [ ] Only in-scope files modified (`jj diff --stat`)
- [ ] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- E2E template dirs differ from the inspector-era results on
  `tests/project` — report the exact diff (missing dir, extra dir, order).
  Do NOT patch expectations to match; the difference is either an extraction
  bug or a real fidelity gap that needs a human decision.
- (Retired: cycle recovery is verified working in-repo on salsa 0.26.2 at
  `python.rs:115-165`.) Only if the two-input callback contract still
  surprises you — report the actual macro error.
- You need information from the inspector to make extraction work (that
  inverts the dependency this plan exists to break).
- `SemanticDb::template_dirs` consumers turn out to branch on
  Known-vs-Unknown in ways a plain `Option` cannot express.

## Maintenance notes

- After this plan, `refresh_external_data` is down to source-roots refresh,
  the library refresh (dies in plan 008), and python-module revision bumps.
  When it shrinks to nothing but bumps, consider renaming it to what it is.
- The APP_DIRS app-resolution here is deliberately lighter than Django's full
  AppConfig machinery (no `apps.py` `default = True` parsing). PR #606's
  `app_registry.rs` has a faithful implementation worth consulting if real
  projects hit the gap: `jj file show -r static-project-model-consolidated-settings-facts crates/djls-semantic/src/project/app_registry.rs`.
- Reviewers: check invalidation granularity — editing a *template* must not
  re-run settings extraction (only `settings.py`-chain file revisions feed
  these queries).
