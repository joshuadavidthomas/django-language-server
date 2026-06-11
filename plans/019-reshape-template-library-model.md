# Plan 019: Make the loadable/builtin distinction positional — delete `LibraryStatus`

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
> `crates/djls-semantic/src/project/{settings,symbols}.rs` included in the
> excerpts below. PR #664 may have merged since. Before starting, confirm the
> "Current state" excerpts content-match the live code (line numbers may
> shift; the code shapes must match). Specifically verify:
> `rg -n "enum LibraryStatus" crates/djls-semantic/src/project/symbols.rs`
> returns one match, and
> `rg -n "fn is_active" crates/djls-semantic/src/project/symbols.rs` shows a
> body matching both `Active` and `Builtin`. On mismatch, STOP.

## Status

- **Priority**: P1
- **Effort**: M
- **Risk**: MED (types feed user-visible completions/hover/diagnostics; the
  contract is "zero behavior change", enforced by snapshots and goldens)
- **Depends on**: plans/008 (DONE). Must land **before** plans/015 (which
  moves these files into `djls-project` — move the clean shape, not the
  vestiges). Independent of plans/009.
- **Category**: tech-debt (domain-model correction)
- **Planned at**: jj commit `710f4107`, 2026-06-11
- **Status**: IN PROGRESS — PR open
- **Implemented at**: source commit `9093a28d`, 2026-06-11
- **Bookmark**: `plan-019-reshape-template-library-model`
- **PR**: https://github.com/joshuadavidthomas/django-language-server/pull/666
- **Validation**: Initial implementation passed `cargo build -q`; `cargo test -q`; `cargo insta test --check`; `cargo test -q -j 2 -- --test-threads=2`; `just e2e`; `just clippy --allow-dirty`; `just fmt --check`; `just lint`; stale API guards and no `.snap.new` files. Follow-up review cleanup passed `just fmt`, `cargo test -q -p djls-semantic project::settings`, `cargo test -q -p djls-semantic -p djls-ide -p djls-db`, `cargo clippy --all-targets --all-features --benches -- -D warnings`, and `just fmt --check`.
- **Design rationale**: `plans/memo-template-library-domain-model.md` — read
  it first; this plan implements its "Recommended model" (shape B).

## Why this matters

`LibraryStatus::{Active, Builtin}` models the loadable/preloaded difference as
a *kind* of library, but it is actually a property of where the library is
mounted in the project: a module in `OPTIONS["builtins"]` is an ordinary
template tag library that is available without `{% load %}`. The enum's live
consumers prove it carries no behavior: `is_active()` returns true for every
variant, and the only code that matches on the status is the router that picks
which collection bucket to insert into. Alongside it ride several inspector-era
vestiges: a fabricated, never-read `LibraryName` on builtins, a one-element
`Vec` per loadable name with a three-stage "best" fallback that always picks
element 0, and predicates (`is_enabled_library`, `enabled_loadable_libraries`)
whose conditions are now constant. After this plan the value type is just
module + symbols, the mount is which index it sits in, and every consumer
either shrinks or is untouched.

## Current state

All in `crates/djls-semantic/src/project/symbols.rs` unless noted.

The enum and the value type (symbols.rs:66-83):

```rust
pub enum LibraryStatus {
    Active {
        module: PyModuleName,
        origin: Option<LibraryOrigin>,
    },
    Builtin {
        module: PyModuleName,
    },
}

pub struct TemplateLibrary {
    pub name: LibraryName,
    pub status: LibraryStatus,
    #[serde(default)]
    pub symbols: Vec<TemplateSymbol>,
}
```

The constant-true predicate (symbols.rs:123-129):

```rust
pub fn is_active(&self) -> bool {
    matches!(
        self.status,
        LibraryStatus::Active { .. } | LibraryStatus::Builtin { .. }
    )
}
```

The collection (symbols.rs:174-179) — note the `Vec` multiplicity:

```rust
pub struct TemplateLibraries {
    pub knowledge: StaticKnowledge,
    pub loadable: BTreeMap<LibraryName, Vec<TemplateLibrary>>,
    pub builtins: Vec<TemplateLibrary>,
}
```

`set_loadable` always writes a one-element Vec (symbols.rs:259-261), so
`best_loadable_library`'s fallback chain (symbols.rs:325-334:
`find(is_active)` → `find(origin().is_some())` → `first()`) always selects
element 0. `push_builtin` dedups by module (symbols.rs:263-271).
`enabled_loadable_libraries` (symbols.rs:273-278) filters `loadable_libraries`
by the constant-true `is_active`. `is_enabled_library` (symbols.rs:354-359) is
now "key exists and Vec non-empty".

The status's only real consumer — the bucket router in
`crates/djls-semantic/src/project/settings.rs:182-192`:

```rust
impl TemplateLibraries {
    fn apply_derived(&mut self, derived: DerivedTemplateLibraries) {
        self.knowledge = self.knowledge.weakened_by(derived.knowledge);
        for library in derived.libraries {
            match &library.status {
                LibraryStatus::Active { .. } => self.set_loadable(library),
                LibraryStatus::Builtin { .. } => self.push_builtin(library),
            }
        }
    }
}
```

The fabricated builtin name (settings.rs:258-269, inside
`SettingsLibraryDeclaration::derive`): builtins get
`LibraryName::parse(module.as_str().split('.').next_back().unwrap_or("builtin"))`.
Nothing reads a builtin's name — dedup is by module, candidate origins use
module (`installed_symbol_candidates`, symbols.rs:280-323), hover/completions
display the module.

Dead code: `configured_library` (settings.rs:356-372) has no callers
(superseded by `SettingsLibraryDeclaration::Loadable`).

`LibraryOrigin` (symbols.rs:59-64) is constructed at settings.rs:343-347 and
re-exported (`project.rs:30`, `lib.rs:28`) but consumed by nothing outside the
vestigial `best_loadable_library` fallback — verify with
`rg -n "LibraryOrigin|\.origin\(\)" crates/ --no-heading`: hits only in
symbols.rs, settings.rs, and the two re-export lines.

The projection layer that already has the right model and must NOT change
(symbols.rs:162-172):

```rust
pub enum InstalledSymbolOrigin {
    Builtin { module: PyModuleName },
    Loadable { load_name: LibraryName },
}
```

Repo conventions: imports one-per-line grouped std/external/crate; internal
imports via `crate::<owning_module>::…`; comments explain why only; this crate
uses `thiserror`-style library conventions (see AGENTS.md "Code Style").

## Commands you will need

| Purpose      | Command                          | Expected on success |
|--------------|----------------------------------|---------------------|
| Build        | `cargo build -q`                 | exit 0              |
| Test (all)   | `cargo test -q`                  | exit 0, all pass    |
| Test (crate) | `cargo test -q -p djls-semantic` | exit 0              |
| E2E suite    | `just e2e`                       | exit 0 (runs the golden comparisons) |
| Lint         | `just clippy`                    | exit 0, no warnings |
| Format       | `just fmt`                       | exit 0 (nightly rustfmt — do not run `cargo fmt` directly) |
| Hooks        | `just lint`                      | exit 0              |

## Scope

**In scope** (the only files you should modify):
- `crates/djls-semantic/src/project/symbols.rs`
- `crates/djls-semantic/src/project/settings.rs`
- `crates/djls-semantic/src/project.rs`, `crates/djls-semantic/src/lib.rs`
  (export removals only)
- `crates/djls-semantic/src/scoping/symbols.rs`,
  `crates/djls-semantic/src/scoping/loads.rs`
- `crates/djls-semantic/src/validation/scoping.rs`
- `crates/djls-semantic/src/testing.rs`
- `crates/djls-ide/src/completions.rs`, `crates/djls-ide/src/hover.rs`
- `crates/djls-db/src/db.rs` (test call sites only)
- `crates/djls-bench/src/specs.rs`
- `CONTEXT.md` (one flagged-ambiguity line), `CHANGELOG.md` (internal note —
  follow the repo's changelog conventions / `djls-changelog` skill if available)

**Out of scope** (do NOT touch, even though they look related):
- `crates/djls-project/**` — no extraction API change of any kind.
- `InstalledSymbolOrigin` / `InstalledSymbolCandidate` /
  `installed_symbol_candidates` — already the correct model; consumers in
  hover.rs:144-150 and completions.rs:376-392 branch on it and must keep
  working unchanged.
- `registration_modules()` iteration-order contract (loadables in key order,
  then builtins in stored order) — `templatetag_modules`
  (`project/resolve.rs:227+`) and `compute_filter_arity_specs`
  (`filters.rs:81-94`) depend on it.
- Any diagnostic/completion/hover *behavior*. This plan is representation only.
- `TemplateSymbol` / `SymbolDefinition` field semantics (serde derives may be
  pruned in Step 4, fields stay).

## Version-control workflow

jj repo — never run mutating `git` commands. Work in your isolated backend.
Inspect with `jj st` / `jj diff`. Suggested commits per step group:
`"refactor: collapse vestigial template library multiplicity"`,
`"refactor: make library mount positional, delete LibraryStatus"`,
`"docs: update library availability vocabulary"`. Do NOT push or move
bookmarks.

## Steps

### Step 1: Dead-code pass (zero behavior change)

In `project/settings.rs`: delete `configured_library` (settings.rs:356-372).

In `project/symbols.rs`:
- Change `loadable` to `BTreeMap<LibraryName, TemplateLibrary>`.
- `set_loadable` → `insert_loadable` (plain map insert; later insert wins —
  this preserves the OPTIONS-overrides-app-scan behavior that
  `template_libraries_options_override_app_library_load_name`
  (settings.rs tests) asserts).
- Replace `best_loadable_library`/`best_loadable_library_str` with
  `loadable_library(&LibraryName) -> Option<&TemplateLibrary>` /
  `loadable_library_str(&str)` (map `get`). Update
  `loadable_library_module(_str)` to go through them.
- Delete `is_active`. Delete `enabled_loadable_libraries` and re-point its
  callers to `loadable_libraries()` (which becomes a plain
  `iter()`-over-the-map adapter yielding `(&LibraryName, &TemplateLibrary)`).
- Rename `is_enabled_library` → `is_loadable`, `is_enabled_library_str` →
  `is_loadable_str` (semantics: key membership).
- `registration_modules` (symbols.rs:199-216): swap
  `enabled_loadable_libraries()` for `loadable_libraries()`; output order must
  be unchanged (map key order, then builtins).

Re-point callers mechanically:
- `scoping/symbols.rs:99-105,136-161` (`enabled_loadable_libraries` → renamed
  iterator),
- `scoping/loads.rs:220-222` (`is_enabled_library_str` → `is_loadable_str`),
- `completions.rs:239-249` callers via `completion_library_names` (drop its
  now-constant filter), `completions.rs:626,654`, `hover.rs:49`
  (`best_loadable_library_str` → `loadable_library_str`),
- `djls-db/src/db.rs` tests at ~856-985 (`best_loadable_library_str` →
  `loadable_library_str`),
- `testing.rs` / `specs.rs` fixture builders (Vec wrapper removal),
- `completions.rs` test fixtures (~748-830).

**Verify**: `cargo test -q` → exit 0;
`rg -n "best_loadable_library|is_enabled_library|enabled_loadable_libraries|is_active" crates/` → no matches;
`cargo insta test --check 2>/dev/null || cargo test -q` shows zero snapshot
updates (any `.snap.new` file is a defect).

### Step 2: Delete `LibraryStatus` and `LibraryOrigin`; mount at the declaration site

In `project/symbols.rs`:
- `TemplateLibrary` becomes `{ pub module: PyModuleName, pub symbols: Vec<TemplateSymbol> }`.
  Delete the `name` field (the load name lives only as the `loadable` map
  key), `LibraryStatus`, `LibraryOrigin`, `origin()`, `new_active`,
  `new_builtin`. Add one constructor `TemplateLibrary::new(module)` (empty
  symbols). `module()` becomes a field access or trivial getter.
- `push_builtin` keeps its dedup-by-module.

In `project/settings.rs`, replace the tagged-value routing with mounting at
the site that knows the declaration kind:
- `templatetag_package_libraries` returns
  `(StaticKnowledge, Vec<(LibraryName, TemplateLibrary)>)` — the app-scan
  entries are loadable by construction; the `template_libraries` query inserts
  them via `insert_loadable`.
- Replace `SettingsLibraryDeclaration` + `DerivedTemplateLibraries` +
  `apply_derived` with two functions mirroring the two declaration kinds:
  - `configured_library(resolver, load_name: &str, module_path: &str) -> (StaticKnowledge, Option<(LibraryName, TemplateLibrary)>)`
    (parse failures → `(Partial, None)`, unresolved module → partial with the
    bare library, as `library_with_symbols` does today),
  - `builtin_library(resolver, module_path: &str) -> (StaticKnowledge, Option<TemplateLibrary>)`
    — **no name fabrication**; parse failure of the module path →
    `(Partial, None)`.
- The `template_libraries` query body keeps its existing structure and
  knowledge-weakening logic (settings.rs:119-180): iterate
  `backend.libraries` → `configured_library` → `insert_loadable`; iterate
  `DEFAULT_TEMPLATE_BUILTINS` then `backend.builtins` → `builtin_library` →
  `push_builtin`. Order of the builtin pushes must be byte-for-byte the same
  as today (defaults first, then per-backend OPTIONS, per backend iteration).
- `LibraryOrigin` construction at settings.rs:343-347 disappears with the type.

Remove the `LibraryOrigin` re-exports (`project.rs:30`, `lib.rs:28`).

Update fixture constructors (`testing.rs:140-215`, `specs.rs:160-180`,
completions.rs test fixtures) to the new shape: loadable entries are
`(LibraryName, TemplateLibrary)` insertions; builtin entries are nameless.
While in `testing.rs`, rename the inspector-era `*_json` helper names
(`builtin_tag_json` etc.) to drop the `_json` suffix — the JSON wire shape
they mimicked is gone (mechanical rename, callers in scoping/symbols.rs
tests).

**Verify**: `cargo test -q` → exit 0;
`rg -n "LibraryStatus|LibraryOrigin|new_active|new_builtin" crates/` → no
matches; zero snapshot changes.

### Step 3: Serde audit (verify, then prune)

Run `rg -n "serde_json::from|from_str|from_value" crates/ --no-heading | rg -i "TemplateLibrar"`.
Expected: no matches (the inspector disk cache that deserialized these types
was deleted by plan 008; the golden test deserializes its own
`GoldenTemplateLibraries` type).

- If no matches: remove `Serialize, Deserialize` derives from
  `TemplateLibraries` and `TemplateLibrary` (and the now-orphaned
  `#[serde(default)]` attributes). **Keep** serde on `TemplateSymbolKind` —
  the golden test's `GoldenTemplateSymbol` (settings.rs tests, ~:661-668)
  deserializes it. For `TemplateSymbol` / `SymbolDefinition`: prune only if
  the same `rg` for those names also returns nothing.
- If there are matches: leave all serde derives in place and note the
  consumer in your report.

**Verify**: `cargo build -q` → exit 0; `cargo test -q` → exit 0.

### Step 4: Docs and changelog

- `CONTEXT.md`: the flagged-ambiguity line `"Installed Template Tag Library"
  is ambiguous…; resolved: describe a **Template Tag Library** as discovered,
  active, or builtin.` (CONTEXT.md:329) — update the resolution to the new
  vocabulary: a library is *loadable* (requires `{% load %}`) or *builtin*
  (preloaded); "active"/"discovered" are no longer states of a library.
- `CHANGELOG.md`: internal note (no user-facing behavior change).

**Verify**: `just lint` → exit 0.

### Step 5: Full validation

**Verify**: `cargo test -q`, `just clippy`, `just fmt --check`, `just lint` →
all exit 0. `just e2e` → exit 0 **including** the two golden-fixture
comparisons (`django_facts_golden_template_libraries_match`,
`django_facts_golden_template_dirs_match` — these are `#[ignore]`d cargo tests
run by the e2e session with the Django venv).

## Test plan

No new behavior, so no new tests — existing coverage is the contract:

- The settings.rs unit tests (app discovery, OPTIONS libraries/builtins,
  override precedence, knowledge demotion — ~14 tests) must pass with only
  mechanical assertion updates (`best_loadable_library_str` →
  `loadable_library_str`; no assertion *values* change).
- `registration_modules_keep_deterministic_precedence_order` and
  `builtin_candidates_keep_last_builtin_symbol` (symbols.rs tests) are the
  order-contract guards — their expected values must not change.
- The golden-fixture e2e comparison is the end-to-end parity gate.

## Done criteria

Machine-checkable. ALL must hold:

- [ ] `rg -n "LibraryStatus|LibraryOrigin|new_builtin|new_active|is_active|best_loadable|is_enabled_library|enabled_loadable" crates/` returns no matches
- [ ] `loadable` field type is `BTreeMap<LibraryName, TemplateLibrary>` (no `Vec`)
- [ ] `TemplateLibrary` has exactly `module` and `symbols` fields
- [ ] `cargo test -q` exits 0; zero insta snapshot changes (`rg --files -g '*.snap.new' crates/` → nothing)
- [ ] `just e2e` exits 0 including the golden comparisons
- [ ] `just clippy` and `just fmt --check` exit 0
- [ ] Only in-scope files modified (`jj diff --stat`)
- [ ] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- The drift check fails (PR #664 follow-ups reshaped these types again).
- Any insta snapshot or golden-fixture comparison differs — this plan must be
  behavior-inert; a diff means the mounting rework changed precedence or
  inventory. Report the exact delta; do not regenerate snapshots or goldens.
- Step 3's `rg` finds a live deserializer of `TemplateLibraries`/
  `TemplateLibrary` — report it; do not redesign serialization.
- You find a real (non-test) consumer that branches on `LibraryStatus` or
  reads `LibraryOrigin`/a builtin's `name` — the memo's zero-consumer claim
  would be wrong; report the call site.
- The change appears to require touching `crates/djls-project` or
  `InstalledSymbolOrigin`.

## Maintenance notes

- Plan 015 moves `project/symbols.rs` and `project/settings.rs` into
  `djls-project`; its move table is unaffected, but its drift check should be
  re-anchored after this lands.
- Plan 018 (not-in-INSTALLED_APPS diagnostics) is where *provenance* returns
  if needed: this plan deletes `LibraryOrigin` because nothing consumes it;
  reintroduce app attribution there, shaped by the actual diagnostic, rather
  than resurrecting the old struct.
- Reviewers: scrutinize the builtin push order in Step 2 (it is the symbol
  precedence contract) and the `insert_loadable` later-wins semantics (it is
  the OPTIONS-overrides-app-scan contract).
- Design rationale and the full vestige inventory:
  `plans/memo-template-library-domain-model.md`.
