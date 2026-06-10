# Plan 004: Derive template files with a tracked query instead of pushing them into the Project input

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat 922cc4d7..HEAD -- crates/djls-semantic/src/project/input.rs crates/djls-semantic/src/project/sync.rs crates/djls-semantic/src/resolution.rs crates/djls-semantic/src/project.rs`
> If any in-scope file changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding; on a
> mismatch, treat it as a STOP condition.

## Status

- **Priority**: P1
- **Effort**: M
- **Risk**: MED
- **Depends on**: 003 (stable project handle); 014 recommended first (collapses test constructor churn)
- **Category**: tech-debt
- **Planned at**: commit `922cc4d7`, 2026-06-10

## Why this matters

`Project.template_files` is **derived data stored as Salsa input state**: an
imperative refresh walks the template directories and writes the result into
the input (`sync.rs::refresh_template_files`). That push-shape is the pattern
that made previous PRs balloon — every new fact needed a field + a refresh
function + setter-compare + constructor churn in every test. This plan
converts the first fact to pull-shape (a tracked query), shrinking the god
input by one field and establishing the exact pattern plans 007/008 repeat
for template dirs and libraries. The reference model is ty, which derives its
file index lazily rather than pushing it (`reference/ruff/crates/ty_project/src/files.rs:14-23`:
"The indexing happens lazily, but the files are then cached for subsequent
reads"); our version is simpler — a plain tracked query whose freshness comes
from file-root revisions.

## Current state

- `crates/djls-semantic/src/project/input.rs:197-199` — the input field to
  remove:

  ```rust
  /// First-party template files discovered for this project.
  #[returns(ref)]
  pub(crate) template_files: ProjectTemplateFiles,
  ```

  plus builder arg at `input.rs:240` (`ProjectTemplateFiles::default()`) and
  `.template_files_durability(Durability::LOW)` at `:244`.

- `crates/djls-semantic/src/project/input.rs:42-112` — `ProjectTemplateFiles`
  / `ProjectTemplateFile` types (name, path, `File` handle), including
  `from_ordered_paths` which calls `db.get_or_create_file`.

- `crates/djls-semantic/src/project/sync.rs:257-299` — the push to replace:
  `refresh_template_files` walks `project.template_dirs(db).as_known()` dirs
  via `db.walk_entries(dir, &WalkOptions::unrestricted())`, builds
  `(name, path)` pairs from `entry.relative.clean()`, sorts per-dir by
  `(name, path)`, extends in dir order, and writes via
  `project.set_template_files(db).to(next)`. Called from
  `refresh_external_data` (`sync.rs:45`).

- `crates/djls-semantic/src/resolution.rs:149-172` — the only consumer:
  `template_origins` iterates `project.template_files(db).iter()`.

- Precedent for both techniques the new query needs, in
  `crates/djls-semantic/src/project/resolve.rs:150-163` (`model_modules`):
  a tracked query that (a) registers a dependency on every search-path root's
  revision (`let _ = root.revision(db);`) and (b) calls
  `db.get_or_create_file(&path)` inside the tracked query.

- Freshness today: template files only change (from Salsa's view) when
  `refresh_external_data` runs, because `refresh_python_modules`
  (`sync.rs:301-313`) bumps every search-path root revision on each refresh.
  The new query preserves exactly this cadence by reading those same root
  revisions.

## Commands you will need

| Purpose      | Command                          | Expected on success |
|--------------|----------------------------------|---------------------|
| Build        | `cargo build -q`                 | exit 0              |
| Test (crate) | `cargo test -q -p djls-semantic` | exit 0, all pass    |
| Test (all)   | `cargo test -q`                  | exit 0, all pass    |
| Lint         | `just clippy`                    | exit 0, no warnings |
| Format       | `just fmt`                       | exit 0              |
| Hooks        | `just lint`                      | exit 0              |

## Scope

**In scope** (the only files you should modify):
- `crates/djls-semantic/src/project/templates.rs` (create — new module)
- `crates/djls-semantic/src/project/input.rs` (remove field + move types out)
- `crates/djls-semantic/src/project/sync.rs` (remove `refresh_template_files`)
- `crates/djls-semantic/src/project.rs` (module decl + re-export updates)
- `crates/djls-semantic/src/resolution.rs` (consumer switch)
- Test files that construct `Project` (find via
  `rg -n "Project::builder|Project::new\(" crates/`; with plan 014 landed,
  that is `testing.rs`'s `ProjectFixture` only)

**Out of scope** (do NOT touch, even though they look related):
- `Project.template_dirs` and its refresh — converted in plan 007.
- `refresh_python_modules` and the root-revision bumping — it stays; it is
  what gives this query its freshness boundary.
- `djls-source` walk/file APIs.

## Git workflow

jj repo — no mutating `git`. Finish with:
`jj commit -m "refactor: derive project template files from a tracked query"`.
Do NOT push.

## Steps

### Step 1: Create the tracked query module

Create `crates/djls-semantic/src/project/templates.rs` (repo convention:
`folder.rs`, not `folder/mod.rs` — this is a flat module file). Move
`ProjectTemplateFiles` and `ProjectTemplateFile` here from `input.rs`
(cut/paste, keep impls intact). Add:

```rust
#[salsa::tracked(returns(ref))]
pub(crate) fn project_template_files(
    db: &dyn ProjectDb,
    project: Project,
) -> ProjectTemplateFiles {
    // Freshness boundary: template discovery re-runs when any search-path
    // root revision is bumped (refresh_external_data does this), matching
    // the previous imperative refresh cadence. Template dirs that live
    // outside every registered root are still re-walked then, because this
    // query invalidates as a whole.
    for search_path in project.search_paths(db).iter() {
        if let Some(root) = db.files().root(db, search_path.path()) {
            let _ = root.revision(db);
        }
    }
    /* body of the old refresh_template_files walk, unchanged:
       match project.template_dirs(db).as_known() { ... } building
       ProjectTemplateFiles::from_ordered_paths(db, templates) —
       but RETURN the value instead of calling set_template_files */
    ...
}
```

Copy the walk logic verbatim from `sync.rs:258-294` (the `match` on
`template_dirs`, the per-dir walk, sort, and extend). Match import style:
one import per line, grouped std/external/crate.

Register `mod templates;` in `project.rs` and re-export
`project_template_files` (`pub(crate)`) plus `ProjectTemplateFiles` at the
same visibility it has today (`pub use` at `project.rs:16`; check its actual
external consumers with `rg "ProjectTemplateFiles" crates/` and keep only the
visibility that's needed).

**Verify**: `cargo build -q` → exit 0 (old field still present, query unused —
warnings acceptable until Step 3).

### Step 2: Switch the consumer

In `resolution.rs:154`, replace
`for template in project.template_files(db).iter()` with
`for template in crate::project::project_template_files(db, project).iter()`.

**Verify**: `cargo test -q -p djls-semantic` → all pass (both paths produce
identical data this commit).

### Step 3: Remove the input field and the push

- `input.rs`: delete the `template_files` field, the builder argument, and
  `.template_files_durability(Durability::LOW)`.
- `sync.rs`: delete `refresh_template_files` and its call at `sync.rs:45`;
  remove now-unused imports (`ProjectTemplateFiles`, `WalkEntryKind`,
  `WalkOptions`, `Utf8PathClean` — whatever clippy flags).
- Update every `Project` constructor call site — find with
  `rg -n "Project::builder|Project::new\(" crates/`. Reality check from
  investigation: `Project::builder` has exactly ONE call site
  (`Project::bootstrap`, input.rs:230-246); the churn is the salsa-generated
  `Project::new(...)` in test helpers (`resolve.rs:384-404`,
  `resolution.rs:346-382`, `resolve.rs:992-1004`). If plan 014 has landed,
  those three collapsed into `ProjectFixture::build` in `testing.rs` — then
  this step touches exactly two places.

**Verify**: `cargo build -q` → exit 0; `rg "set_template_files|template_files\(db\)" crates/` → no matches outside `templates.rs`.

### Step 4: Full validation

**Verify**: `cargo test -q` → all pass; `just clippy`, `just fmt`,
`just lint` → exit 0. If any insta snapshot changes, STOP — this refactor
must be behavior-preserving.

## Test plan

- Add one incrementality test in `crates/djls-db/src/db.rs` (model after
  `template_libraries_change_validates_templatetag_module_projection`,
  db.rs:297-330): prime `project_template_files` via a template-resolution
  query, take the event log, bump a search-path root revision
  (`db.bump_file_root_revision(root)`), and assert `was_executed(...,
  "project_template_files")`.
- Add one behavior test in `djls-semantic` using `TestDatabase` +
  `InMemoryFileSystem`: two template dirs with a shadowed template name;
  assert resolution order matches dir order (this pins the sort/precedence
  semantics copied from the old refresh).
- Existing resolution tests must pass unchanged.

## Done criteria

Machine-checkable. ALL must hold:

- [ ] `rg "refresh_template_files|set_template_files" crates/` returns no matches
- [ ] `Project` input has 9 fields (`rg -c "returns\(ref\)" crates/djls-semantic/src/project/input.rs` decreased by one)
- [ ] `cargo test -q` exits 0, including the two new tests
- [ ] `just clippy` exits 0
- [ ] Only in-scope files modified (`jj diff --stat`)
- [ ] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- `db.files().root(db, ...)` or `db.walk_entries(...)` is not available on
  `&dyn ProjectDb` inside a tracked query (check `model_modules` compiles the
  same calls first — it does at `resolve.rs:156` today).
- Any insta snapshot changes.
- A test depends on `template_files` being settable directly (a
  `set_template_files` call site outside `sync.rs`) — that test needs a
  design decision, not a workaround.
- Creating `File` entities inside the tracked query produces a Salsa panic
  (it must not — `model_modules` does this today — but if it does, report
  rather than moving file creation outside).

## Maintenance notes

- Plan 007 changes `template_dirs` from an input to a tracked query; at that
  point `project_template_files` switches from reading
  `project.template_dirs(db)` to calling that query — a one-line change noted
  in plan 007's steps.
- If file watching lands later, the root-revision dependency in Step 1 is the
  hook: the watcher bumps the containing root's revision and this query
  re-runs without any imperative refresh. This mirrors ty's `.pth`-scanning
  pattern (`reference/ruff/crates/ty_module_resolver/src/resolve.rs`, the
  `dynamic_resolution_paths` query depends on a site-packages root revision).
- Reviewers: scrutinize the precedence semantics (per-dir sort then extend in
  dir order) — Django template resolution order is user-visible behavior.
