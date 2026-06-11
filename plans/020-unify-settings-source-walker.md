# Plan 020: Compute the settings refresh footprint with the extractor's own walk

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: This plan was written against working-copy
> commit `710f4107` on branch `plan-008-derive-template-libraries-from-source`
> (PR #664), with then-uncommitted edits to
> `crates/djls-semantic/src/project/settings.rs` included in the excerpts
> below. Before starting, verify:
> `rg -n "fn collect_settings_source_files" crates/djls-semantic/src/project/settings.rs`
> returns one match, and
> `rg -n "ruff_python_parser::parse_module" crates/djls-semantic/src/project/settings.rs`
> returns one match (inside that function). On mismatch, content-compare the
> "Current state" excerpts; if the walker has already been replaced, STOP.

## Status

- **Priority**: P1 (fixes a live invalidation bug for the most common Django
  settings idiom)
- **Effort**: S/M
- **Risk**: LOW-MED (refresh path only; the new walk is the already-tested
  extractor)
- **Depends on**: plans/007, plans/008 (both DONE). Should land **before**
  plans/015 (which moves `settings.rs`/`sync.rs` into `djls-project` — move
  one walker, not two). Independent of plan 019 (different functions in the
  same file; if both run, sequence them rather than parallel).
- **Category**: bug + tech-debt
- **Planned at**: jj commit `710f4107`, 2026-06-11
- **Design rationale**: `plans/memo-settings-source-graph.md` — read it
  first; this plan implements its option C ("one walker, two drivers").

## Why this matters

Two walkers cover the settings star-import graph: the extractor in
`djls-project` (drives `django_settings`) and a hand-rolled scan in
`djls-semantic` (drives `settings_source_files`, which tells the refresh
boundary which `File` revisions to bump). The hand-rolled scan reads only
**top-level** `from X import *` statements, while the extractor follows star
imports inside any compound statement — including `try:` bodies. The classic
Django override idiom

```python
try:
    from .local_settings import *
except ImportError:
    pass
```

is therefore followed by extraction (Salsa records the dependency) but never
listed by the refresh walker — so a disk edit to `local_settings.py` survives
`refresh_external_data` with stale settings: the root-revision bumps make
`django_settings` re-run, but `file.source(local_settings)` returns the
unbumped memo. The fix deletes the second walker and computes the footprint by
running the extractor itself over **disk** content with a recording resolver,
making the bump set track the extraction read-set by construction.

## Current state

`crates/djls-semantic/src/project/settings.rs`:

The tracked extraction query (settings.rs:51-61):

```rust
#[salsa::tracked(returns(ref))]
pub(crate) fn django_settings(db: &dyn ProjectDb, project: Project) -> DjangoSettings {
    let Some(file) = settings_module_file(db, project) else {
        return DjangoSettings::default();
    };
    let source = file.source(db);
    let path = file.path(db);
    let mut resolver = SalsaSettingsResolver { db, project };
    extract_settings(source.as_str(), path, &mut resolver)
}
```

The untracked footprint function (settings.rs:63-73) and its walker
(settings.rs:455-494). The walker's load-bearing properties: it reads via
`self.db.read_file(path)` — a **direct, untracked, overlay-aware** filesystem
read (`crates/djls-source/src/db.rs:17-19`), so it sees current disk truth,
not the Salsa memo — and its defect: it re-parses with
`ruff_python_parser::parse_module` and scans **only top-level statements**:

```rust
// settings.rs:474-479
for stmt in parsed.into_syntax().body {
    let Stmt::ImportFrom(import) = stmt else {
        continue;
    };
    if !import.names.iter().any(|alias| alias.name.as_str() == "*") {
        continue;
    }
```

By contrast the extractor (`crates/djls-project/src/extraction/extractor.rs`)
recurses through `if`/`for`/`while`/`with`/`try` bodies (walk_stmt,
extractor.rs:192-246; `Try` at :218-226) and handles star imports wherever
they occur (walk_import_from, extractor.rs:322-354), skipping statically-false
branches (walk_if, extractor.rs:356-365). The resolver seam it calls is
`SettingsSourceResolver` (`crates/djls-project/src/extraction/settings.rs:226-233`):

```rust
pub trait SettingsSourceResolver {
    fn resolve_star_import(
        &mut self,
        import: &SettingsStarImport,
        importer: &Utf8Path,
    ) -> Option<SettingsSource>;
}
```

The Salsa-backed impl (settings.rs:559-572) resolves the module to a `File`
via `module_file` (settings.rs:496-509: touches search-root revisions, then
probes paths with `module_file_in_search_path`) and serves
`file.source(self.db)` — the tracked read. The shared relative-import
resolution is `resolve_star_import_module` + `ModuleFileParts`
(settings.rs:527-556, 574-612) — both drivers must keep using it.

The consumer (`crates/djls-semantic/src/project/sync.rs:42-44`):

```rust
for file in settings_source_files(db, project) {
    db.bump_file_revision(file);
}
```

The exemplar invalidation test to model after —
`refresh_external_data_reads_changed_star_imported_settings_source_for_template_libraries`
(`crates/djls-db/src/db.rs:810-877`): builds a tempdir + `InMemoryFileSystem`
with `manage.py`, `settings.py` containing `from .base import *`, and
`base.py` declaring an OPTIONS library; bootstraps a project; asserts the
library module; rewrites `base.py` in the mock fs; calls
`djls_semantic::refresh_external_data(&mut db)`; asserts the new module. It
passes today **only because the star import is top-level**.

Repo conventions: imports one-per-line grouped std/external/crate; comments
explain why only; `camino::Utf8Path` for paths.

## Commands you will need

| Purpose      | Command                          | Expected on success |
|--------------|----------------------------------|---------------------|
| Build        | `cargo build -q`                 | exit 0              |
| Test (all)   | `cargo test -q`                  | exit 0, all pass    |
| Test (crate) | `cargo test -q -p djls-db`       | exit 0              |
| One test     | `cargo test -q -p djls-db <name>`| exit 0              |
| E2E suite    | `just e2e`                       | exit 0              |
| Lint         | `just clippy`                    | exit 0, no warnings |
| Format       | `just fmt`                       | exit 0 (nightly — never `cargo fmt` directly) |

## Scope

**In scope** (the only files you should modify):
- `crates/djls-semantic/src/project/settings.rs`
- `crates/djls-db/src/db.rs` (new regression tests)
- `CHANGELOG.md` (bug-fix note per repo changelog conventions)

**Out of scope** (do NOT touch, even though they look related):
- `crates/djls-project/**` — **zero public-API change**. The resolver trait is
  already the seam; the recording adapter lives semantic-side.
- `crates/djls-semantic/src/project/sync.rs` — `settings_source_files` keeps
  its name, signature (`pub(super) fn (db, project) -> Vec<File>`), and
  untracked placement; sync.rs needs no edit.
- `django_settings` / `settings_module_file` — the tracked queries are
  correct as-is.
- `Project::touch_search_path_roots` / `module_file_in_search_path` — the
  untracked-probe/root-revision pattern stays exactly as it is.

## Version-control workflow

jj repo — never run mutating `git` commands. Work in your isolated backend.
Suggested commits: `"test: cover nested star-import refresh invalidation"`
(failing test allowed only in the working change, not committed standalone —
fold test + fix into one commit:
`"fix: walk the full settings graph for refresh invalidation"`). Do NOT push
or move bookmarks.

## Steps

### Step 1: Write the failing regression test (prove the bug)

In `crates/djls-db/src/db.rs`, clone the exemplar test (db.rs:810-877) as
`refresh_external_data_reads_changed_try_star_imported_settings_source` with
one change to the fixture: `settings.py` becomes

```python
try:
    from .base import *
except ImportError:
    pass
```

(everything else identical — `base.py` swaps `old_tags` → `new_tags`, assert
library module before/after refresh).

**Verify**: `cargo test -q -p djls-db refresh_external_data_reads_changed_try_star_imported` →
**FAILS** (second assertion sees `old_tags`). If it passes, the bug is already
fixed — STOP and report.

### Step 2: Replace the walker with the extractor over disk content

In `crates/djls-semantic/src/project/settings.rs`:

1. Add a disk-backed recording resolver next to `SalsaSettingsResolver`:

   ```rust
   struct DiskSettingsResolver<'db> {
       db: &'db dyn ProjectDb,
       project: Project,
       touched: Vec<File>,
   }
   ```

   Its `SettingsSourceResolver::resolve_star_import` mirrors the Salsa impl's
   resolution (`resolve_star_import_module` → `module_file` — share these:
   factor the two private helpers so both resolver types call the same
   functions; they are methods on `SalsaSettingsResolver` today, so move them
   to free functions taking `(db, project)` or a small shared struct) but
   serves source via `self.db.read_file(path)` instead of `file.source(db)`,
   records `self.db.get_or_create_file(path)` into `touched` (dedup not
   required here — see step 3), and returns `None` on read failure.

2. Reimplement `settings_source_files` as extract-and-discard:

   ```rust
   pub(super) fn settings_source_files(db: &dyn ProjectDb, project: Project) -> Vec<File> {
       let Some(file) = settings_module_file(db, project) else {
           return Vec::new();
       };
       let path = file.path(db);
       let Ok(source) = db.read_file(path) else {
           return vec![file];
       };
       let mut resolver = DiskSettingsResolver { db, project, touched: Vec::new() };
       let _ = extract_settings(&source, path, &mut resolver);
       // root file first, then the recorded closure, deduped
       ...
   }
   ```

   Note the root file's source is also read from **disk** (`db.read_file`),
   not `file.source(db)` — the whole point is seeing edges Salsa hasn't.

3. Dedup: collect into the result preserving first-seen order with a
   `BTreeSet<Utf8PathBuf>` guard (the extractor may ask the resolver for the
   same module more than once; its internal cycle guard returns early but the
   resolve call happens per import statement). Include the root settings file
   exactly once, first.

4. Delete `collect_settings_source_files` (settings.rs:455-494) and the
   now-unused direct `ruff_python_parser` import and `Stmt`/import-AST uses it
   pulled in (check what else in the file still needs them —
   `TemplateLibraryAnalysis::stmt_defines_library` uses `Stmt`/`Expr` types;
   remove only what is genuinely unused).

**Verify**: `cargo test -q -p djls-db refresh_external_data_reads_changed_try_star_imported` →
passes. `cargo test -q -p djls-semantic -p djls-db` → all pass (the existing
top-level-import test db.rs:810-877 must still pass).

### Step 3: Add the remaining regression cases

Two more tests in `crates/djls-db/src/db.rs`, same skeleton:

1. `refresh_external_data_reads_changed_conditionally_star_imported_settings_source`:
   `settings.py` =

   ```python
   import os
   if os.environ.get("EXTRA"):
       from .base import *
   else:
       from .base import *
   ```

   (an *ambiguous* condition — both arms import; asserts the walk enters
   ambiguous branches). Before/after refresh assertion as in Step 1. Note:
   knowledge will be `Partial` here, so assert on the extracted library module
   the same way (the library is still derived; if the
   `template_libraries`-based assertion proves awkward under Partial, assert
   on `django_settings` content instead — adjust to what the existing test
   harness exposes, and record which you chose).

2. `refresh_external_data_discovers_newly_star_imported_known_file` (the
   new-edge case): fixture has `settings.py` = `INSTALLED_APPS = []\n` plus an
   `extra.py` declaring an OPTIONS library, AND something that causes
   `extra.py` to be read into Salsa up front (simplest: assert
   `db.template_libraries()` once, then read `extra.py` via
   `get_or_create_file` + `source` to materialize the memo). Then rewrite
   `settings.py` on the mock fs to `from .extra import *`, **also rewrite**
   `extra.py` content, refresh, and assert the post-refresh state reflects
   the *new* `extra.py` content — proving the bump set was computed from
   current disk, not the stale closure.

**Verify**: `cargo test -q -p djls-db` → all pass, including the three new
tests.

### Step 4: Changelog and full validation

`CHANGELOG.md`: fixed — settings changes inside `try`/`if` blocks (e.g.
`local_settings` overrides) are now picked up on refresh.

**Verify**: `cargo test -q`, `just e2e`, `just clippy`, `just fmt --check`,
`just lint` → all exit 0.

## Test plan

- Step 1: the try/except case (the bug — written first, must fail before the
  fix).
- Step 3: ambiguous-branch star import; new-edge-to-stale-file case.
- Existing: `refresh_external_data_reads_changed_star_imported_settings_source_for_template_libraries`
  (db.rs:810) and the settings.rs unit tests
  (`django_settings_resolves_relative_star_imports`,
  `django_settings_recovers_from_star_import_cycle`) must pass unchanged.
- Model all new tests structurally on db.rs:810-877.

## Done criteria

Machine-checkable. ALL must hold:

- [ ] `rg -n "collect_settings_source_files" crates/` returns no matches
- [ ] `rg -n "ruff_python_parser" crates/djls-semantic/src/project/settings.rs` returns no matches
- [ ] `cargo test -q` exits 0, including the 3 new djls-db tests
- [ ] `just e2e` exits 0
- [ ] `just clippy` and `just fmt --check` exit 0
- [ ] No changes under `crates/djls-project/` (`jj diff --stat`)
- [ ] Only in-scope files modified (`jj diff --stat`)
- [ ] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- Step 1's test passes before any fix (bug already fixed elsewhere — the plan
  is stale).
- Sharing the resolution helpers between the two resolvers turns out to
  require changing `djls-project`'s API or the `SettingsSourceResolver` trait
  — the seam assumption is wrong; report what's missing.
- Step 3's new-edge test cannot be made to fail against the *old*
  implementation when you spot-check it (i.e., you cannot demonstrate it
  guards anything) — report rather than keeping a vacuous test.
- The existing top-level-import test (db.rs:810) regresses.
- Any e2e diff appears — refresh behavior changes must be invisible to the
  fixture project.

## Maintenance notes

- The invariant this establishes, worth a short comment at
  `settings_source_files` and in sync.rs's module doc: *the refresh bump set
  must be a superset of the extraction read-set evaluated against current
  disk; this holds by construction because both are produced by the same
  walk.* Any future divergence (a second walk implementation) reintroduces
  the bug class.
- Plan 015 moves `settings.rs` + `sync.rs` into `djls-project` afterwards;
  one walker, two resolvers move as a unit.
- Reviewers: check that the disk resolver reads **everything** via
  `db.read_file` (one accidental `file.source(db)` quietly reintroduces the
  stale-memo problem) and that the root settings file is in the returned set.
- Deferred (consciously): hoisting the per-call `touch_search_path_roots` out
  of `module_file` into the tracked-query entries — pure noise reduction,
  Salsa dedups the edges; see the memo's "Interaction with the existing
  seams".
- Design rationale and rejected alternatives (footprint-from-extraction,
  graph-as-salsa-query): `plans/memo-settings-source-graph.md`.
