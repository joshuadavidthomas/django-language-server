# Plan 016: Create `djls-testing` — corpus plus shared test infrastructure

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: This plan is sequenced after plans 009, 014,
> and 015 (check their README status rows). Then:
> `git diff --stat 922cc4d7..HEAD -- crates/djls-corpus crates/djls-semantic/src/testing.rs`
> The files you move are **whatever stands at execution time**, not the
> planned-at excerpts below — content-match before relying on any line
> number. Tolerated drift: if plan 015 has NOT landed, the `ProjectDb`
> trait still lives at `djls_semantic::ProjectDb` instead of
> `djls_project` — adjust imports accordingly, everything else is
> identical. If plan 014 has not landed, `ProjectFixture` won't exist in
> `testing.rs` — move what is there. If plan 009 has landed (expected),
> the `project_introspector` method and its `ProjectIntrospector` import
> are already gone from `testing.rs` — do not resurrect them.

## Status

- **Priority**: P2
- **Effort**: M
- **Risk**: LOW (test-only surface; no production code changes)
- **Depends on**: plans/014, plans/015 (soft — see drift check)
- **Category**: tech-debt / DX
- **Planned at**: commit `922cc4d7`, 2026-06-10

## Why this matters

Two consolidations, one crate:

1. **The corpus is a library plus a sync tool, not a domain.** `djls-corpus`
   (2,018 lines) exists solely to feed tests and benches real-world Django
   source. A dedicated crate for test data, separate from the test helpers
   that consume it, is an odd boundary — the maintainer wants one *testing*
   crate.
2. **The Salsa test database is hand-rolled six times.** The workspace has
   one production database (`DjangoDatabase` in djls-db) and five
   independent test/bench reimplementations of the same trait stack:
   `djls-semantic/src/testing.rs:145` (the canonical one),
   `djls-semantic/src/python/analysis/calls.rs:176` (duplicating its own
   crate's helper!), `djls-ide/src/formatting.rs:68`,
   `djls-server/src/document.rs:168`, `djls-server/src/workspace.rs:404`,
   plus `djls-bench/src/db.rs:108`. Every database-trait change (plans
   003/007/008/015 all make them) is multiplied across these copies.

The reference design is ty's `ty_test`: a dedicated test-support crate
holding the shared test database and the mdtest harness, consumed as a
dev-dependency by the crates it serves. Cargo explicitly permits the
resulting dev-dependency cycle — verified in `reference/ruff`:
`crates/ty_python_semantic/Cargo.toml:54` dev-depends on `ty_test`, while
`crates/ty_test/Cargo.toml:24` depends back on `ty_python_semantic` as a
normal dependency. Dev-dependencies don't participate in the lib build, so
there is no build cycle.

## Current state

(Excerpts verified at `922cc4d7`; content-match after prerequisite churn.)

- `crates/djls-corpus/` — lib (`Corpus`, `LockFilter`,
  `module_path_from_file`; modules `archive`, `lock`, `manifest`, `sync`),
  a CLI bin (`src/main.rs`, 141 lines), `build.rs` (17 lines), plus
  `manifest.toml`, `manifest.lock`, `fixtures/`, `licenses/`, `README.md`.
  Critical path facts:

  ```rust
  // crates/djls-corpus/src/lib.rs:49-50
  const CORPUS_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/.corpus");
  const LOCKFILE_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/manifest.lock");
  ```

  The synced data lives at `crates/djls-corpus/.corpus` (gitignored via the
  root `.gitignore:201` pattern `.corpus/`, possibly gigabytes). Moving the
  crate moves where the consts point.

  `build.rs` emits `cargo:rustc-cfg=corpus_available` — **dead**: 
  `rg "cfg\(corpus_available\)" crates/ -g '!target'` matches only the
  emitter. The useful parts are the `rerun-if-changed` lines and the
  "Corpus not synced" warning.

- Corpus consumers: `djls-semantic` `[dev-dependencies]`
  (Cargo.toml:30), `djls-bench` `[dependencies]` (Cargo.toml:8). Import
  sites: `djls-semantic/src/testing.rs`, `src/python/testing.rs`,
  `tests/corpus.rs`, `tests/corpus_models.rs`;
  `djls-bench/benches/models.rs`, `benches/check.rs` (inventory again with
  `rg -l "djls_corpus" crates/`).

- Tooling entry points: `Justfile:29-30`
  (`corpus *ARGS:` → `cargo run -q -p djls-corpus -- {{ ARGS }}`) and
  `noxfile.py:108` (`cargo run -p djls-corpus -- sync`).

- `crates/djls-semantic/src/testing.rs` (868 lines) — `#[cfg(test)] mod
  testing;` in `lib.rs:14-15`. Contains:
  - `TestDatabase` (`:145-266`): fields `storage`,
    `fs: Arc<Mutex<InMemoryFileSystem>>`, `files: SourceFiles`, plus
    semantic-trait state (`tag_specs`, `filter_arity_specs`,
    `template_libraries`, `project`); builder-style `with_*` methods,
    `add_file`/`remove_file`, and `#[salsa::db]` impls of
    `salsa::Database`, `djls_source::Db`, `ProjectDb`, `SemanticDb`.
  - Fixture helpers: `builtin_tag_json`/`library_tag_json`/
    `builtin_filter_json`/`library_filter_json`,
    `make_template_libraries` (`:101`), `collect_errors` (`:268`),
    `snapshot_validate`/`snapshot_validate_file` (`:441/:445`), and (post
    plan 014) `ProjectFixture`.
  - `mod mdtest;` (`:1`) → `src/testing/mdtest.rs` (726 lines): the
    markdown snapshot runner. Its entry point is a `#[test]` hardcoding
    the suite location:

    ```rust
    // crates/djls-semantic/src/testing/mdtest.rs:98-100
    #[test]
    fn mdtest() {
        MdtestRun::new(Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/mdtest")).run();
    ```

    The `.md` suites live at `crates/djls-semantic/resources/mdtest/` and
    STAY there (ty precedent: harness in `ty_test`, suites in
    `ty_python_semantic/resources/mdtest`).
- `crates/djls-semantic/src/python/testing.rs` (117 lines) — corpus-grounded
  Python source helpers (`find_function_in_source`, corpus file loaders).
- The duplicate test databases to collapse (verify each before touching):
  - `djls-semantic/src/python/analysis/calls.rs:176` — in-memory, inside
    the same crate as the canonical helper. Pure duplication.
  - `djls-ide/src/formatting.rs:68-91` — in-memory, implements
    `djls_source::Db` only.
  - `djls-server/src/document.rs:168-189` — check whether in-memory.
  - `djls-server/src/workspace.rs:404-425` — holds `fs: Arc<dyn
    FileSystem>` and tests against a real `tempdir()`. **Not** served by an
    in-memory TestDatabase — this one stays local (see Scope).
- Workspace conventions for the new crate (from
  `.claude/skills/djls-workspace-conventions`): crates auto-discovered via
  `members = ["crates/*"]`; versions live in root
  `[workspace.dependencies]`; internal deps listed before third-party,
  blank-line separated; `[lints] workspace = true`; library crates are
  `version = "0.0.0"`; module files are `folder.rs`, never `folder/mod.rs`.

## Commands you will need

| Purpose      | Command                          | Expected on success |
|--------------|----------------------------------|---------------------|
| Build        | `cargo build -q`                 | exit 0              |
| Test (all)   | `cargo test -q`                  | exit 0, all pass    |
| Test (crate) | `cargo test -q -p djls-testing`  | exit 0              |
| Rust matrix  | `just test`                      | exit 0              |
| Corpus CLI   | `cargo run -q -p djls-testing --bin corpus -- --help` | exit 0, help text |
| Lint         | `just clippy`                    | exit 0, no warnings |
| Format       | `just fmt`                       | exit 0              |
| Hooks        | `just lint`                      | exit 0              |

## Scope

**In scope** (the only files you should modify/create/delete):
- `crates/djls-testing/` (create — everything under it)
- `crates/djls-corpus/` (delete, contents moved)
- Root `Cargo.toml` (workspace-deps table: swap djls-corpus → djls-testing)
- `crates/djls-semantic/`: `Cargo.toml`, `src/lib.rs` (drop the testing
  mod), `src/testing.rs` + `src/testing/mdtest.rs` +
  `src/python/testing.rs` (move out), `tests/` (import updates + new
  `tests/mdtest.rs`), in-crate test modules whose imports change,
  `src/python/analysis/calls.rs` (drop inline TestDatabase)
- `crates/djls-ide/`: `Cargo.toml`, `src/formatting.rs` (test mod only)
- `crates/djls-server/`: `Cargo.toml`, `src/document.rs` (test mod only,
  if in-memory)
- `crates/djls-bench/`: `Cargo.toml`, `benches/*.rs` (import updates only)
- `Justfile`, `noxfile.py` (corpus invocation)
- Docs that reference `djls-corpus` as current (sweep:
  `rg -n "djls-corpus|djls_corpus" -g '!target' -g '!plans/' .`)

**Out of scope** (do NOT touch, even though they look related):
- `crates/djls-server/src/workspace.rs` — its TestDb runs against a real
  filesystem (`tempdir()`); per the ty precedent, small local unit-test
  databases are acceptable when they genuinely differ. Leave it.
- `crates/djls-bench/src/db.rs` — the bench database stays. Benches should
  measure a production-shaped database, not a test convenience type
  (see Maintenance notes for the better future consolidation).
- `crates/djls-semantic/resources/mdtest/` — the suites do not move.
- The production `DjangoDatabase` (djls-db) and all database *traits*.
- Test *behavior*: no test may be deleted, weakened, or have its
  assertions changed. This plan moves scaffolding only.

## Git workflow

jj repo — no mutating `git`. Two commits suggested:
`jj commit -m "refactor: move corpus into new djls-testing crate"`, then
`jj commit -m "refactor: consolidate salsa test databases into djls-testing"`.
Do NOT push.

## Steps

### Step 1: Create the crate and absorb the corpus

Create `crates/djls-testing/` and move (move files, do not recreate from
memory):

| From (djls-corpus) | To (djls-testing) |
|---|---|
| `src/lib.rs` | `src/corpus.rs` (module façade for the corpus API) |
| `src/{archive,lock,manifest,sync}.rs` | `src/corpus/{archive,lock,manifest,sync}.rs` |
| `src/main.rs` | `src/main.rs` |
| `build.rs` | `build.rs` — drop the dead `cargo:rustc-cfg=corpus_available` line; keep `rerun-if-changed` + the not-synced warning |
| `manifest.toml`, `manifest.lock`, `fixtures/`, `licenses/`, `README.md` | crate root (same names) |

Mechanical adjustments: internal `crate::archive`-style paths become
`crate::corpus::archive`; create `src/lib.rs` declaring `mod corpus;` (plus
the modules added in Steps 2–3) and re-exporting the corpus API
(`pub use corpus::Corpus;` etc. — mirror exactly what the old
`djls-corpus/src/lib.rs` exported). The `CORPUS_DIR`/`LOCKFILE_PATH`
consts need no edit — `CARGO_MANIFEST_DIR` re-anchors them.

`Cargo.toml` for the new crate: `version = "0.0.0"`,
`[lints] workspace = true`, all deps via `{ workspace = true }`, internal
deps first (after Steps 2–3 these are `djls-conf`, `djls-project` (if plan
015 landed), `djls-semantic`, `djls-source`, `djls-templates`), then
third-party (corpus set: `anyhow`, `camino`, `clap`, `flate2`, `ignore`,
`reqwest`, `serde`, `serde_json`, `tar`, `tempfile`, `toml`, `tracing`,
`tracing-subscriber`; testing set added in Steps 2–3: `pulldown-cmark`,
`ruff_python_ast`, `ruff_python_parser`, `salsa`). The CLI keeps working
under a stable name:

```toml
[[bin]]
name = "corpus"
path = "src/main.rs"
```

Root `Cargo.toml`: replace the `djls-corpus` entry in
`[workspace.dependencies]` with
`djls-testing = { path = "crates/djls-testing" }` (alphabetical within the
internal group). Delete `crates/djls-corpus/` once everything is moved.

**Data migration** (developer machines, not CI): if
`crates/djls-corpus/.corpus` exists, move it:
`mv crates/djls-corpus/.corpus crates/djls-testing/.corpus` — it is
untracked, so this is a plain filesystem move. Otherwise the next
`just corpus sync` re-downloads.

Update consumers' manifests and imports (`djls_corpus::` →
`djls_testing::`): djls-semantic dev-dep, djls-bench dep, the import sites
inventoried in Current state. Update `Justfile:29-30` to
`cargo run -q -p djls-testing --bin corpus -- {{ ARGS }}` (recipe name
`corpus` unchanged — `just corpus sync` keeps working) and `noxfile.py:108`
likewise.

**Verify**: `cargo build -q` → exit 0;
`cargo run -q -p djls-testing --bin corpus -- --help` → help text;
`cargo test -q` → all pass (corpus-gated tests still find their data).

### Step 2: Move the shared test database and fixtures

Move `crates/djls-semantic/src/testing.rs` into the new crate, split by
role (folder.rs convention):

- `src/db.rs` — `TestDatabase`, its builder methods, and the `#[salsa::db]`
  trait impls. Visibility `pub(crate)` → `pub`. Imports flip from
  `crate::...` to `djls_semantic::...` (and `djls_project::...` where plan
  015 moved things).
- `src/fixtures.rs` — the JSON builders, `make_template_libraries`,
  `collect_errors`/`collect_errors_with_revision`,
  `snapshot_validate`/`snapshot_validate_file`, `ProjectFixture` (plan
  014), and the contents of `src/python/testing.rs` (corpus-grounded
  Python-source helpers).

**Before moving, inventory the boundary**: `rg -n "use crate::" crates/djls-semantic/src/testing.rs crates/djls-semantic/src/python/testing.rs`
— every item must be importable through djls-semantic's (or
djls-project's) public API from the new crate. For each item that is NOT
public (e.g. check `builtin_tag_specs`, `ValidationErrorAccumulator`,
`ModelGraph`): if it is a type/function a test-support crate legitimately
needs, add the missing `pub use` to the owning crate's `lib.rs`; if it is
deeply internal, the helper that needs it stays behind in djls-semantic as
a local test helper — list any such leftovers in your report.

In djls-semantic: remove `#[cfg(test)] mod testing;` from `lib.rs`, add
`djls-testing` to `[dev-dependencies]` (this is the sanctioned ty-style
dev-cycle), and update every `crate::testing::` reference in in-crate test
modules to `djls_testing::`. Remove `pulldown-cmark` from djls-semantic's
dev-dependencies once nothing uses it (Step 3 moves the only user).

**Verify**: `cargo test -q -p djls-semantic -p djls-testing` → all pass,
same test count as before the move (compare `cargo test -q -p
djls-semantic 2>&1 | tail -5` before/after).

### Step 3: Move the mdtest runner; suites stay put

Move `crates/djls-semantic/src/testing/mdtest.rs` to
`crates/djls-testing/src/mdtest.rs`. Parameterize the entry point: the
hardcoded `#[test]` (mdtest.rs:98-100 at planned-at) becomes

```rust
pub fn run_suite(dir: &Path) {
    MdtestRun::new(dir.to_path_buf()).run();
}
```

and a thin integration test goes in
`crates/djls-semantic/tests/mdtest.rs`:

```rust
#[test]
fn mdtest() {
    djls_testing::run_suite(&Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/mdtest"));
}
```

The runner's own unit tests (the `#[test]` fns near the bottom of
mdtest.rs, ~`:544-695` at planned-at, which test the markdown parsing —
not the suites) move with it. Preserve the
`DJLS_UPDATE_MDTEST_SNAPSHOTS` update flow untouched.

**Verify**: `cargo test -q -p djls-semantic mdtest` → the suite runs and
passes; deliberately corrupt one snapshot line in a
`resources/mdtest/*.md` file, confirm the test FAILS, revert
(`jj restore <file>`), confirm green again. (This proves the relocated
runner still reads the real suites rather than vacuously passing.)

### Step 4: Collapse the duplicate test databases

One at a time, each followed by that crate's tests:

1. `djls-semantic/src/python/analysis/calls.rs` — delete the inline
   `TestDatabase` (`:176-204+` at planned-at); use
   `djls_testing::TestDatabase`. If the inline one carries state the
   shared one lacks, extend the shared one's `with_*` builders rather
   than keeping the copy.
2. `djls-ide/src/formatting.rs` — delete the inline `TestDb`
   (`:68-91`); add `djls-testing` to djls-ide `[dev-dependencies]`;
   construct `djls_testing::TestDatabase` and use its `add_file`. The
   functions under test take source-layer `&dyn Db` arguments, which the
   full-stack TestDatabase satisfies.
3. `djls-server/src/document.rs` — same treatment **if** its TestDb is
   in-memory. If it, like workspace.rs, needs a real filesystem, leave it
   and note that in your report.

**Verify** after each: `cargo test -q -p <crate>` → all pass, test count
unchanged.

### Step 5: Sweep and full validation

`rg -n "djls_corpus|djls-corpus" . -g '!target' -g '!plans/' -g '!docs/'`
→ fix any straggler (CHANGELOG/docs history references may stay). Update
docs that describe the corpus crate as current (its README moved in Step
1; check `CONTRIBUTING.md`, `ARCHITECTURE.md`).

**Verify**: `cargo test -q`, `just test`, `just clippy`, `just fmt`,
`just lint` → all exit 0.

## Test plan

No new test behavior — this plan relocates scaffolding. The contract is:
identical test counts per crate before/after, plus the Step 3
corrupt-snapshot probe proving the mdtest runner still exercises the real
suites. The corpus CLI is verified by `--help` (do not require a network
sync in CI; if `.corpus` is absent locally, run `just corpus sync` once to
prove the moved tool works end-to-end).

## Done criteria

Machine-checkable. ALL must hold:

- [ ] `crates/djls-corpus/` does not exist; `crates/djls-testing/` builds
- [ ] `rg -l "djls_corpus" crates/` → no matches
- [ ] `just corpus sync` invokes the new crate (read the Justfile diff)
- [ ] `rg -n "struct TestDb|struct TestDatabase" crates/ -g '!target'` matches only `crates/djls-testing/src/db.rs` and (allowed) `crates/djls-server/src/workspace.rs`
- [ ] `rg -n "#\[cfg\(test\)\]\s*mod testing" crates/djls-semantic/src/lib.rs` → no matches
- [ ] mdtest suites still run from djls-semantic (`cargo test -q -p djls-semantic mdtest` lists ≥ 1 test) and the corrupt-snapshot probe failed then passed
- [ ] Per-crate test counts unchanged (record before/after numbers in your report)
- [ ] `cargo test -q` exits 0; `just test` exits 0
- [ ] `just clippy` exits 0
- [ ] Only in-scope files modified (`jj diff --stat`)
- [ ] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- The Step 2 boundary inventory finds a `pub(crate)` item that the moved
  helpers need but that clearly should NOT become public API (e.g. an
  internal validator type) and it cannot reasonably stay behind as a
  local helper either — report the item and the options.
- Cargo rejects the dev-dependency cycle (it should not — the ty
  precedent is cited above; if it does, report the exact error rather
  than restructuring).
- Replacing an inline TestDb changes any test's behavior or count — the
  shared database is supposed to be a superset; if it isn't, report the
  gap.
- The corpus CLI breaks in a way that path-const re-anchoring doesn't
  explain — do not patch path logic ad hoc.

## Maintenance notes

- **The dev-cycle is deliberate and bounded**: djls-testing depends on the
  library crates; only `[dev-dependencies]` point back. Never add
  djls-testing to any crate's `[dependencies]` except djls-bench (whose
  benches are dev-shaped by nature).
- **djls-bench's database**: left alone on purpose. The right future
  consolidation is for benches to construct the slimmed production
  `DjangoDatabase` (post plan 009 it has no inspector baggage), not the
  test database — benches should measure what ships.
- **workspace.rs's local TestDb**: sanctioned (real-filesystem tests).
  If a second OS-backed test database ever appears, add a
  `TestDatabase::with_fs(Arc<dyn FileSystem>)` constructor to
  djls-testing then — not before.
- New shared fixtures belong in `djls-testing/src/fixtures.rs`; resist
  per-crate helper modules growing back.
- Plan 017 (semantic tidy) relocates djls-semantic's in-crate lib.rs
  tests; it assumes this plan's `djls_testing::` imports are in place.
