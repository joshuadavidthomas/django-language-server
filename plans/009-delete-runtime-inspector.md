# Plan 009: Delete the runtime Python inspector

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: This plan requires plans 007 and 008 to be
> DONE. Precondition check: `rg "project_introspector\(\)\.query" crates/`
> must return **no matches** (the inspector has no callers). If it returns
> matches, STOP — a feature still depends on the subprocess.

## Status

- **Priority**: P2
- **Effort**: M
- **Risk**: LOW-MED
- **Depends on**: plans/007, plans/008
- **Category**: tech-debt (the payoff step)
- **Planned at**: commit `922cc4d7`, 2026-06-10
- **Status**: IN PROGRESS — implemented locally, not pushed
- **Implemented at**: source commit `437117c5`, 2026-06-11
- **Bookmark**: `plan-009-delete-runtime-inspector`
- **Validation**: `cargo build -q`; `cargo test -q`; `just fmt`; `just lint`; `cargo test -q -j 2 -- --test-threads=2`; `just test`; `just e2e`; `just clippy --allow-dirty`; `just fmt --check`; clean-tree `just clippy`; clean-tree `just fmt --check`; stale inspector/introspector guards; debug binary `djls_inspector` guard.
- **Divergence**: Step 1 also found a test-only `OsTestDatabase` in `crates/djls-semantic/src/project/settings.rs` implementing `ProjectDb::project_introspector`; this was removed with the rest of the trait plumbing because it was not a feature caller.

## Why this matters

This is the goal line: after plans 007/008, every Django fact the server
uses is derived from source. The Python inspector — a zipapp embedded in the
binary, written to a temp file, executed against the project's interpreter
with `django.setup()`, managed by a subprocess-with-reaper-thread lifecycle —
no longer feeds anything. Deleting it removes a process boundary, a JSON
wire protocol, an embedded Python artifact and its build step, and the
single biggest reason the server ever needed a working Python environment.
"Static extraction never imports Django or runs Python" stops being an
invariant with an asterisk.

## Current state

(Verify each against the post-008 tree; planned-at line numbers will have
shifted.)

- `crates/djls-semantic/src/project/introspector.rs` (555 lines at
  `922cc4d7`) — `ProjectIntrospector`, `InspectorProcess`,
  `IntrospectionRequest` trait, the reaper thread (`thread::spawn` at
  `:135` and `:420`), and the embedded zipapp:

  ```rust
  // introspector.rs:323
  const INSPECTOR_PYZ: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/djls_inspector.pyz"));
  ```

- `crates/djls-semantic/inspector/` — the shipped Python package
  (`__main__.py`, `inspector.py`, `queries.py`; calls `django.setup()`).
- `crates/djls-semantic/build.rs` — builds the `.pyz` into `OUT_DIR`
  (verify: `rg -n "pyz|inspector" crates/djls-semantic/build.rs`).
- Trait surface: `ProjectDb::project_introspector`
  (`crates/djls-semantic/src/project/db.rs:22-23`), implemented by
  `DjangoDatabase` (`crates/djls-db/src/db.rs:187-189` + the
  `project_introspector` field at `:48`) and `TestDatabase`
  (`crates/djls-semantic/src/testing.rs`, single shared instance after
  plan 003).
- Re-export: `crates/djls-semantic/src/project.rs:19`
  (`pub use ... ProjectIntrospector;`) and the matching `lib.rs` export.
- Dependencies that exist for the inspector (verify each is otherwise unused
  before removing from `crates/djls-semantic/Cargo.toml`):
  `rg -n "which|wait_timeout|tempfile|libc" crates/djls-semantic/src crates/djls-semantic/Cargo.toml`.
- Docs that describe the inspector as current behavior:
  `ARCHITECTURE.md:148-162` ("The Python Inspector" section — already
  carries a "planned for replacement" note pointing at issue #401),
  `CONTEXT.md:37-39` ("Project Introspection" glossary term: "expected to
  shrink as Static Extraction matures").

## Commands you will need

| Purpose      | Command                          | Expected on success |
|--------------|----------------------------------|---------------------|
| Build        | `cargo build -q`                 | exit 0              |
| Test (all)   | `cargo test -q`                  | exit 0, all pass    |
| Rust matrix  | `just test`                      | exit 0 (cargo via nox; does NOT run tests/e2e) |
| E2E suite    | `just e2e`                       | exit 0              |
| Lint         | `just clippy`                    | exit 0, no warnings |
| Format       | `just fmt`                       | exit 0              |
| Hooks        | `just lint`                      | exit 0              |
| Cog blocks   | `just cog`                       | exit 0 (only if docs edits touch generated blocks) |

## Scope

**In scope** (the only files you should modify/delete):
- `crates/djls-semantic/src/project/introspector.rs` (delete)
- `crates/djls-semantic/inspector/` (delete directory)
- `crates/djls-semantic/build.rs` (delete the pyz build; delete the file if
  that's all it does)
- `crates/djls-semantic/src/project/db.rs` (drop the trait method)
- `crates/djls-semantic/src/project.rs`, `src/lib.rs` (exports)
- `crates/djls-semantic/Cargo.toml` (deps + build-script entry)
- `crates/djls-db/src/db.rs` (field + impl)
- `crates/djls-semantic/src/testing.rs` (field + impl)
- `crates/djls-bench/` if it implements the trait (`rg project_introspector crates/djls-bench`)
- `ARCHITECTURE.md`, `CONTEXT.md` (docs honesty)
- `CHANGELOG.md` (Removed entry)
- `pyproject.toml` / `uv.lock` only if the inspector had Python-side dev
  dependencies that nothing else uses (verify before touching)

**Out of scope** (do NOT touch, even though they look related):
- `tools/django_facts.py` and `tests/fixtures/django-facts/` (created by
  plan 014) — the dev-only descendant of the inspector. It is a standalone
  script with no dependence on the embedded zipapp or the wire protocol;
  it survives this deletion as the golden-fixture regenerator.
- The interpreter discovery in `project/python.rs` — still used (search
  paths need site-packages); plan 015 later moves it into djls-project.
- The `Interpreter` field on the `Project` input — site-packages discovery
  still consumes it. (Whether it can slim down later is a separate
  decision.)
- The e2e test venv setup — e2e still needs a real project fixture with
  installed packages to exercise search-path discovery.

## Git workflow

jj repo — no mutating `git`. Single commit is fine:
`jj commit -m "refactor: delete the runtime python inspector"`. Do NOT push.

## Steps

### Step 1: Confirm zero callers

`rg -n "project_introspector|ProjectIntrospector|IntrospectionRequest" crates/ --no-heading`

**Verify**: hits only in the files this plan deletes/edits (the trait decl,
the two impls, the introspector module itself, re-exports). Any hit inside a
query or feature path is a STOP.

### Step 2: Delete the module, package, and build step

Delete `introspector.rs`, the `inspector/` directory, and the pyz build in
`build.rs`. Remove `mod introspector;` and the re-export from `project.rs`,
the `lib.rs` export, the `ProjectDb::project_introspector` method
(`project/db.rs:22-23`), the `DjangoDatabase.project_introspector` field and
impl, and the `TestDatabase` equivalents.

**Verify**: `cargo build -q` → exit 0.

### Step 3: Prune dependencies

For each of `which`, `wait-timeout`, `tempfile`, `libc`, `sha2`,
`serde_json` in `crates/djls-semantic/Cargo.toml`: `rg` the crate's `src/`
for remaining use; remove the manifest entry if unused. Check the root
`Cargo.toml` workspace-dependency table for entries that no crate references
anymore (`rg <dep-name> crates/*/Cargo.toml`).

**Verify**: `cargo build -q` → exit 0; `cargo test -q` → all pass.

### Step 4: Docs and changelog

- `ARCHITECTURE.md`: replace the "The Python Inspector" section
  (`:148-162`) with a short "How Knowledge Gets In" description of the
  static chain (settings file → extraction queries → derived facts),
  and update the two-phase-startup paragraph (the cache phase died in plan
  008). Do not hand-edit any cog-generated blocks — update sources and run
  `just cog` if markers are present.
- `CONTEXT.md`: update the "Project Introspection" term — it is now
  historical; keep the term with a note that runtime-backed discovery was
  removed (glossary terms describe the domain, and PRs/issues still
  reference the word).
- `CHANGELOG.md`: "Removed" entry — runtime inspector subprocess, embedded
  zipapp, and `~/.cache/djls/inspector/` disk cache; users no longer need a
  working Django setup for the server to function.

**Verify**: `just lint` → exit 0 (markdown hooks);
`rg -i "inspector" ARCHITECTURE.md` → only historical references remain.

### Step 5: Full validation

**Verify**: `cargo test -q`, `just test`, `just e2e`, `just clippy`, `just fmt`,
`just lint` → all exit 0. Binary sanity: `cargo build -q` then check the
release artifact no longer embeds the zipapp
(`rg -c "djls_inspector" target/debug/djls 2>/dev/null || echo clean` — or
simply confirm `OUT_DIR` references are gone from the source tree).

## Test plan

No new tests — deletions. The whole-suite + e2e pass is the contract: the
server must behave identically with the subprocess machinery gone, because
nothing called it (plan 008's done criteria proved that).

## Done criteria

Machine-checkable. ALL must hold:

- [ ] `rg -i "introspector|inspector" crates/ --no-heading` returns no source matches (docs/changelog history references excepted)
- [ ] `crates/djls-semantic/inspector/` and `introspector.rs` do not exist
- [ ] `rg "include_bytes" crates/djls-semantic/src/` returns no matches
- [ ] `cargo test -q` exits 0
- [ ] `just test` exits 0 and `just e2e` exits 0
- [ ] `just clippy` exits 0
- [ ] Only in-scope files modified (`jj diff --stat`)
- [ ] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- Step 1 finds a live caller (plans 007/008 incomplete or regressed).
- A test exists that asserts inspector *behavior* (not just absence) and
  cannot be deleted as obviously-dead — list such tests and stop.
- Removing `build.rs` breaks something unrelated packaged through it.
- e2e fails in a way that implicates missing runtime data — that means plan
  008's parity gate was not actually met; do not patch around it.

## Maintenance notes

- Issue #401 ("replace the inspector with static settings extraction") can
  be closed when this lands — reference it in the commit/changelog.
- Runtime introspection survives in exactly one place: the dev-only
  golden-fixture generator (`tools/django_facts.py`, plan 014), which
  regenerates the parity contract against real Django. If a future feature
  needs runtime verification (an opt-in doctor command), build it the same
  way — standalone tooling, never a server dependency. The server's
  invariant is now "never runs project Python", full stop.
- The startup-rethink track (deferred; see plans/README.md) gets simpler
  after this: startup is now config-read + Salsa warm-up, no subprocess
  phases to orchestrate.
