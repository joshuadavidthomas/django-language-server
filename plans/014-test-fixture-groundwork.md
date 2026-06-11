# Plan 014: Test fixture groundwork — project builder, enriched e2e fixture, golden Django facts

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat 922cc4d7..HEAD -- crates/djls-semantic/src/testing.rs crates/djls-semantic/src/project/resolve.rs crates/djls-semantic/src/resolution.rs tests/ noxfile.py Justfile`
> Content-match the excerpts below; mismatch beyond landed prerequisite
> plans = STOP.
> NOTE on ordering: despite the number, this plan runs BEFORE plans 007–009
> (see the README execution order). Parts B and C MUST run while the runtime
> inspector still works — they pin inspector-era ground truth that the
> static derivation will be measured against.

## Status

- **Priority**: P1
- **Effort**: M
- **Risk**: LOW
- **Depends on**: none hard (003 makes Part A slightly cleaner; do not wait on it)
- **Category**: tests (Tidy First pass for plans 004/007/008/009)
- **Planned at**: commit `922cc4d7`, 2026-06-10
- **Status**: DONE
- **Implemented at**: source commits `4e065e8f`, `b49cec5d`, `1e6c5368`, 2026-06-10
- **Bookmark**: `plan-014-test-fixture-groundwork`

## Why this matters

Three gaps make plans 007/008's test work expensive and their parity gates
weak:

1. **No fixture-project builder.** Tests that need a file-backed `Project`
   hand-roll the salsa-generated 11-arg `Project::new` in two duplicated
   local helpers plus one inline site. Every input-field removal
   (plans 004/007/008) touches all of them; and plans 007/008 need exactly
   this kind of fixture (settings.py + apps on an in-memory FS) in a dozen
   new tests.
2. **The e2e fixture proves almost nothing.** `tests/project` has **no
   `templatetags/` package anywhere** and `TEMPLATES[0]["DIRS"]` is empty —
   so the parity gates in plans 007/008 would pass trivially on the dirs
   side and exercise zero first-party library derivation.
3. **No durable ground truth.** Once plan 009 deletes the inspector, there
   is nothing to regenerate expected values from. Capturing the inspector's
   answers as **checked-in, normalized golden JSON** — with a small
   dev-only Python script kept as the regeneration tool — turns the parity
   contract into a living artifact: when `tests/project` grows or a new
   Django version enters the matrix, regenerate against real Django and the
   static derivation must keep up.

## Current state

- **The duplicated constructors** (all build `Project::new` with 11
  positional args):
  - `project_for_search_paths` — `crates/djls-semantic/src/project/resolve.rs:384-404`, used ~13× in resolve.rs tests
  - `project_with_templates` — `crates/djls-semantic/src/resolution.rs:346-382` (writes template files to the in-memory FS, injects `TemplateDirs::Known` + `ProjectTemplateFiles`), used 7×
  - inline `Project::new` — `crates/djls-semantic/src/project/resolve.rs:992-1004`
  - Production construction is already funneled: the only `Project::builder`
    call is inside `Project::bootstrap` (`project/input.rs:230-246`).
- **TestDatabase** (`crates/djls-semantic/src/testing.rs:143-213`): has
  `fs: Arc<Mutex<InMemoryFileSystem>>`, `add_file` (`:187-196`),
  `set_project` (`:198-200`), and fact-constant injection
  (`with_specs`/`with_template_libraries`, `:169-185`) — but no project
  builder.
- **tests/project layout**: `manage.py`, `djls.toml`
  (`django_settings_module = "djls_test.settings"`), app `djls_app/`
  (apps/models/admin/views, `templates/djls_app/*.html`), project package
  `djls_test/` (`settings.py`, urls, wsgi, asgi). `settings.py` constructs:
  `BASE_DIR = Path(__file__).resolve().parent.parent` (line 18),
  `INSTALLED_APPS = [8 string literals]` (35-44), `TEMPLATES` literal dict
  with `DIRS: []`, `APP_DIRS: True` (58-71). **Every construct is inside
  plan 006's supported-shape list** — no env() calls, no star imports, no
  conditionals.
- **How e2e runs**: `noxfile.py:112-129` — session `e2e` (py3.10):
  `uv sync --frozen` (pytest 9, pytest-lsp 1.0.0 via uv.lock), then
  `session.install("django==5.2")`, then pytest over `tests/e2e`
  (testpaths in `pyproject.toml:134`). Invoked by `just e2e` — **note:
  `just test` runs cargo tests only and never executes tests/e2e**.
- **The inspector's Python source** (the logic the golden generator
  reuses): `crates/djls-semantic/inspector/{__main__.py,inspector.py,queries.py}`
  — `queries.py` calls `django.setup()` and answers `template_dirs` /
  `template_libraries` queries. Do NOT move these files (the build embeds
  them via `build.rs` until plan 009); the new tool is a standalone copy of
  the *query logic*.
- Justfile recipe style: see existing recipes (`just corpus sync` pattern)
  — mirror their shape for `just fixtures`.

## Commands you will need

| Purpose      | Command                            | Expected on success |
|--------------|------------------------------------|---------------------|
| Build        | `cargo build -q`                   | exit 0              |
| Test (all)   | `cargo test -q`                    | exit 0, all pass    |
| E2E suite    | `just e2e` (or `nox -s e2e`)       | exit 0, all pass    |
| Lint         | `just clippy`                      | exit 0, no warnings |
| Format       | `just fmt`                         | exit 0              |
| Hooks        | `just lint`                        | exit 0              |
| Regenerate goldens | `just fixtures` (created in Part C) | exit 0, writes JSON |

## Scope

**In scope** (the only files you should modify/create):
- `crates/djls-semantic/src/testing.rs` (the builder)
- `crates/djls-semantic/src/project/resolve.rs`,
  `crates/djls-semantic/src/resolution.rs` (migrate test helpers)
- `tests/project/**` (fixture enrichment)
- `tests/e2e/test_completions.py`, `tests/e2e/test_diagnostics.py`
  (new assertions)
- `tools/django_facts.py` (create — the golden generator)
- `tests/fixtures/django-facts/` (create — golden JSON)
- `Justfile`, `noxfile.py` (the `fixtures` recipe/session)

**Out of scope** (do NOT touch, even though they look related):
- `crates/djls-semantic/inspector/` and `build.rs` — the embedded inspector
  stays untouched until plan 009.
- The mdtest harness (`testing.rs`/`mdtest.rs`) — investigated; wrong shape
  for settings-extraction tests, which use plain asserts.
- Production code paths — Part A is test-only code; Parts B/C are fixtures
  and tooling.

## Git workflow

jj repo — no mutating `git`. Three commits suggested, one per part:
`"test: add fixture project builder"`,
`"test: enrich e2e fixture project with first-party templatetags"`,
`"test: capture golden django facts with a dev-only generator"`. Do NOT push.

## Steps

### Part A — the fixture-project builder

**Step A1**: In `testing.rs`, add a builder that funnels all test `Project`
construction:

```rust
pub(crate) struct ProjectFixture { root: Utf8PathBuf, files: Vec<(Utf8PathBuf, String)>,
    django_settings_module: Option<String>, pythonpath: Vec<String>, /* ... */ }

impl ProjectFixture {
    pub(crate) fn new(root: impl Into<Utf8PathBuf>) -> Self { ... }
    pub(crate) fn file(mut self, path: ..., source: ...) -> Self { ... }
    pub(crate) fn django_settings_module(mut self, dsm: &str) -> Self { ... }
    /// Writes files into the db's InMemoryFileSystem, builds SearchPaths
    /// from the fixture's root/interpreter/pythonpath, registers roots,
    /// constructs the Project (single Project::new call site), and
    /// db.set_project()s it.
    pub(crate) fn build(self, db: &TestDatabase) -> Project { ... }
}
```

Keep optional setters for the facts the current helpers inject
(`template_dirs`, `template_libraries`, `template_files`) so migration is
mechanical; plans 004/007/008 delete those setters as the fields die.

**Step A2**: Migrate the three existing sites (`resolve.rs:384-404` +
its ~13 callers, `resolution.rs:346-382` + 7 callers, `resolve.rs:992-1004`)
to the builder. After this, the crate has exactly **two** `Project`
construction sites: `Project::bootstrap` (production) and
`ProjectFixture::build` (tests).

**Verify**: `cargo test -q -p djls-semantic` → all pass;
`rg -n "Project::new\(" crates/djls-semantic/src/` → exactly one match
(inside `ProjectFixture::build`); `rg -n "Project::builder" crates/` →
exactly one match (`input.rs`).

### Part B — enrich the e2e fixture project (inspector still running)

**Step B1**: Add a first-party templatetag library:
- `tests/project/djls_app/templatetags/__init__.py` (empty)
- `tests/project/djls_app/templatetags/djls_app_tags.py`:

  ```python
  from django import template

  register = template.Library()

  @register.simple_tag
  def djls_greeting(name):
      return f"hello {name}"

  @register.filter
  def djls_shout(value):
      return str(value).upper()
  ```

**Step B2**: Make `DIRS` non-trivial: create
`tests/project/templates/project_base.html` (any small template) and change
`tests/project/djls_test/settings.py` `TEMPLATES[0]["DIRS"]` from `[]` to
`[BASE_DIR / "templates"]`.

**Step B3**: Pin the inspector-era truth with e2e assertions:
- `test_completions.py`: a `{% load %}` completion test asserting
  `djls_app_tags` appears among library completions (model after the
  existing `static` test at `test_completions.py:48-71`).
- `test_diagnostics.py`: using `djls_greeting` without `{% load %}` yields
  the unloaded-tag diagnostic (S109-class), mirroring the existing
  assertions at `test_diagnostics.py:12`.

**Verify**: `just e2e` → all pass **with the current runtime inspector** —
this is the point: the assertions encode what the inspector reports, and
plans 007/008 must reproduce it statically.

### Part C — golden Django facts + the dev-only generator

**Step C1**: Create `tools/django_facts.py` — a standalone script (~80
lines), adapted from the query logic in
`crates/djls-semantic/inspector/queries.py` (read it first; reuse its
engine-registry access patterns, not its wire protocol):
- inputs: `--project <path>` and `--settings <module>` (default from the
  project's `djls.toml`)
- `django.setup()`, then collect: template dirs (engine `dirs` +
  app dirs), loadable libraries (load-name → module), builtins (ordered),
  and per-library symbol names/kinds
- normalize paths: replace the project root with `${PROJECT}` and the
  detected site-packages root with `${SITE_PACKAGES}` so output is
  machine-independent
- emit deterministic JSON (sorted keys) to stdout

**Step C2**: Wire `just fixtures`: a Justfile recipe (and a nox session if
the venv management needs it — mirror how `e2e` gets `django==5.2`
installed, `noxfile.py:112-129`) that runs the script against
`tests/project` and writes
`tests/fixtures/django-facts/django-5.2.json`.

**Step C3**: Run it; check in the golden file. Eyeball it: it must contain
the `djls_app_tags` library from Part B, the `static`/`i18n` libraries from
`django.templatetags`, the three default builtin modules, and both the
`${PROJECT}/templates` dir and the app template dirs.

**Verify**: `just fixtures` → exit 0, idempotent (running twice produces an
identical file — sorted keys, normalized paths). `just lint` → exit 0.

## Test plan

- Part A: existing migrated tests are the coverage; plus one new test using
  the builder end-to-end (fixture with a settings module path → project
  with expected search paths).
- Part B: the two new e2e assertions ARE the deliverable.
- Part C: idempotence check above; the golden file's real consumers arrive
  in plans 007/008 (their parity steps compare derived facts against this
  JSON — see the amendments in those plans).

## Done criteria

Machine-checkable. ALL must hold:

- [x] `rg -c "Project::new\(" crates/djls-semantic/src/` == 1
- [x] `tests/project/djls_app/templatetags/djls_app_tags.py` exists; `TEMPLATES[0]["DIRS"]` is non-empty in the fixture settings
- [x] `just e2e` exits 0 including the two new assertions (against the runtime inspector)
- [x] `tests/fixtures/django-facts/django-5.2.json` exists, contains `djls_app_tags`, `static`, `${PROJECT}`, `${SITE_PACKAGES}`; `just fixtures` is idempotent
- [x] `cargo test -q` exits 0; `just clippy` exits 0
- [x] Only in-scope files modified (`jj diff --stat`)
- [x] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- The new e2e assertions fail against the **runtime inspector** — that
  means the fixture enrichment itself is wrong (bad templatetag module,
  wrong settings edit); fix the fixture, never weaken the assertion.
- Site-packages root detection in the generator is ambiguous (multiple
  roots) — report rather than guessing a normalization.
- Migrating a test to the builder changes its behavior (different search
  paths or durabilities than the hand-rolled constructor produced) — the
  builder must reproduce the old fixtures exactly; report the difference.

## Maintenance notes

- **The generator is the surviving "form of the inspector"** — dev-only,
  never shipped, never run by the server. When `tests/project` changes or a
  new Django version joins the matrix, run `just fixtures` (per version)
  and commit the diff; plans 007/008's parity tests then enforce that
  static derivation keeps up. This replaces PR #606's in-production
  static-vs-runtime comparison harness with the same idea in the right
  place.
- Plan 009 deletes `crates/djls-semantic/inspector/` — `tools/django_facts.py`
  is deliberately independent of it (standalone copy, no zipapp, no wire
  protocol) so 009 stays a pure deletion.
- Multi-version goldens (`django-6.0.json`, …) are a one-recipe-loop
  extension when the matrix grows; deliberately not built now.
- Reviewers of plans 007/008: the golden file is the parity contract —
  treat unexplained regeneration diffs as regressions.
