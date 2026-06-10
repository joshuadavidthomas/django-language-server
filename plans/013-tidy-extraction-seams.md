# Plan 013: Tidy the extraction seams (pre-006 structure-only pass)

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat 922cc4d7..HEAD -- crates/djls-semantic/src/lib.rs crates/djls-semantic/src/python.rs crates/djls-semantic/src/project/resolve.rs`
> If these files changed since the plan was written, content-match the
> excerpts below before proceeding; mismatch = STOP.
> NOTE on ordering: despite the number, this plan runs EARLY — before plans
> 006–008 (see the README execution order). It has no dependencies.

## Status

- **Priority**: P1
- **Effort**: S
- **Risk**: LOW
- **Depends on**: none (run before 006/007/008)
- **Category**: tech-debt (Tidy First pass for the extraction plans)
- **Planned at**: commit `922cc4d7`, 2026-06-10
- **Status**: DONE
- **Implemented at**: source commit `0b06bd31`, 2026-06-10
- **Merged at**: commit `737f9091`, 2026-06-10
- **PR**: https://github.com/joshuadavidthomas/django-language-server/pull/656

## Why this matters

Four small, structure-only tidyings, each directly lowering the cost of a
specific feature plan (Kent Beck's rule: tidy only what the imminent change
touches). Together they: remove plan 008's stated STOP risk (the registry
analysis is currently *uncallable* from `project/` code), give plan 007 a
reusable module-probe instead of a copy-paste, and let plan 007 reshape
`TemplateDirs`/`ProjectTemplateFiles` with provably zero external dependents.

## Current state

1. **Dead lib.rs exports.** `crates/djls-semantic/src/lib.rs` has 62
   `pub use` lines; six have **zero consumers in any other crate**
   (verified against djls-db, djls-server, djls-ide, djls-bench, djls,
   djls-corpus): `ProjectTemplateFiles` (`lib.rs:32`), `TemplateDirs`
   (`lib.rs:35`), `BlockSpecs` (`lib.rs:46`), `FilterArityMap` (`lib.rs:49`),
   `ModelDef` (`lib.rs:50`), `TagRuleMap` (`lib.rs:56`). All internal users
   import via `crate::project`/`crate::python` paths.

2. **Registry analysis is private.** `crates/djls-semantic/src/python/registry.rs`
   is pure and db-free — entry point
   `collect_registrations_from_body(body: &[Stmt]) -> Vec<RegistrationInfo>`
   (`registry.rs:131-135`), types `RegistrationInfo { name, kind, func_name }`
   (`registry.rs:28-32`) and `RegistrationKind` (`registry.rs:78-88`) — but
   `mod registry;` is private inside `python.rs` (`python.rs:6`) and nothing
   re-exports those items. Plan 008 (which builds template libraries in
   `project/`) cannot call it.

3. **A generic probe with a domain-specific name.**
   `templatetag_module_file(fs, module_path: &str, search_path) -> Option<Utf8PathBuf>`
   (`project/resolve.rs:208-225`) is a fully generic "dotted module path →
   `<root>/<a/b/c>.py` or `.../__init__.py`" probe with exactly one caller
   (`resolve.rs:251`). Plan 007's `settings_module_file` needs exactly this
   function; the name hides that.

4. **Empty stray directory.** `crates/djls-semantic/src/project/python/`
   exists, is empty, and is untracked noise (the real module is
   `project/python.rs`).

## Commands you will need

| Purpose      | Command                          | Expected on success |
|--------------|----------------------------------|---------------------|
| Build        | `cargo build -q`                 | exit 0              |
| Test (all)   | `cargo test -q`                  | exit 0, all pass    |
| Lint         | `just clippy`                    | exit 0, no warnings |
| Format       | `just fmt`                       | exit 0              |
| Hooks        | `just lint`                      | exit 0              |

## Scope

**In scope** (the only files you should modify):
- `crates/djls-semantic/src/lib.rs`
- `crates/djls-semantic/src/python.rs`
- `crates/djls-semantic/src/project/resolve.rs`
- `crates/djls-semantic/src/project/python/` (remove empty dir)

**Out of scope** (do NOT touch, even though they look related):
- Splitting `resolve.rs` (investigated; 918 of its 1,276 lines are tests,
  coupling is shallow — rejected).
- Unifying `ModulePath`/`PyModuleName` (different invariants; most affected
  lines die in plan 001 — rejected; if a second bridge appears in plan 008,
  add a single `From<&PyModuleName> for ModulePath` impl then).
- Wiring-only exports (`refresh_external_data`, `ProjectIntrospector`,
  `Interpreter`, `load_env_file`) — they die with their consumers in
  plans 005/008/009.
- Any behavior change whatsoever.

## Git workflow

jj repo — no mutating `git`. Finish with:
`jj commit -m "refactor: tidy extraction seams ahead of static discovery work"`.
Do NOT push.

## Steps

### Step 1: Delete the six dead exports

Remove the `pub use` lines for `ProjectTemplateFiles`, `TemplateDirs`,
`BlockSpecs`, `FilterArityMap`, `ModelDef`, `TagRuleMap` from
`crates/djls-semantic/src/lib.rs`.

**Verify**: `cargo build -q` → exit 0 (if anything fails to compile, that
export had a consumer — STOP and report which).

### Step 2: Expose the registry seam

In `crates/djls-semantic/src/python.rs`, add a crate-internal re-export next
to the existing re-exports (`python.rs:33-54`):

```rust
pub(crate) use crate::python::registry::RegistrationInfo;
pub(crate) use crate::python::registry::RegistrationKind;
pub(crate) use crate::python::registry::collect_registrations_from_body;
```

(Adjust `registry.rs` item visibility to `pub(crate)` where needed.) Do not
add a wrapper function — plan 008 decides whether it wants a tracked
`module_registrations(db, file)` query; this step only makes that possible.

**Verify**: `cargo build -q` → exit 0; `just clippy` → exit 0 (unused
re-exports inside the crate may warn — if so, gate with
`#[allow(unused_imports)]`? NO: instead reference them from a one-line unit
test asserting `collect_registrations_from_body(&[])` returns empty, which
both exercises the seam and silences the warning honestly).

### Step 3: Rename the module probe

Rename `templatetag_module_file` → `module_file_in_search_path` in
`project/resolve.rs` (definition `:208`, call site `:251`, plus its tests).
Mechanical rename only — no signature change.

**Verify**: `rg -n "templatetag_module_file" crates/` → no matches;
`cargo test -q -p djls-semantic` → all pass.

### Step 4: Remove the stray directory

`rmdir crates/djls-semantic/src/project/python/` (it is empty; `rmdir`
fails if not — which would be a STOP).

**Verify**: `cargo test -q` → all pass; `just fmt`, `just lint` → exit 0.

## Test plan

One new unit test (Step 2's empty-body registry call). Everything else is
covered by the existing suite passing unchanged — this plan is
structure-only by definition.

## Done criteria

Machine-checkable. ALL must hold:

- [ ] The six named `pub use` lines are gone from `lib.rs`
- [ ] `rg "templatetag_module_file" crates/` returns no matches
- [ ] `collect_registrations_from_body` is reachable from `crate::python` (the new test compiles and passes)
- [ ] `crates/djls-semantic/src/project/python/` does not exist
- [ ] `cargo test -q` exits 0; `just clippy` exits 0
- [ ] Only in-scope files modified (`jj diff --stat`)
- [ ] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- Step 1 breaks a build — a "dead" export had a consumer this
  investigation missed.
- The rename in Step 3 collides with an existing `module_file_in_search_path`
  symbol or requires touching files outside resolve.rs.
- `rmdir` fails (directory not empty — investigate what appeared there).

## Maintenance notes

- Plans 007/008 reference the renamed probe and the registry seam; if their
  texts still say `templatetag_module_file`, the rename here is the source
  of truth (the plans' drift checks will flag it — that's expected, proceed).
- Reviewers: this commit must contain zero behavior change; any test-output
  difference is a defect.
