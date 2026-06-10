# Plan 005: Extract db-free Python environment discovery into a `djls-python` crate

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat 922cc4d7..HEAD -- crates/djls-semantic/src/project/python.rs crates/djls-semantic/src/project/system.rs crates/djls-semantic/src/project/resolve.rs crates/djls-semantic/src/project.rs Cargo.toml`
> If any in-scope file changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding; on a
> mismatch, treat it as a STOP condition.

## Status

- **Priority**: P2
- **Effort**: M
- **Risk**: LOW
- **Depends on**: none (001 recommended first)
- **Category**: tech-debt
- **Planned at**: commit `922cc4d7`, 2026-06-10

## Why this matters

Python environment discovery — finding the venv, the interpreter, and
site-packages — currently lives inside `djls-semantic`, the crate whose
stated job is "Django/project/template meaning". It is pure filesystem
probing with no Salsa or semantic dependency, and it is exactly the part of
the system that should grow next (pyvenv.cfg parsing, better layout probing).
ty isolates the same concern as `ty_site_packages`: a crate with **zero
database dependency** whose discovery chain is plain functions over a
filesystem trait (`reference/ruff/crates/ty_site_packages/src/lib.rs:284-300`
— `PythonEnvironment::discover(project_root, system)` checking
`$VIRTUAL_ENV` first, then falling back through heuristics). Moving our
equivalent out gives environment discovery a home where it can grow without
fattening djls-semantic, and makes it independently testable.

## Current state

- `crates/djls-semantic/src/project/python.rs` (444 lines) — the whole file
  is the move target. Key items:

  ```rust
  // python.rs:14-21
  pub enum Interpreter {
      Auto,
      VenvPath(String),
      InterpreterPath(String),
  }
  // python.rs:26-40  Interpreter::discover(venv_path: Option<&str>) — checks
  //                  explicit setting, then $VIRTUAL_ENV, else Auto
  // python.rs:42-117 site_packages_path / site_packages_path_in_venv —
  //                  probes venv layouts (Windows Lib\site-packages,
  //                  lib/pythonX.Y/site-packages with version-sorted pick)
  // python.rs:119+   auto_venv_paths — [".venv", "venv", "env", ".env"]
  ```

  It depends only on `camino`, `djls_source::{FileSystem, WalkEntryKind,
  WalkOptions}`, and `crate::project::system`.

- `crates/djls-semantic/src/project/system.rs` (205 lines) — env-var/`which`
  access with test mocking, used by `Interpreter::discover` in test builds
  (`python.rs:34-37`). Check its other consumers:
  `rg -n "system::" crates/djls-semantic/src` — if only `python.rs` uses it,
  it moves too; if other modules use it, it stays and `djls-python` gets its
  own copy of the env-var seam (report which case you found).

- Consumers of `Interpreter` outside `python.rs` (verify with
  `rg -n "Interpreter" crates/ --no-heading`):
  - `crates/djls-semantic/src/project/input.rs:171` (Project input field),
    `:217` (`Interpreter::discover` in bootstrap)
  - `crates/djls-semantic/src/project/resolve.rs:95`
    (`interpreter.site_packages_path(fs, root)` inside
    `SearchPaths::from_project_settings`)
  - `crates/djls-semantic/src/project/sync.rs:59,161` (cache key)
  - `crates/djls-db/src/settings.rs:2,70` (`djls_semantic::Interpreter`)
  - re-exports: `project.rs:23`, and `djls-semantic/src/lib.rs`

- Workspace manifest: `Cargo.toml` — internal crates are listed as path
  workspace dependencies; copy the dependency-table style of an existing
  small crate (`crates/djls-conf/Cargo.toml`) when writing the new manifest.
  Internal library crates use version `0.0.0` (see AGENTS.md "Crate
  Responsibilities" and any sibling crate manifest).

## Commands you will need

| Purpose      | Command                          | Expected on success |
|--------------|----------------------------------|---------------------|
| Build        | `cargo build -q`                 | exit 0              |
| Test (new)   | `cargo test -q -p djls-python`   | exit 0, all pass    |
| Test (all)   | `cargo test -q`                  | exit 0, all pass    |
| Lint         | `just clippy`                    | exit 0, no warnings |
| Format       | `just fmt`                       | exit 0              |
| Hooks        | `just lint`                      | exit 0              |

## Scope

**In scope** (the only files you should modify/create):
- `crates/djls-python/` (create: `Cargo.toml`, `src/lib.rs`, moved modules)
- `crates/djls-semantic/src/project/python.rs` (delete after move)
- `crates/djls-semantic/src/project/system.rs` (move or keep — Step 1 decides)
- `crates/djls-semantic/src/project.rs`, `crates/djls-semantic/src/lib.rs`
  (drop re-exports), `crates/djls-semantic/Cargo.toml`
- `crates/djls-semantic/src/project/{input,resolve,sync}.rs` (import paths)
- `crates/djls-db/src/settings.rs`, `crates/djls-db/Cargo.toml`
- Root `Cargo.toml` (workspace members + workspace deps if the repo lists
  internal crates there — mirror how `djls-conf` is wired)

**Out of scope** (do NOT touch, even though they look related):
- `SearchPath`/`SearchPaths` (`resolve.rs:17-147`) — they stay in
  djls-semantic because `register_roots` needs the db. Only the call into
  `interpreter.site_packages_path(...)` changes its import path.
- Adding pyvenv.cfg parsing or new discovery features — this plan is a pure
  move. New capability comes later.
- The Python *source analysis* code (`djls-semantic/src/python/`) — different
  concern, different crate (plan 006 territory).

## Git workflow

jj repo — no mutating `git`. When relocating code, move the file first, then
edit it in place (repo rule — do not retype from memory). Finish with:
`jj commit -m "refactor: extract python environment discovery into djls-python"`.
Do NOT push.

## Steps

### Step 1: Determine `system.rs` ownership

Run `rg -n "system::" crates/djls-semantic/src --no-heading`.

**Verify**: record the consumer list. If `python.rs` is the only consumer,
`system.rs` moves with it. Otherwise `system.rs` stays and the moved code
takes the env-var seam with it under `djls-python` (duplicating ~the env_var
helper only — not the whole module). Note the outcome in your final report.

### Step 2: Scaffold the crate

Create `crates/djls-python/Cargo.toml` modeled on
`crates/djls-conf/Cargo.toml` (same `[lints]`/edition/workspace inheritance
style, `version = "0.0.0"`). Dependencies: `camino`, `djls-source`,
`tracing` (workspace = true for each). Add the crate to the workspace
members list in the root `Cargo.toml` if members are explicit.

**Verify**: `cargo build -q -p djls-python` → exit 0 (empty lib).

### Step 3: Move the code

`git`-level move via filesystem: move `python.rs` to
`crates/djls-python/src/interpreter.rs` (and `system.rs` per Step 1 outcome).
`src/lib.rs` declares the modules and re-exports the boundary API:

```rust
mod interpreter;
mod system; // if moved

pub use crate::interpreter::Interpreter;
```

Adjust internal paths (`crate::project::system` → `crate::system`). Make
`site_packages_path` `pub` — it was `pub(crate)` and `SearchPaths` (staying
in djls-semantic) still calls it. Tests inside the moved files move with
them.

**Verify**: `cargo test -q -p djls-python` → all pass (the moved unit tests).

### Step 4: Re-point consumers

- `djls-semantic`: remove `mod python;` from `project.rs` and the
  `pub use crate::project::python::Interpreter;` re-export (`project.rs:23`);
  remove the lib.rs re-export of `Interpreter` (check
  `rg "Interpreter" crates/djls-semantic/src/lib.rs`). Add `djls-python`
  to `crates/djls-semantic/Cargo.toml`. Update imports in `input.rs`,
  `resolve.rs`, `sync.rs` to `djls_python::Interpreter`.
- `djls-db`: `use djls_semantic::Interpreter` →
  `use djls_python::Interpreter` (settings.rs:2) + manifest dep.
- Clean break: do NOT leave a `pub use djls_python::Interpreter` shim in
  djls-semantic "for compatibility" — update every importer instead
  (repo rule: no re-exporting through multiple layers).

**Verify**: `cargo build -q` → exit 0;
`rg "djls_semantic::Interpreter|project::python" crates/` → no matches.

### Step 5: Full validation

**Verify**: `cargo test -q` → all pass; `just clippy`, `just fmt`,
`just lint` → exit 0.

## Test plan

No new tests — the moved unit tests (venv layout probing, `$VIRTUAL_ENV`
handling via the system mock, auto-discovery order) keep covering the moved
code from the new crate. Confirm with
`cargo test -q -p djls-python` listing > 0 tests.

## Done criteria

Machine-checkable. ALL must hold:

- [ ] `crates/djls-python/` exists, builds, and its tests pass
- [ ] `crates/djls-semantic/src/project/python.rs` no longer exists
- [ ] `rg "djls_semantic::Interpreter" crates/` returns no matches
- [ ] `rg "pub use djls_python" crates/djls-semantic/` returns no matches (no shim)
- [ ] `cargo test -q` exits 0
- [ ] `just clippy` exits 0
- [ ] Only in-scope files modified (`jj diff --stat`)
- [ ] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- `system.rs` turns out to be consumed by modules with semantics beyond
  env-var/`which` lookup (Step 1) and splitting it isn't mechanical.
- `Interpreter` implements traits defined in djls-semantic (orphan-rule
  breakage on move).
- The root `Cargo.toml` workspace layout differs materially from what
  `djls-conf` suggests (e.g. centralized internal-crate version table you
  can't pattern-match) — ask rather than guess.

## Maintenance notes

- This crate is the future home of richer static environment discovery:
  pyvenv.cfg parsing (model: `PyvenvCfgParser` in
  `reference/ruff/crates/ty_site_packages/src/lib.rs` — pyvenv.cfg "looks
  like INI but isn't valid INI", hence ty's hand-rolled cursor parser),
  `home`-key resolution, uv's `extends-environment`, Debian
  `dist-packages` layouts. Each lands here as pure functions + fixtures,
  never as db queries.
- Origin-carrying error types (ty's `SysPrefixPathOrigin` pattern — *why* we
  probed a path: config, env var, heuristic) are the right next API change
  when discovery failures need user-facing diagnostics.
- Reviewers: the move must be literal — diff the moved file against the old
  one (`git diff --find-renames` view) and reject rewrites.
