# Plan 003: Stabilize the Project handle on the databases

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat 922cc4d7..HEAD -- crates/djls-db/src crates/djls-semantic/src/project/db.rs crates/djls-semantic/src/testing.rs`
> If any in-scope file changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding; on a
> mismatch, treat it as a STOP condition.

## Status

- **Priority**: P1
- **Effort**: S
- **Risk**: LOW
- **Depends on**: none
- **Category**: tech-debt
- **Planned at**: commit `922cc4d7`, 2026-06-10
- **Status**: DONE
- **Implemented at**: commit `f49ad9d4`, 2026-06-10
- **Bookmark**: `plan-003-stabilize-project-handle`

## Why this matters

`DjangoDatabase` stores its `Project` Salsa input handle behind
`Arc<Mutex<Option<Project>>>`. Tracked queries read it through the untracked
`ProjectDb::project()` accessor, so a `None → Some` transition or a handle
swap would change query results invisibly to Salsa — a stale-result bug class.
In practice the handle is already set once at construction and only mutated
field-by-field via setters (`djls-db/src/settings.rs`), so the Mutex buys
nothing and hides the invariant. ty documents and enforces exactly this
pattern with a plain field — see the comment in
`reference/ruff/crates/ty_project/src/db.rs:36-46`: "This handle must remain
stable for the lifetime of the database… Structural reloads must update the
existing `Project` in place via salsa setters instead of swapping in a freshly
constructed handle." This plan adopts that shape and writes the invariant
down. It is a precondition for plans 004/007/008, which hang more tracked
queries off the handle.

## Current state

- `crates/djls-db/src/db.rs:42` — the field:

  ```rust
  /// The single project for this database instance
  pub(crate) project: Arc<Mutex<Option<Project>>>,
  ```

- `crates/djls-db/src/db.rs:107-117` — set once during construction:

  ```rust
  if let Some(path) = project_path {
      db.set_project(path, settings);
  }
  ...
  fn set_project(&mut self, root: &Utf8Path, settings: &Settings) {
      let project = Project::bootstrap(self, root, settings);
      *self.project.lock().unwrap() = Some(project);
  }
  ```

- `crates/djls-db/src/db.rs:181-190` — the untracked accessor:

  ```rust
  impl ProjectDb for DjangoDatabase {
      fn project(&self) -> Option<Project> {
          *self.project.lock().unwrap()
      }
  ```

- `crates/djls-db/src/settings.rs:63-114` — `update_project_from_settings`
  already complies with the invariant: it mutates fields via
  `project.set_interpreter(self).to(...)` etc. and never constructs a new
  `Project`.

- Construction sites that pass a project path: `crates/djls-server/src/session.rs:74`,
  `crates/djls/src/commands/check.rs:157` and `:255`. Sites passing `None`:
  `crates/djls/src/commands/common.rs:130,150,170` (these dbs never gain a
  project — `project()` stays `None` for their lifetime, which is fine).

- `crates/djls-semantic/src/testing.rs:230-238` — `TestDatabase` mirrors the
  same `Arc<Mutex<Option<Project>>>` shape, and its
  `project_introspector()` constructs a **fresh** `ProjectIntrospector` on
  every call:

  ```rust
  fn project_introspector(&self) -> Arc<ProjectIntrospector> {
      Arc::new(ProjectIntrospector::new())
  }
  ```

- Tests assign the project after construction through the shared handle, e.g.
  `crates/djls-db/src/db.rs:269-270`:

  ```rust
  let project = Project::bootstrap(&db, "/test/project".into(), &settings);
  *db.project.lock().unwrap() = Some(project);
  ```

- Note `Project` (a `#[salsa::input]` handle) is `Copy` — storing it as a
  plain `Option<Project>` keeps `DjangoDatabase: Clone` derivable.

## Commands you will need

| Purpose      | Command                          | Expected on success |
|--------------|----------------------------------|---------------------|
| Build        | `cargo build -q`                 | exit 0              |
| Test (db)    | `cargo test -q -p djls-db`       | exit 0, all pass    |
| Test (all)   | `cargo test -q`                  | exit 0, all pass    |
| Lint         | `just clippy`                    | exit 0, no warnings |
| Format       | `just fmt`                       | exit 0              |
| Hooks        | `just lint`                      | exit 0              |

## Scope

**In scope** (the only files you should modify):
- `crates/djls-db/src/db.rs`
- `crates/djls-db/src/settings.rs` (only if the compiler forces it — it uses
  the accessor and should not need changes)
- `crates/djls-semantic/src/testing.rs`
- Any other file `rg "project\.lock\(\)" crates/` reveals (update mechanically)

**Out of scope** (do NOT touch, even though they look related):
- `crates/djls-semantic/src/project/db.rs` — the `ProjectDb` trait signature
  (`fn project(&self) -> Option<Project>`) stays as-is.
- `crates/djls-bench/` — check whether `BenchDatabase` has the same field; if
  it implements `ProjectDb` with the Mutex shape, apply the same mechanical
  change; if it has no project field, leave it alone.
- The `settings: Arc<Mutex<Settings>>` field — different concern, not this plan.
- Splitting the `Project` input itself — later plans.

## Git workflow

jj repo — no mutating `git`. Finish with:
`jj commit -m "refactor: store the project handle as a plain stable field"`.
Do NOT push.

## Steps

### Step 1: Verify the set-once premise

Run: `rg -n "set_project|Project::bootstrap|project\.lock" crates/ --no-heading`

**Verify**: `Project::bootstrap` is called only from
`DjangoDatabase::set_project` (db.rs:115), `TestDatabase` setup helpers, and
test functions that assign immediately after construction. No call site
replaces an existing project on a database that already has one, and no call
site sets the project after the db has been cloned. If either exists, STOP.

### Step 2: Change the field on `DjangoDatabase`

In `crates/djls-db/src/db.rs`:
- `project: Arc<Mutex<Option<Project>>>` → `project: Option<Project>`, with a
  doc comment adapted from ty (cite it):

  ```rust
  /// The single project for this database instance.
  ///
  /// This handle must remain stable for the lifetime of the database:
  /// tracked queries branch on the untracked `db.project()` read, so
  /// replacing the handle (or flipping None→Some after queries have run)
  /// changes results outside Salsa's dependency graph. Set once during
  /// construction; reloads mutate fields via Salsa setters
  /// (see `update_project_from_settings`).
  /// Same invariant as ty's ProjectDatabase (ty_project/src/db.rs).
  project: Option<Project>,
  ```

- `set_project` body → `self.project = Some(project);`
- `ProjectDb::project()` → `self.project`
- `Default` impl and both test constructors: `project: None,`
- Test assignments `*db.project.lock().unwrap() = Some(project);` →
  `db.project = Some(project);` (same crate, field is `pub(crate)`).
- Test reads `db.project.lock().unwrap().unwrap()` →
  `db.project.unwrap()` (or `db.project().unwrap()`).

**Verify**: `cargo test -q -p djls-db` → all pass.

### Step 3: Apply the same change to `TestDatabase`

In `crates/djls-semantic/src/testing.rs`: same field change and constructor
updates. Also fix the introspector churn: add a
`project_introspector: Arc<ProjectIntrospector>` field initialized once in
the builder/constructor, and return a clone of it from the trait method
instead of `Arc::new(ProjectIntrospector::new())` per call.

**Verify**: `cargo test -q -p djls-semantic` → all pass.

### Step 4: Sweep remaining lock sites and bench

`rg -n "project\.lock" crates/` → fix every remaining site mechanically.
Check `crates/djls-bench/src/` for the same pattern and apply if present.

**Verify**: `rg "Mutex<Option<Project>>" crates/` → no matches;
`cargo test -q` → all pass; `just clippy` → exit 0.

## Test plan

No new tests required; the existing `djls-db` invalidation tests
(`tag_specs_cached_on_repeated_access`,
`update_project_from_settings_unchanged_no_invalidation`, etc., db.rs:275-421)
are the behavioral contract and must pass unchanged. Optionally add one test
asserting `DjangoDatabase::new(..., Some(path)).project().is_some()` to pin
the set-at-construction behavior.

## Done criteria

Machine-checkable. ALL must hold:

- [ ] `rg "Mutex<Option<Project>>" crates/` returns no matches
- [ ] `rg -c "Arc::new\(ProjectIntrospector::new\(\)\)" crates/djls-semantic/src/testing.rs` returns 1 or 0 (the single constructor site), not one-per-call
- [ ] `cargo test -q` exits 0
- [ ] `just clippy` exits 0
- [ ] Only in-scope files modified (`jj diff --stat`)
- [ ] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- Step 1 finds a site that sets or replaces the project after the database
  could have been cloned (the Arc previously made clones share the late
  write; a plain field will not — that semantic change must be reviewed by a
  human, not papered over).
- A test relied on sharing the project slot across `db.clone()` boundaries.
- `DjangoDatabase` stops being `Clone` for any reason.

## Maintenance notes

- Reviewers should check that no future code path calls `Project::bootstrap`
  on a database that already has a project. The right operation is always
  field setters (the `settings.rs` pattern).
- When plan 007 turns more facts into tracked queries hanging off `project`,
  this invariant is what makes the untracked `db.project()` read safe — the
  comment added in Step 2 is the enforcement mechanism until someone writes a
  debug assertion.
- Future work may remove the `Option` entirely (CLI paths construct dbs
  without projects today — `commands/common.rs:130-170`); that is a separate
  decision, out of scope here.
