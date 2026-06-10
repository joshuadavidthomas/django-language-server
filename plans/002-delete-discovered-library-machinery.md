# Plan 002: Delete the never-populated "Discovered library" machinery

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat 922cc4d7..HEAD -- crates/djls-semantic/src/project/symbols.rs crates/djls-semantic/src/validation.rs crates/djls-semantic/src/validation/scoping.rs crates/djls-semantic/src/errors.rs crates/djls-ide/src crates/djls-semantic/src/testing.rs crates/djls-db/src/db.rs`
> If any in-scope file changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding; on a
> mismatch, treat it as a STOP condition.

## Status

- **Priority**: P1
- **Effort**: M
- **Risk**: MED
- **Depends on**: none (001 recommended first)
- **Category**: tech-debt
- **Planned at**: commit `922cc4d7`, 2026-06-10
- **Status**: DONE
- **Implemented at**: commit `3913dce8`, 2026-06-10
- **PR**: https://github.com/joshuadavidthomas/django-language-server/pull/654

## Why this matters

`TemplateLibraries` carries a dual-source merge design ("Discovered" libraries
found by scanning sys.path vs "Active" libraries reported by the runtime
inspector) in which the Discovered half is **never populated in production**:
`TemplateLibrary::new_discovered` is called only from test code, and
`TemplateLibraries.discovery_knowledge` is permanently `Knowledge::Unknown`
outside tests (a `djls-db` test asserts exactly that). Every consumer of the
Discovered half is therefore dead in production, including two
`ValidationError` variants that can never fire. Plans 007–008 rebuild library
knowledge from static extraction; this leftover surface would otherwise have
to be threaded through all of that work.

## Current state

- `crates/djls-semantic/src/project/symbols.rs:66-94` — the status enum and
  dead constructor:

  ```rust
  pub enum LibraryStatus {
      Discovered(LibraryOrigin),
      Active { module: PyModuleName, origin: Option<LibraryOrigin> },
      Builtin { module: PyModuleName },
  }
  ...
  pub fn new_discovered(name: LibraryName, origin: LibraryOrigin) -> Self {
  ```

  Production constructors are `new_active`/`new_builtin` only. Callers of
  `new_discovered`: `crates/djls-semantic/src/testing.rs:755` and
  `crates/djls-ide/src/hover.rs:303` — both inside `#[cfg(test)] mod tests`
  (hover's test module starts at `hover.rs:258`).

- `symbols.rs:199-218` — `TemplateLibraries` with the dead field:

  ```rust
  pub struct TemplateLibraries {
      pub active_knowledge: Knowledge,
      pub discovery_knowledge: Knowledge,   // permanently Unknown in production
      ...
  ```

  Methods gated on `discovery_knowledge != Knowledge::Known` (so always
  returning `None` in production) live at `symbols.rs:392`, `:403`, and
  `:431` (`discovered_symbol_candidates_by_name`, which builds
  `HashMap<TemplateSymbolName, Vec<DiscoveredSymbolCandidate>>`).

- `crates/djls-semantic/src/validation.rs:59-91` — the validator caches the
  always-`None` maps:

  ```rust
  env_tags: Option<HashMap<TemplateSymbolName, Vec<DiscoveredSymbolCandidate>>>,
  env_filters: Option<HashMap<TemplateSymbolName, Vec<DiscoveredSymbolCandidate>>>,
  ...
  let env_tags =
      template_libraries.discovered_symbol_candidates_by_name(TemplateSymbolKind::Tag);
  ```

  and passes them to scoping at `validation.rs:149` and `:183`.

- `crates/djls-semantic/src/validation/scoping.rs:28-53` and `:84-107` — the
  `env_tags`/`env_filters` parameters guard emission of
  `ValidationError::TagNotInInstalledApps` / `FilterNotInInstalledApps`;
  with the maps always `None`, those branches are unreachable in production.

- `crates/djls-semantic/src/project.rs:26` —
  `pub(crate) use crate::project::symbols::DiscoveredSymbolCandidate;`

- `crates/djls-db/src/db.rs:557-569` — test
  `discovered_template_libraries_stored_on_project` asserting
  `discovery_knowledge == Knowledge::Unknown`.

- The `Knowledge` enum itself (`symbols.rs:169-172`, variants `Known`/`Unknown`)
  **stays** — `active_knowledge` is real production gating
  (`scoping.rs:31`, `:87`, `:140`).

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
- `crates/djls-semantic/src/project/symbols.rs`
- `crates/djls-semantic/src/project.rs` (remove the re-export)
- `crates/djls-semantic/src/validation.rs`
- `crates/djls-semantic/src/validation/scoping.rs`
- `crates/djls-semantic/src/errors.rs` (remove the two dead variants)
- `crates/djls-semantic/src/testing.rs` (test fixtures)
- `crates/djls-ide/src/hover.rs`, `crates/djls-ide/src/completions.rs` (test-only uses)
- `crates/djls-db/src/db.rs` (the one assertion test)
- Snapshot files and docs rows that mention the two deleted error codes (see Step 5)

**Out of scope** (do NOT touch, even though they look related):
- `LibraryStatus::Active { origin }` and `LibraryOrigin` — origins on Active
  libraries are real (the inspector reports them) and hover uses them.
- `active_knowledge` and the `Knowledge` enum — live production gating.
- `apply_active_snapshot` (`symbols.rs:473`) — the live inspector-apply path.
  Only remove the lines inside it that read/write `Discovered` status, if any.
- `crates/djls-semantic/inspector/` and the introspector — plan 009.

## Git workflow

jj repo — no mutating `git`. Finish with:
`jj commit -m "refactor: delete never-populated discovered-library machinery"`.
Do NOT push.

## Steps

### Step 1: Confirm the production-dead premise

Run: `rg -n "new_discovered|LibraryStatus::Discovered" crates/ --no-heading`

**Verify**: every construction site (`new_discovered(` calls,
`LibraryStatus::Discovered(` literals outside `symbols.rs` itself) is inside a
`#[cfg(test)]` module or `testing.rs`. If any production code constructs
either, STOP.

### Step 2: Remove the validator/scoping dead path

In `validation.rs`: delete the `env_tags`/`env_filters` fields (lines 59-60),
their construction (78-81), and the two arguments at the
`check_tag_scoping_rule`/`check_filter_scoping_rule` call sites (149, 183).
In `scoping.rs`: delete the `env_tags`/`env_filters` parameters and the
`if let Some(env_tags) = env_tags ...` blocks (the `TagNotInInstalledApps` /
`FilterNotInInstalledApps` emissions); keep the `UnknownTag`/`UnknownFilter`
fallthrough that follows them. Remove the now-unused
`DiscoveredSymbolCandidate` import (`scoping.rs:12`).

**Verify**: `cargo build -q` → exit 0 (expect dead-code warnings next step).

### Step 3: Remove the symbols machinery

In `symbols.rs`, delete: `LibraryStatus::Discovered` variant and its match
arms (`module()` at :121, `origin()` at :129, plus any others the compiler
finds); `new_discovered` (:88-94); the `discovery_knowledge` field, its
`Default` line (:212), and all writes; the three methods gated on it (:392,
:403, :431 — `discovered_symbol_candidates_by_name` and the two name-map
getters); `DiscoveredSymbolCandidate` (:193-197); `has_discovered_library`
and `discovered_loadable_libraries` **if** `rg` shows their only remaining
callers are tests — `completion_library_names` (:270-280) currently filters
`is_enabled_library(name) || has_discovered_library(name)`; reduce it to
`is_enabled_library(name)`. Remove the `project.rs:26` re-export.

**Verify**: `cargo build -q` → exit 0; `rg "discovery_knowledge|DiscoveredSymbolCandidate|new_discovered" crates/` → no matches.

### Step 4: Delete the dead error variants

In `errors.rs`, remove `ValidationError::TagNotInInstalledApps` and
`FilterNotInInstalledApps` and everything the compiler then flags: their
diagnostic-code mapping, severity mapping, and message rendering arms.
Find any remaining references: `rg -n "NotInInstalledApps" crates/ docs/`.

**Verify**: `cargo build -q` → exit 0.

### Step 5: Fix tests, snapshots, and docs

- `crates/djls-db/src/db.rs:557-569`: delete the
  `discovered_template_libraries_stored_on_project` test or reduce it to
  asserting `loadable.is_empty()`.
- `crates/djls-semantic/src/testing.rs` (~:740-760): remove the
  discovered-library fixture helpers and any tests that exercised
  `TagNotInInstalledApps`-class behavior.
- `crates/djls-ide/src/hover.rs` test module: delete
  `discovered_symbol_hover_shows_load_hint` (:269-…) and any sibling tests
  constructing `discovery_knowledge`. In `completions.rs` tests
  (:805, :839, :877), remove the `discovery_knowledge: Knowledge::Known`
  struct-literal fields.
- If the deleted error codes appear in `docs/` (check
  `rg -n "NotInInstalledApps|S1" docs/template-validation.md`), remove those
  rows. If they sit inside a cog-generated block (look for cog markers),
  update the source of truth and run `just cog` instead of hand-editing.

**Verify**: `cargo test -q` → all pass. If insta reports snapshot changes,
inspect each: only snapshots that referenced the two deleted error codes may
change; any other snapshot diff is a STOP condition.

### Step 6: Full validation

**Verify**: `cargo test -q`, `just clippy`, `just fmt`, `just lint` → all exit 0.

## Test plan

No new tests. Deleted tests are exactly those that constructed the
never-produced state. The remaining suite must pass unchanged — in
particular `djls-semantic` scoping tests for `UnknownTag`, `UnloadedTag`,
and `AmbiguousUnloadedTag`, which prove the live availability paths were not
disturbed.

## Done criteria

Machine-checkable. ALL must hold:

- [ ] `rg "Discovered|discovery_knowledge" crates/djls-semantic/src/project/symbols.rs` returns no matches (comments included)
- [ ] `rg "DiscoveredSymbolCandidate|new_discovered|NotInInstalledApps" crates/ docs/` returns no matches
- [ ] `cargo test -q` exits 0
- [ ] `just clippy` exits 0
- [ ] Only in-scope files modified (`jj diff --stat`)
- [ ] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- Step 1 finds a production constructor of `Discovered` status.
- A snapshot diff appears for a template that never used the two deleted
  error codes.
- Removing `has_discovered_library` changes any non-test completion result
  (run `cargo test -q -p djls-ide` before and after Step 3 to compare).
- The serde shape concern bites somewhere unexpected: `TemplateLibraries` is
  serialized in the inspector disk cache (`sync.rs` `CacheEnvelope`).
  Removing the field makes old caches fail to deserialize, which the loader
  handles by returning `false` (cache miss) — that is acceptable. If you find
  any *other* persisted serialization of `TemplateLibraries`, STOP.

## Maintenance notes

- The deleted diagnostics ("tag exists in an app that isn't in
  INSTALLED_APPS") are genuinely useful *ideas* — they died because nothing
  produced their data. Plan 008 derives real library knowledge from source;
  if that work wants these diagnostics back, it should reintroduce them fed
  by extraction facts, not resurrect this plumbing.
- Old `~/.cache/djls/inspector/` caches will silently miss once after this
  lands (shape change). No migration needed; the cache rewrites itself —
  and plan 009 deletes the cache entirely.
