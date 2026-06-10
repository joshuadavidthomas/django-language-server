# Plan 001: Delete the dead "static model" milestone scaffolding

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat 922cc4d7..HEAD -- crates/djls-semantic/src/project.rs crates/djls-semantic/src/project/static_model.rs crates/djls-semantic/src/project/static_resolver.rs crates/djls-semantic/src/project/static_django_environments.rs`
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
- **PR**: https://github.com/joshuadavidthomas/django-language-server/pull/653
- **Merged at**: commit `6bc3b07d`, 2026-06-10

## Why this matters

`crates/djls-semantic/src/project/` carries 1,444 lines of `#![allow(dead_code)]`
modules left over from an abandoned "static project model" milestone plan
(`static_model.rs` 485 lines, `static_resolver.rs` 729, `static_django_environments.rs` 230).
They duplicate live types under parallel names (`ImportRoot` vs the live
`SearchPath` in `resolve.rs`, `ResolvedModule` vs `PythonModule`,
`resolve_module` vs the live resolver) and define a 4-variant `Fact<T>`
confidence lattice that nothing consumes. Upcoming static-extraction work
(plans 006–008) introduces new types with overlapping names; this parallel
vocabulary must be gone first so the new code has exactly one name for each
concept.

## Current state

- `crates/djls-semantic/src/project.rs:7-9` — the only references to these modules:

  ```rust
  mod static_django_environments;
  mod static_model;
  mod static_resolver;
  ```

  They are private `mod` declarations; nothing in `project.rs` re-exports any
  of their items (verified: the `pub use` block at `project.rs:14-39` contains
  no `static_*` items).

- `crates/djls-semantic/src/project/static_model.rs:7-10`:

  ```rust
  #![allow(
      dead_code,
      reason = "Milestone A1 defines fact types before later milestones populate them."
  )]
  ```

  Defines `Confidence`, `Fact<T>`, `Reason`, `ImportRoot`, `ResolvedModule`, etc.

- `crates/djls-semantic/src/project/static_resolver.rs:8-11` — same
  `#![allow(dead_code)]` marker ("Milestone A2"); imports only from
  `static_model` and `names`.

- `crates/djls-semantic/src/project/static_django_environments.rs:8-11` — same
  marker ("Milestone A3"). Note: this module is the **only** consumer of
  `djls_conf::Settings::django_environments()` outside `djls-conf` itself.
  After deletion, the `[[django_environments]]` config schema in
  `crates/djls-conf` is parsed but unused — that is expected and stays
  (see Maintenance notes).

## Commands you will need

| Purpose      | Command                          | Expected on success |
|--------------|----------------------------------|---------------------|
| Build        | `cargo build -q`                 | exit 0              |
| Test (crate) | `cargo test -q -p djls-semantic` | exit 0, all pass    |
| Test (all)   | `cargo test -q`                  | exit 0, all pass    |
| Lint         | `just clippy`                    | exit 0, no warnings |
| Format       | `just fmt`                       | exit 0              |
| Hooks        | `just lint`                      | exit 0              |

Do not run `cargo fmt --check` directly — this repo's `.rustfmt.toml` needs
nightly; always go through `just fmt`.

## Scope

**In scope** (the only files you should modify):
- `crates/djls-semantic/src/project/static_model.rs` (delete)
- `crates/djls-semantic/src/project/static_resolver.rs` (delete)
- `crates/djls-semantic/src/project/static_django_environments.rs` (delete)
- `crates/djls-semantic/src/project.rs` (remove the three `mod` lines)

**Out of scope** (do NOT touch, even though they look related):
- `crates/djls-conf/` — the `django_environments` schema stays even though its
  last consumer is being deleted; multi-environment support is a documented
  future direction.
- `crates/djls-semantic/src/project/symbols.rs` — the `Discovered` library
  machinery there is removed by plan 002, not this plan.
- `crates/djls-semantic/src/project/resolve.rs` — the live resolver.

## Git workflow

This repo uses jj (colocated `.jj` + `.git`). Never run mutating `git` commands.

- Make the changes; jj snapshots the working copy automatically.
- Finish with: `jj commit -m "refactor: delete dead static-model milestone scaffolding"`
- Do NOT push, create bookmarks, or open a PR.

## Steps

### Step 1: Confirm the modules are referenced nowhere else

Run: `rg -l "static_model|static_resolver|static_django_environments" crates/`

**Verify**: matches only in the three `static_*.rs` files themselves and
`crates/djls-semantic/src/project.rs`. Any other match is a STOP condition.

### Step 2: Delete the three files and their module declarations

Delete the three files. In `crates/djls-semantic/src/project.rs`, remove
lines `mod static_django_environments;`, `mod static_model;`,
`mod static_resolver;`.

**Verify**: `cargo build -q` → exit 0.

### Step 3: Sweep for newly dead code

The deleted modules consumed a few helpers from live modules (e.g.
`crate::project::input::resolve_django_settings`). Those helpers have live
callers too (`input.rs::bootstrap`), but clippy will tell you if anything is
now unused.

**Verify**: `just clippy` → exit 0, no warnings. If clippy reports newly dead
items in `input.rs`/`names.rs`, delete those items too only if their sole
caller was the deleted scaffolding; otherwise STOP.

### Step 4: Full validation

**Verify**: `cargo test -q` → all pass; `just fmt` → exit 0; `just lint` → exit 0.

## Test plan

No new tests — this is a pure deletion. The existing suite passing is the
proof: tests inside the deleted files die with the files (they tested dead
code only).

## Done criteria

Machine-checkable. ALL must hold:

- [ ] The three `static_*.rs` files no longer exist
- [ ] `rg "static_model|static_resolver|static_django_environments" crates/` returns no matches
- [ ] `rg "allow\(\s*dead_code" crates/djls-semantic/src/project/` returns no matches
- [ ] `cargo test -q` exits 0
- [ ] `just clippy` exits 0
- [ ] No files outside the in-scope list are modified (`jj diff --stat`)
- [ ] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- Step 1's `rg` finds references outside the three files + `project.rs` —
  something started consuming the scaffolding after this plan was written.
- Deleting a "newly dead" helper in Step 3 breaks a test — that helper had a
  live caller; restore it and report.

## Maintenance notes

- `djls-conf`'s `[[django_environments]]` schema (`crates/djls-conf/src/lib.rs:78`,
  `crates/djls-conf/src/django_environments.rs`) is now parsed-but-unused.
  When multi-environment support is designed (see CONTEXT.md "Django
  Environment"), it plugs back in there.
- Plans 006–008 introduce a new `djls-extraction` crate with its own
  `Knowledge` type; reviewers should confirm no resurrection of the deleted
  4-variant `Fact<T>` shape (Josh's rule: no enum variants whose match arms
  are identical everywhere).
