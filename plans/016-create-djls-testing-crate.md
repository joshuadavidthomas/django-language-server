# Plan 016: Create `djls-testing` — corpus plus shared test infrastructure

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: This plan is sequenced after plans 009, 014,
> and 015 (check their README status rows). Then:
> `jj diff --from d7340eb2 --stat crates/djls-testing crates/djls-semantic/src/testing.rs`
> The files you move are **whatever stands at execution time**, not the
> planned-at excerpts below — content-match before relying on any line
> number. Tolerated drift: if plan 015 has NOT landed, the `ProjectDb`
> trait still lives at `djls_semantic::ProjectDb` instead of
> `djls_project` — adjust imports accordingly, everything else is
> identical. If plan 014 has not landed, `ProjectFixture` won't exist in
> `testing.rs` — move what is there. If plan 009 has landed (expected),
> the `project_introspector` method and its `ProjectIntrospector` import
> are already gone from `testing.rs` — do not resurrect them. If plan 021
> has landed (expected — it is sequenced before this plan), the spec
> extraction subtree lives in djls-project: the corpus-grounded Python
> helpers are at `crates/djls-project/src/specs/testing.rs`, the corpus
> extraction tests at `crates/djls-project/tests/corpus*.rs`, and the
> duplicate inline TestDatabase at
> `crates/djls-project/src/specs/analysis/calls.rs`.
>
> **Mid-execution revision (2026-06-11)**: Step 1 landed as `d7340eb2`
> ("refactor: move corpus into djls-testing"). The original Step 2 then
> failed in execution: it directed in-crate unit tests to import
> `djls_testing::TestDatabase`, which Cargo's crate-identity rules make
> impossible (see "The crate-identity limit" below). The maintainer's
> call: centralization stands — the scaffolding all moves to
> djls-testing, and the tests that consume it relocate to integration
> position (`tests/`), where the identity limit does not apply. Steps
> 2–7 are redesigned accordingly (this also absorbs plan 017's original
> Step 2, the lib.rs test-module move). If you are resuming: the corpus
> move is done; start at Step 2 as now written.

## Status

- **Priority**: P2
- **Effort**: L (was M; the test relocation grew it)
- **Risk**: LOW (test-only surface; no production code changes)
- **Depends on**: plans/014, plans/015, plans/021 (015/021 soft — see
  drift check)
- **Category**: tech-debt / DX
- **Planned at**: commit `922cc4d7`, 2026-06-10
- **Revised**: 2026-06-11, mid-execution at `d7340eb2` — Steps 2–7
  redesigned after the dev-cycle identity blocker: scaffolding
  centralizes (maintainer call), consuming tests relocate to `tests/`.
  Effort M → L (the relocation is the added cost)
- **Execution status**: PR #670 open from bookmark
  `plan-016-create-djls-testing`, head `60a9472a`; one review
  correction outstanding (the corpus-fixture vendoring pass below)

## Execution record — PR #670 (2026-06-11)

Steps 2–7 landed as `d6e91ff9` → `88e3567c` and PR #670 opened. Full
validation passed at that head (`cargo build -q`, corpus CLI `--help`,
`cargo test -q`, `just test`, clean-tree `just clippy`,
`just fmt --check`, `just lint`).

**Review verdict on `88e3567c`**: the centralization core was accepted
as planned (shared `TestDatabase`/fixtures/mdtest in djls-testing;
djls-semantic fully scrubbed; project/ide tests in true integration
position; snapshots re-keyed byte-identically). Rejected: four
internal-shaped test modules had been physically parked under
`tests/support/` and compiled back in-crate via
`#[cfg(test)] #[path = "../tests/support/…"] mod tests;`. That hybrid
satisfied the placement guard only textually — most acutely
`djls-server`'s document tests, which imported the shared
`djls_testing::TestDatabase` from what is semantically `src/` unit-test
code — and left `tests/support/` as a trap (unit modules with `crate::`
paths sitting where integration-test helpers conventionally live;
djls-server's `tests/` had no test target at all). The revert landed as
`60a9472a` ("test: restore private unit test modules"): all four
modules back in `src/` as plain `#[cfg(test)] mod tests`, document.rs
back on its minimal local source-only `TestDb`, `tests/support/`
deleted.

**Corpus ruling (maintainer call, 2026-06-11, supersedes the interim
`Corpus` carve-out)**: the synced corpus is an **integration-boundary
asset** — unit tests must run without external syncing. The
corpus-grounded unit tests in `djls-project`
(`src/extraction/registry.rs` `mod tests`, ~21 tests asserting
recognizer behavior on real Django decorators, and the
`src/specs/analysis/calls.rs` tests fed by `src/specs/testing.rs`
helpers) therefore do not import `djls_testing::Corpus`. Instead they
**vendor pinned fixtures**: inline snippet literals (or `include_str!`
from a small in-crate `testdata/` dir for function bodies) with
provenance comments naming the corpus file they came from — the
existing `// Corpus: ...` comments become citations. The corpus-access
helpers in `specs/testing.rs` (`corpus_source`, `package_source`,
`django_source`) are deleted; pure parsing helpers
(`find_function_in_source`) stay. Drift defense in depth: live-corpus
coverage continues in `tests/corpus*.rs`, which glob-snapshot
`extract_rules` over every extraction target. **This vendoring pass is
the remaining work on PR #670** — at `60a9472a` the `Corpus` imports
are still present in those two `src/` test modules.

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

   The consolidation target (maintainer call, 2026-06-11): **one shared
   `TestDatabase` and one fixtures home, both in djls-testing.** Cargo's
   crate-identity rules (next section) forbid in-crate `#[cfg(test)]`
   unit tests from consuming them — so the tests that need the
   scaffolding move to integration position (`tests/`), where the limit
   does not apply. In-crate test modules keep only what genuinely cannot
   leave: tests that assert on `pub(crate)` internals, each inventoried
   in the executor report with a minimal local helper if it needs a
   database at all. (ty keeps a fat in-crate `TestDb` because its unit
   tests probe type-inference internals; DLS's database-consuming tests
   are overwhelmingly public-API-shaped — validate/scope/outline a
   template and assert on public outputs — so DLS can centralize further
   than ty does.) The sanctioned exceptions that remain: the
   real-filesystem database in `workspace.rs` and the production-shaped
   bench database.

The reference design is ty's `ty_test`: a dedicated test-support crate
holding the shared test database and the mdtest harness, consumed as a
dev-dependency by the crates it serves. Cargo explicitly permits the
resulting dev-dependency cycle — verified in `reference/ruff`:
`crates/ty_python_semantic/Cargo.toml:54` dev-depends on `ty_test`, while
`crates/ty_test/Cargo.toml:24` depends back on `ty_python_semantic` as a
normal dependency. Dev-dependencies don't participate in the lib build, so
there is no build cycle.

### The crate-identity limit (why the original Step 2 failed)

Cargo permits the dev-cycle, but resolves it by building the host crate
twice: once as the normal lib (the copy djls-testing links) and once as
the unit-test crate (`#[cfg(test)]` enabled). Those are two distinct
crate identities; Rust traits and types are nominal across them. So a
`djls_testing::TestDatabase` that implements the *lib copy's*
`djls_semantic::Db` does not satisfy `crate::db::Db` inside
djls-semantic's own `#[cfg(test)]` modules — the build fails with
`the trait bound TestDatabase: db::Db is not satisfied` and
`note: there are multiple different versions of crate djls_semantic in
the dependency graph`. Integration tests under `tests/` are exempt: they
link the host crate externally, the same copy djls-testing links.

ty obeys exactly this line, which the original plan missed when citing
it: `ty_python_semantic` keeps an in-crate `pub(crate) struct TestDb`
(`src/db.rs:50` in `reference/ruff`) for unit tests and consumes
`ty_test` only from integration position (`tests/mdtest.rs`,
`tests/corpus.rs`).

What identity makes *possible*: the shared scaffolding works from
integration tests of any crate, and also from in-crate unit tests of
crates djls-testing does not depend on (djls-ide, djls-server, djls-db,
djls-bench).

The convention this plan adopts is stricter and uniform (maintainer
call, 2026-06-11), so nobody has to remember which side of djls-testing
a crate sits on:

- **Any test that constructs a salsa database lives in its crate's
  `tests/` directory** — even in crates where an in-crate
  `djls_testing::` import would be legal. One rule; it also survives
  future changes to djls-testing's dependency set.
- In-crate `#[cfg(test)]` modules are for tests of `pub(crate)`
  internals. An internal-shaped test that genuinely needs a database is
  a **recorded exception**: it stays in-crate with a minimal local
  helper (never a `djls_testing::` import — the uniform guard below
  checks `src/` wholesale). `workspace.rs` is the standing example.
- Consequently `djls_testing::` never appears under any crate's `src/`
  — machine-checkable in the done criteria.
- djls-testing items that mention no downstream types (`Corpus`,
  `module_path_from_file`) are identity-safe everywhere — which is why
  Step 1's imports inside `djls-semantic/src/testing.rs` compile fine —
  but the uniform rule applies to them all the same: test code that
  needs them sits in `tests/` (or `benches/`). Identity-safety earns no
  carve-out (ruled at PR #670 review): an in-crate test that wants
  corpus source vendors a pinned snippet with a provenance comment
  instead — unit tests must run without external syncing, and the
  live-corpus coverage belongs to integration tests.
- Physical placement is not the rule's substance — a module
  `#[path]`-included from `tests/` into `src/` is still an in-crate
  unit test and counts as `src/` for every rule above. No `#[path]`
  test includes, period.

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
  sites at planned-at: `djls-semantic/src/testing.rs`,
  `src/python/testing.rs`, `tests/corpus.rs`, `tests/corpus_models.rs`;
  `djls-bench/benches/models.rs`, `benches/check.rs`. Post-021 the
  `python/testing.rs` and `tests/corpus*.rs` sites live in djls-project
  (which also dev-depends on the corpus). Inventory again with
  `rg -l "djls_corpus" crates/` — the sweep is authoritative.

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

**In scope** (the only files you should modify/create/delete; Step 1
items already landed in `d7340eb2`):
- `crates/djls-testing/`: `Cargo.toml` (internal deps), `src/lib.rs`
  (new modules), `src/db.rs` (new — shared TestDatabase),
  `src/fixtures.rs` (new — shared fixture helpers), `src/mdtest.rs`
  (moved runner + its curated validation environment)
- `crates/djls-semantic/`: `Cargo.toml`, `src/lib.rs` (drop `mod
  testing;` and the inline `mod tests`), `src/testing.rs` +
  `src/testing/mdtest.rs` (move out), in-crate `#[cfg(test)]` modules
  that consume `crate::testing::` (relocate to `tests/`), new
  `tests/*.rs`, insta snapshot files (re-keyed by the relocation)
- `crates/djls-project/`: `src/testing.rs` + `src/specs/testing.rs`
  (move out or minimize per the inventory), `src/specs/analysis/calls.rs`
  (drop inline TestDatabase), in-crate test modules that consume the
  moved helpers (relocate to `tests/`), new `tests/*.rs`, snapshot files
- `crates/djls-ide/`: `Cargo.toml`, `src/formatting.rs` (test mod only)
- `crates/djls-server/`: `Cargo.toml`, `src/document.rs` (test mod only,
  if in-memory)
- Docs that describe the test-infrastructure layout (`ARCHITECTURE.md`,
  `CONTRIBUTING.md` — sweep in Step 7)

**Out of scope** (do NOT touch, even though they look related):
- `crates/djls-server/src/workspace.rs` — its TestDb runs against a real
  filesystem (`tempdir()`); a local database is sanctioned when it
  genuinely differs. Leave it.
- `crates/djls-bench/src/db.rs` — the bench database stays. Benches should
  measure a production-shaped database, not a test convenience type
  (see Maintenance notes for the better future consolidation).
- `crates/djls-semantic/resources/mdtest/` — the suites do not move.
- The production `DjangoDatabase` (djls-db) and all database *traits*.
- Test *behavior*: no test may be deleted, weakened, or have its
  assertions changed. This plan moves scaffolding only.

## Git workflow

jj repo — no mutating `git`. Step 1 is already committed (`d7340eb2`,
"refactor: move corpus into djls-testing"). Remaining commits suggested,
one per step so each lands green:
`jj commit -m "refactor: add shared test database and fixtures to djls-testing"`,
`jj commit -m "refactor: move mdtest runner into djls-testing"`,
`jj commit -m "refactor: move djls-semantic scaffolding tests to integration position"`,
`jj commit -m "refactor: move djls-project scaffolding tests to integration position"`,
`jj commit -m "refactor: collapse duplicate test databases"`.
Do NOT push.

## Steps

### Step 1: Create the crate and absorb the corpus — DONE (`d7340eb2`)

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

### Step 2: Build the shared database and fixtures in djls-testing

(Redesigned 2026-06-11. The scaffolding is COPIED into djls-testing in
this step and the original deleted in Step 4, after its consumers have
relocated — that ordering keeps every step green.)

Copy `crates/djls-semantic/src/testing.rs` into djls-testing (copy the
file, do not recreate from memory), split by role (folder.rs
convention):

- `src/db.rs` — `TestDatabase`, its builder methods, and the
  `#[salsa::db]` trait impls (`salsa::Database`, `djls_source::Db`,
  `djls_project::Db`, `djls_semantic::Db`).
- `src/fixtures.rs` — the JSON builders (`builtin_tag_json`,
  `library_tag_json`, `builtin_filter_json`, `library_filter_json`),
  `make_template_libraries`, `collect_errors`/
  `collect_errors_with_revision`, `is_argument_validation_error`,
  `collect_argument_validation_errors_with_revision`, `ProjectFixture`,
  and the corpus-grounded extraction helpers (`extract_and_merge`,
  `build_specs_from_extraction`, `build_entry_specs`). The mdtest
  curated environment (`standard_validation_db` and below) moves in
  Step 3, not here.

Adaptations:

- Visibility `pub(crate)` → `pub`; imports flip from `crate::...` to
  `djls_semantic::...` public paths. Everything the database and
  collect-helpers need is already public — verified at `d7340eb2`:
  `validate_nodelist` (`lib.rs:65`), `ValidationErrorAccumulator`
  (`lib.rs:16`), `builtin_tag_specs` (`lib.rs:36`), `TagSpecs`,
  `FilterAritySpecs`, `ValidationError`. For anything that turns out
  non-public: add a `pub use` to the owning crate's `lib.rs` if a
  test-support crate legitimately needs it; if it is deeply internal,
  STOP and report rather than widening.
- `collect_errors` and friends should take the shared `TestDatabase`
  (or `&dyn djls_semantic::Db` where that is strictly more useful —
  prefer the concrete type; generality without a second consumer is
  speculation).
- `crates/djls-testing/Cargo.toml` gains the internal deps the new
  modules demand (expected: `djls-conf`, `djls-project`,
  `djls-semantic`, `djls-source`, `djls-templates`, plus `salsa`; let
  compile errors be authoritative). Internal deps before third-party,
  blank-line separated. This creates the dev-cycle — the sanctioned ty
  shape.
- `crates/djls-testing/src/lib.rs` declares `mod db;` / `mod fixtures;`
  and re-exports the API (`pub use db::TestDatabase;`,
  `pub use fixtures::...`).

djls-semantic changes in this step: **none**. Its `testing.rs` still
exists and its in-crate tests still pass against it; Step 4 retires it.

**Verify**: `cargo build -q` → exit 0; `cargo test -q` → unchanged (the
new modules gain consumers in Steps 3–6); `just clippy` → exit 0.

### Step 3: Move the mdtest runner; suites stay put

Move `crates/djls-semantic/src/testing/mdtest.rs` to
`crates/djls-testing/src/mdtest.rs`. The runner reaches into semantic's
testing module for exactly two items (`mdtest.rs:16-17`):
`snapshot_validate` and `snapshot_validate_file`. Those — together with
the curated environment behind them (`render_validate_snapshot`,
`standard_validation_db`, `standard_tag_specs`,
`standard_template_libraries`, `standard_filter_arities`,
`set_tag_rule`, and the curated rule fns `autoescape_rule` …
`widthratio_rule`) — move WITH the runner into djls-testing,
reconstructed on top of Step 2's `djls_testing::TestDatabase`.

Before moving them, check for consumers beyond the runner:
`rg -n "snapshot_validate|standard_validation_db" crates/djls-semantic/src/`
— expected matches are only `testing.rs` (definitions) and
`testing/mdtest.rs` (uses). If an in-crate unit test also uses them,
note it: that test relocates to `tests/` in Step 4 like every other
scaffolding consumer.

Dependency churn: `pulldown-cmark` moves from djls-semantic
`[dev-dependencies]` to djls-testing `[dependencies]` (the runner is its
only user). djls-semantic gains nothing new — its dev-dep on
djls-testing already exists from Step 1.

Parameterize the entry point: the hardcoded `#[test]`
(mdtest.rs:98-100 at planned-at) becomes

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

### Step 4: Relocate djls-semantic's scaffolding tests; retire its testing.rs

Inventory the consumers: `rg -ln "crate::testing::" crates/djls-semantic/src/`
— expected at `d7340eb2`: `lib.rs` (the ~720-line inline `mod tests`),
`references.rs`, `offset.rs`, `structure/tree.rs`,
`structure/outline.rs`, `structure/opaque.rs`. The sweep is
authoritative.

Classify each consuming test:

- **Public-API-shaped** (builds a database, calls public items, asserts
  on public outputs — the overwhelming majority): move to an
  integration test under `crates/djls-semantic/tests/`. Imports flip
  `crate::X` → `djls_semantic::X` and `crate::testing::Y` →
  `djls_testing::Y`. Group by area rather than one file per source
  module — each `tests/*.rs` is a separate crate to compile. Suggested:
  `tests/validation.rs` (lib.rs's inline module — this absorbs plan
  017's original Step 2), `tests/structure.rs` (tree/outline/opaque),
  `tests/references.rs`, `tests/offset.rs` (merge small ones where
  sensible).
- **Internal-shaped** (asserts on `pub(crate)` items): stays in-crate.
  Do NOT widen visibility to force a move. List each in your report; if
  one needs a database, see the STOP conditions.

Insta snapshots re-key when their test's module path changes: run the
suite, accept the *renamed* snapshot files, then **diff old vs new
content below the insta metadata header — it must be byte-identical**.
Delete the orphaned old `.snap` files.

When the last consumer has relocated: delete
`crates/djls-semantic/src/testing.rs`, remove `#[cfg(test)] mod
testing;` and the inline `mod tests` from `lib.rs`, and drop
djls-semantic dev-deps nothing uses anymore (dev-deps still serve
`tests/`, so check before cutting — `insta` likely stays).

**Verify**: `cargo test -q -p djls-semantic` → all pass, same total test
count as before (unit + integration; record both numbers);
`rg -n "crate::testing" crates/djls-semantic/src/` → no matches.

### Step 5: Same treatment for djls-project

Inventory: `rg -ln "crate::testing::|crate::specs::testing::" crates/djls-project/src/`
— expected at `d7340eb2`: `templates.rs`, `settings.rs`, `resolve.rs`,
`specs/analysis/calls.rs`, plus whatever uses the corpus-grounded
helpers in `specs/testing.rs`. The sweep is authoritative.

Same classification rule as Step 4. The shared
`djls_testing::TestDatabase` implements `djls_project::Db`, so
relocated project tests use it directly; fold any project-specific
helpers the relocated tests need into `djls-testing/src/fixtures.rs`
(they may already be covered by the Step 2 copy of the extraction
helpers).

`specs/analysis/calls.rs` and its inline `TestDatabase` (`:176` at
`d7340eb2`): if its tests are public-API-shaped, they relocate and the
inline database dies with them. If they assert on `pub(crate)` analysis
internals (likely), they stay in-crate — then keep exactly ONE minimal
in-crate database for all remaining internal tests (collapse the
`calls.rs` inline copy and `src/testing.rs` into whichever single
definition the residue needs) and record it in your report.

When done: `src/testing.rs` and `src/specs/testing.rs` are deleted or
reduced to the single justified residue.

**Verify**: `cargo test -q -p djls-project` → all pass, counts
unchanged; `rg -n "crate::testing|crate::specs::testing" crates/djls-project/src/`
→ no matches beyond the justified residue.

### Step 6: Collapse the remaining duplicate test databases

One at a time, each followed by that crate's tests. The identity limit
does not bind here (djls-testing depends on neither crate), but the
uniform rule does: the relocation to `tests/` happens anyway.

1. `djls-ide/src/formatting.rs` — delete the inline `TestDb`
   (`:68-91`) and move its tests to `crates/djls-ide/tests/` (e.g.
   `tests/formatting.rs`), constructed on `djls_testing::TestDatabase`
   with `add_file`; add `djls-testing` to djls-ide
   `[dev-dependencies]`. The functions under test take source-layer
   `&dyn Db` arguments, which the full-stack TestDatabase satisfies. If
   any of them turn out to be `pub(crate)`, apply the Step 4
   classification rule: that test is internal-shaped and stays in-crate
   with a minimal local helper, recorded in your report.
2. `djls-server/src/document.rs` — same treatment **if** its TestDb is
   in-memory and the tested items are public. djls-server is glue, so
   expect internal-shaped tests here; if so (or if it, like
   workspace.rs, needs a real filesystem), leave them in-crate with
   their minimal local database and record it — that is the rule's
   exception clause working as designed, not a failure.

**Verify** after each: `cargo test -q -p <crate>` → all pass, total test
count unchanged.

### Step 7: Sweep and full validation

`rg -n "djls_corpus|djls-corpus" . -g '!target' -g '!plans/' -g '!docs/'`
→ fix any straggler (CHANGELOG/docs history references may stay). Update
docs that describe the corpus crate as current (its README moved in Step
1; check `CONTRIBUTING.md`, `ARCHITECTURE.md`).

**Verify**: `cargo test -q`, `just test`, `just clippy`, `just fmt`,
`just lint` → all exit 0.

## Test plan

No new test behavior — this plan relocates scaffolding and the tests
that consume it. The contract is: identical *total* test count per crate
before/after (tests redistribute from in-crate `#[cfg(test)]` to
`tests/`, so record unit and integration counts separately), snapshot
*content* byte-identical below the insta metadata header (renames from
re-keying are expected), plus the Step 3 corrupt-snapshot probe proving
the mdtest runner still exercises the real suites. The corpus CLI is
verified by `--help` (do not require a network sync in CI; if `.corpus`
is absent locally, run `just corpus sync` once to prove the moved tool
works end-to-end).

## Done criteria

Machine-checkable. ALL must hold:

- [ ] `crates/djls-corpus/` does not exist; `crates/djls-testing/` builds
- [ ] `rg -l "djls_corpus" crates/` → no matches
- [ ] `just corpus sync` invokes the new crate (read the Justfile diff)
- [ ] `rg -n "struct TestDb|struct TestDatabase" crates/ -g '!target'` matches only `crates/djls-testing/src/db.rs`, `crates/djls-server/src/workspace.rs`, and the internal-shaped residues recorded in the Step 4/5/6 reports (expected: at most djls-project's specs analysis and djls-server's document tests)
- [ ] `rg -n "djls_testing" crates/*/src/ -g '!djls-testing/**'` → no matches (the uniform guard: shared test scaffolding is consumed only from `tests/` and `benches/`, in every crate — zero exceptions, including `Corpus`)
- [ ] `rg -n "#\[path" crates/*/src/` → no matches (no test modules path-included from outside `src/`; `tests/support/` directories do not exist)
- [ ] `rg -n "djls_testing::Corpus" crates/djls-project/src/` → no matches (corpus-grounded unit tests vendor pinned fixtures; corpus reads happen only in `tests/corpus*.rs`)
- [ ] `rg -n "mod testing" crates/djls-semantic/src/lib.rs` → no matches; `rg -c "#\[cfg\(test\)\]" crates/djls-semantic/src/lib.rs` → 0 (the inline test module moved to `tests/validation.rs`)
- [ ] mdtest suites still run from djls-semantic (`cargo test -q -p djls-semantic mdtest` lists ≥ 1 test) and the corrupt-snapshot probe failed then passed
- [ ] Per-crate test counts unchanged (record before/after numbers in your report)
- [ ] `cargo test -q` exits 0; `just test` exits 0
- [ ] `just clippy` exits 0
- [ ] Only in-scope files modified (`jj diff --stat`)
- [ ] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- The Step 2 copy or the Step 3 move needs a `pub(crate)` item that
  clearly should NOT become public API (e.g. an internal validator
  type) — report the item and the options.
- An **internal-shaped** test (asserts on `pub(crate)` items) needs the
  shared database: it cannot import it (identity limit) and its target
  is not public (cannot relocate). Do not widen visibility or duplicate
  the database to unblock yourself — report the test and the options.
- An internal-shaped test needs corpus source. Do not import
  `djls_testing::Corpus` in `src/`, do not `#[path]`-include the module
  from `tests/`, and do not copy corpus files in a build script —
  vendor the needed snippet inline with a provenance comment. If the
  needed input is too large to vendor sensibly, report.
- The Step 4/5 classification finds that a *large share* of consuming
  tests are internal-shaped (more than a handful of modules) — the
  centralization premise would be wrong; report before relocating
  anything.
- Replacing an inline TestDb changes any test's behavior or count — the
  shared database is supposed to be a superset; if it isn't, report the
  gap.
- Any snapshot's content below the insta metadata header differs after
  re-keying — that means behavior changed, not just location.
- The corpus CLI breaks in a way that path-const re-anchoring doesn't
  explain — do not patch path logic ad hoc.

## Maintenance notes

- **The dev-cycle is deliberate and bounded**: djls-testing depends on the
  library crates; only `[dev-dependencies]` point back. Never add
  djls-testing to any crate's `[dependencies]` except djls-bench (whose
  benches are dev-shaped by nature).
- **The identity rule is permanent, not transitional**: in-crate
  `#[cfg(test)]` modules of crates djls-testing depends on can never
  import `djls_testing::` scaffolding (Cargo builds the host crate
  twice across a dev-cycle; trait identities don't unify). The standing
  convention this plan establishes is deliberately broader than the
  rule Cargo forces: **any test that constructs a salsa database lives
  in `tests/`, in every crate; in-crate test modules are for
  `pub(crate)` internals only** (recorded exception: an internal-shaped
  test that needs a database keeps a minimal local helper). One rule to
  review against, no dependency-graph qualifier, and `rg "djls_testing"
  crates/*/src/` staying empty is the whole enforcement story.
  Placement is semantic, not physical: `#[path]`-including a module
  from `tests/` into `src/` does not move it out of the unit-test
  binary and is banned outright.
- **The corpus is an integration-boundary asset** (ruled at PR #670
  review): unit tests never require a synced corpus — no
  `djls_testing::Corpus` under `src/`, no build-script copying.
  Recognizer-level tests pin real-world snippets inline with
  provenance comments naming the corpus file; corpus-wide coverage
  lives in `tests/corpus*.rs` glob snapshots. Defense in depth: the
  pinned snippets give stage isolation, the integration snapshots
  catch real-Django drift.
- **djls-bench's database**: left alone on purpose. The right future
  consolidation is for benches to construct the slimmed production
  `DjangoDatabase` (post plan 009 it has no inspector baggage), not the
  test database — benches should measure what ships.
- **workspace.rs's local TestDb**: sanctioned (real-filesystem tests).
  If a second OS-backed test database ever appears, add a
  `TestDatabase::with_fs(Arc<dyn FileSystem>)` constructor to
  djls-testing then — not before.
- New shared fixtures belong in `djls-testing/src/fixtures.rs`; resist
  per-crate helper modules growing back. The only sanctioned in-crate
  test scaffolding is the minimal residue serving internal-shaped tests
  (Step 5's report lists it).
- Plan 017 (semantic tidy) originally moved the lib.rs inline test
  module itself; this plan absorbed that step (`tests/validation.rs`).
  017 is re-scoped to the trait deletion and export audit.
