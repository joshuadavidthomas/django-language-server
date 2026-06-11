# Plan 008: Derive template tag libraries from source and gate diagnostics on partial knowledge

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: Plans 002, 006, and 007 are prerequisites and
> have reshaped these files. Before starting verify: `django_settings`
> and `template_dirs` queries exist in
> `crates/djls-semantic/src/project/settings.rs` (plan 007);
> `discovery_knowledge` no longer exists anywhere (plan 002);
> `djls_project::StaticKnowledge` has a `Partial` variant (exported by
> plan 007). If any check fails, STOP.

## Status

- **Priority**: P1
- **Effort**: L
- **Risk**: HIGH (user-visible diagnostics depend on this data)
- **Depends on**: plans/002, plans/006, plans/007, plans/013 (registry seam), plans/014 (fixtures + goldens)
- **Category**: direction (static Django discovery)
- **Planned at**: commit `922cc4d7`, 2026-06-10
- **Status**: DONE
- **Implemented at**: source commits `097bbb0e`, `ba7b106a`, 2026-06-11
- **Bookmark**: `plan-008-derive-template-libraries-from-source`
- **PR**: https://github.com/joshuadavidthomas/django-language-server/pull/664

## Why this matters

Template tag library knowledge — which `{% load %}` names exist, which
modules they map to, which builtins are active, what symbols they export —
is the last fact still produced by the runtime inspector. This plan derives
it statically: `INSTALLED_APPS` (from plan 007's Django settings) → app
modules → `templatetags/` packages → per-module symbol extraction (the
Ruff-AST registry analysis djls-semantic already has). With it lands the one
genuinely behavior-bearing piece of "confidence": when extraction is
`Partial`, the validator keeps positive diagnostics ("this library exists
but isn't loaded") and suppresses absence claims ("unknown tag") — because
an incomplete library list cannot prove absence. After this plan the
inspector feeds nothing and plan 009 deletes it.

## Current state

(Line numbers from the planned-at SHA `922cc4d7`; content-match after the
prerequisite plans' churn.)

- `crates/djls-semantic/src/project/input.rs:188-196` — the input field to
  delete (`template_libraries: TemplateLibraries`).
- `crates/djls-semantic/src/project/sync.rs:145-211` — the inspector pull +
  cache write + apply (`TemplateLibrarySnapshotRequest`,
  `refresh_template_libraries`, `apply_template_library_snapshot`), plus the
  phase-1 cache loader `load_template_library_cache` (`sync.rs:54-93`) and
  the cache key/dir helpers (`sync.rs:219-255`). All deleted here (the
  inspector *process* machinery is plan 009).
- `crates/djls-server/src/server.rs:190-241` — `initialized` phase 1 loads
  the cache (`:196-210`); phase 2 calls `refresh_external_data`. Phase 1
  disappears with the cache (static derivation needs no disk cache — Salsa
  recomputes from source in milliseconds at startup).
- `crates/djls-semantic/src/project/resolve.rs:227+` — `templatetag_modules`
  reads `project.template_libraries(db).registration_modules()` — **this is
  the dependency arrow that inverts**: today extraction targets come from
  the inspector; after this plan the library list itself comes from
  extraction.
- Symbol extraction already exists:
  `crates/djls-semantic/src/python/registry.rs` (736 lines) walks a parsed
  module for `register = template.Library()` +
  `@register.tag/simple_tag/filter/...` registrations. The ruff-side model
  for that one-hop resolution is
  `reference/ruff/crates/ruff_python_semantic/src/analyze/typing.rs:1168-1199`
  (`resolve_assignment`: name → binding → call RHS → qualified callee).
- Builtins today come from the inspector snapshot
  (`TemplateLibrarySnapshot.builtins`). Statically: Django's default
  builtins are stable (`django.template.defaulttags`,
  `django.template.defaultfilters`, `django.template.loader_tags`) plus
  per-backend `OPTIONS["builtins"]` additions from `DjangoSettings`.
- Availability gating (`crates/djls-semantic/src/validation/scoping.rs`
  after plan 002): `check_tag_scoping_rule` early-returns unless
  `active_knowledge == StaticKnowledge::Known`, then emits `UnknownTag` /
  `UnloadedTag` / `AmbiguousUnloadedTag`; `check_filter_scoping_rule`
  mirrors it; `check_load_libraries_rule` (`scoping.rs:134+`) flags unknown
  `{% load %}` names.
- `TemplateLibraries` (`project/symbols.rs:199-206` post-plan-002):
  `{ active_knowledge, loadable: BTreeMap<LibraryName, Vec<TemplateLibrary>>, builtins, builtin_order }` —
  this type **stays** as the assembled output shape; only its producer
  changes.

## Commands you will need

| Purpose      | Command                          | Expected on success |
|--------------|----------------------------------|---------------------|
| Build        | `cargo build -q`                 | exit 0              |
| Test (all)   | `cargo test -q`                  | exit 0, all pass    |
| Rust matrix  | `just test`                      | exit 0 (cargo tests via nox — does NOT run tests/e2e) |
| E2E suite    | `just e2e` (or `nox -s e2e`)     | exit 0 — THE parity gate; pytest-lsp over tests/project |
| Lint         | `just clippy`                    | exit 0, no warnings |
| Format       | `just fmt`                       | exit 0              |
| Hooks        | `just lint`                      | exit 0              |

## Scope

**In scope** (the only files you should modify):
- `crates/djls-semantic/src/project/{settings,resolve,symbols,sync,input}.rs`
- `crates/djls-semantic/src/project.rs`, `src/lib.rs`, `src/db.rs`
- `crates/djls-semantic/src/validation/scoping.rs` (Partial gating)
- `crates/djls-db/src/db.rs` (SemanticDb impl + invalidation tests)
- `crates/djls-server/src/server.rs` (remove phase-1 cache load)
- `crates/djls-bench/src/specs.rs` (`realistic_db` builds libraries via
  `apply_active_snapshot` at specs.rs:161,182,196 — migrate alongside
  Step 4's deletions or bench stops compiling)
- `crates/djls-semantic/src/testing.rs` and affected tests in
  djls-semantic / djls-db / djls-ide
- `CHANGELOG.md` (user-facing change: no more disk cache; follow the
  repo's changelog conventions)

**Out of scope** (do NOT touch, even though they look related):
- `introspector.rs`, `inspector/` Python package, `build.rs`,
  `ProjectDb::project_introspector` — plan 009 deletes them once nothing
  calls them.
- `compute_tag_specs` / tag-spec extraction — it already derives from
  `templatetag_modules` and keeps working once that query's input switches.
- Django-version-aware builtins detection — v1 ships the stable default
  list + OPTIONS additions (see Maintenance notes).

## Git workflow

jj repo — no mutating `git`. Commit per step group. Suggested messages:
`"add static template library derivation"`,
`"refactor: gate availability diagnostics on partial knowledge"`,
`"refactor: drop inspector template library refresh and cache"`. Do NOT push.

## Steps

### Step 1: Derive app templatetags libraries

In `project/settings.rs` (or a sibling `project/libraries.rs` if it reads
better — `folder.rs` convention):

```rust
#[salsa::tracked(returns(ref))]
pub(crate) fn template_libraries(db: &dyn ProjectDb, project: Project) -> TemplateLibraries
```

Composition (mirror Django's
`django.template.backends.django.get_template_tag_modules`, which scans
`django.templatetags` FIRST and then each installed app's
`<app>.templatetags`):
1. **Django's own libraries**: scan `django.templatetags` exactly like an
   app package — resolve the `django` package on the search paths
   (site-packages) and walk `django/templatetags/*.py`. This is where
   `static`, `i18n`, `l10n`, `tz`, and `cache` come from; `django` itself
   is NOT in `INSTALLED_APPS`. The e2e suite hard-requires this:
   `tests/e2e/test_completions.py:48-97` asserts `static` and `i18n`
   load/complete, and `test_diagnostics.py:12` asserts unloaded-`static`
   diagnostics. Omitting this scan fails the parity gate.
2. **App libraries**: for each `installed_apps` entry (plan 007's
   `DjangoSettings`, resolved to app package dirs the same way
   `template_dirs` resolves APP_DIRS apps): if
   `<app_dir>/templatetags/__init__.py` exists, every
   non-underscore `*.py` in it is a candidate library whose load name is the
   file stem and module is `<app>.templatetags.<stem>`. Parse each candidate
   (existing tracked parse) and include it only if registry analysis finds a
   `Library` registration — plan 013 exposed the seam:
   `crate::python::collect_registrations_from_body` /
   `RegistrationInfo` / `RegistrationKind` (pure, db-free; pair with the
   cached `parse_python_module(db, File)` like `extract_tag_rules` does at
   `python.rs:187-198`). Do not re-implement.
3. **OPTIONS libraries**: per-backend `OPTIONS["libraries"]` name→module
   pairs from `DjangoSettings` (resolve module → file via search paths;
   unresolvable → include the library with no symbols + demote knowledge to
   Partial).
4. **Builtins**: the Django default builtin modules (constant list:
   `django.template.defaulttags`, `django.template.defaultfilters`,
   `django.template.loader_tags`) followed by per-backend
   `OPTIONS["builtins"]`, preserving order (precedence was fixed by #652 —
   keep registration order).
5. **Symbols**: for every included library/builtin module file, extract
   symbols via the registry analysis; build `TemplateLibrary` values with
   `new_active`/`new_builtin` and real `SymbolDefinition::Exact { file }`
   definitions where the file is known. **Known degradation**: the registry
   analysis extracts no docstrings (`RegistrationInfo` has no `doc` field),
   so `TemplateSymbol.doc` becomes `None` where the inspector supplied
   docs — hover output loses doc text. Record this in the changelog entry.
   (Optional small follow-up inside this plan if cheap: read the
   `StmtFunctionDef` docstring during registration collection — it is in
   hand at extraction time.)
6. `active_knowledge`: `Known` when `installed_apps.knowledge == Known` and
   all module resolutions succeeded; `Partial` when installed_apps is
   Partial or any resolution failed; `Unknown` when installed_apps is
   Unknown or there is no settings module.

Depend on search-path root revisions (the `model_modules` pattern,
`resolve.rs:155-163`) so refreshes/watchers re-run discovery.

**Verify**: `TestDatabase` + `InMemoryFileSystem` fixture: an app with
`templatetags/custom.py` containing `register = template.Library()` and one
`@register.simple_tag` → library `custom` with one tag symbol, knowledge
Known. A Partial installed_apps → `active_knowledge == Partial`.

### Step 2: Invert the templatetag_modules arrow

`templatetag_modules` (`resolve.rs:227+`) currently reads
`project.template_libraries(db).registration_modules()`. Point it at the
Step 1 query's result instead (same `registration_modules()` shape on the
returned `TemplateLibraries`). `compute_tag_specs` and filter-arity
derivation now flow entirely from source.

**Verify**: `cargo test -q -p djls-semantic` — tag-spec extraction tests
pass; the `djls-db` invalidation test
`template_libraries_change_validates_templatetag_module_projection`
(db.rs:297-330) will need rewriting in Step 4 (it sets the input directly).

### Step 3: Partial-knowledge gating in scoping

In `validation/scoping.rs`, change the early returns
(`if active_knowledge != StaticKnowledge::Known { return; }` at the top of
`check_tag_scoping_rule` / `check_filter_scoping_rule` / the equivalent in
`check_load_libraries_rule`) to three-way policy:

- `Unknown` → return (today's behavior: claim nothing).
- `Known` → today's behavior (all diagnostics).
- `Partial` → **suppress absence claims, keep positive claims**:
  - `TagAvailability::Unknown` / `FilterAvailability::Unknown` → emit
    nothing (we cannot prove the tag doesn't exist).
  - `Unloaded` / `AmbiguousUnloaded` → emit (the library is positively
    known; "you forgot to load it" is evidence-backed).
  - `check_load_libraries_rule`'s unknown-`{% load %}`-name diagnostic →
    suppress under Partial (absence claim).

**Verify**: new scoping tests (model after existing ones in
`validation/scoping.rs`'s test module / `testing.rs` fixtures): with
`active_knowledge = Partial`, an unknown tag yields no diagnostic, an
unloaded-but-known tag still yields `UnloadedTag`.

### Step 4: Delete the input field, the cache, and phase-1

- `input.rs`: remove the `template_libraries` field + builder arg; update
  `Project::builder` call sites.
- `sync.rs`: delete `refresh_template_libraries`,
  `apply_template_library_snapshot`, `load_template_library_cache`,
  `TemplateLibrarySnapshotRequest`, `CacheEnvelope`, `cache_key`,
  `cache_dir`, their tests, and the `sha2`/`serde_json` imports if now
  unused (check `Cargo.toml` for droppable deps).
- `project.rs`/`lib.rs`: drop `load_template_library_cache` re-exports
  (`project.rs:38`); keep `TemplateLibraries` exports (still the public
  shape). `TemplateLibrarySnapshot` and `apply_active_snapshot`: delete;
  the verified consumer inventory to migrate:
  `djls-semantic/src/{testing.rs,project/sync.rs,project/symbols.rs,project/resolve.rs,project.rs,lib.rs}`,
  `djls-db/src/db.rs`, and `djls-bench/src/specs.rs:161,182,196`
  (`realistic_db`). Migrate fixtures to construct `TemplateLibraries`
  directly or through the new derivation.
- `crates/djls-db/src/db.rs`: `SemanticDb::template_libraries()` impl
  routes to the Step 1 query (`compute_tag_specs`-style: query returns
  `&TemplateLibraries` via `returns(ref)`, db.rs:136-144 is the pattern).
- `crates/djls-server/src/server.rs:193-241`: remove phase 1 entirely; keep
  phase 2 (`refresh_external_data` still bumps roots). Simplify the
  `cache_loaded` branching — `initialized` now always awaits the (fast)
  background refresh or returns immediately; preserve the existing
  "don't block the editor" structure. **Preserve the log line**
  `"Server initialization completed"` (`server.rs:230`) —
  `tests/e2e/test_initialized.py:23-35` polls for it; dropping it hangs
  e2e to timeout.
- Rewrite the `djls-db` invalidation tests that called
  `set_template_libraries` (db.rs:297-330, 572-595) to mutate the
  *sources* (settings file / templatetag file revisions) and assert the
  derived queries re-run.
- `CHANGELOG.md`: note the removals (inspector cache, instant-cache phase)
  per the repo's changelog style.

**Verify**: `cargo build -q` → exit 0;
`rg "set_template_libraries|load_template_library_cache|inspector.json" crates/` → no matches;
`cargo test -q` → all pass.

### Step 5: E2E parity

Run `just e2e` (NOT `just test`, which runs only cargo tests). The fixture
project (`tests/project`) must produce the same completions/diagnostics as
the inspector did: same loadable library names (including plan 014's
first-party `djls_app_tags` and Django's `static`/`i18n`), same builtin
tags valid without `{% load %}`, same unloaded-tag suggestions.

Additionally, compare the derived `TemplateLibraries` against the golden
fixture `tests/fixtures/django-facts/django-5.2.json` (captured by plan 014
from the runtime inspector; paths normalized with `${PROJECT}` /
`${SITE_PACKAGES}` placeholders): library load-names, module mapping,
builtin order, and symbol names must match. Write this as a test —
preferred shape: a `#[ignore]`d cargo test in djls-semantic that reads the
golden JSON + runs derivation over `tests/project` via `OsFileSystem`,
executed by the nox e2e session (which has the venv) via
`cargo test -p djls-semantic -- --ignored`; if wiring that into nox is
awkward, a pytest e2e comparing against golden-derived expectations is
acceptable. Record which you chose.

**Verify**: `just e2e` → exit 0 including the golden comparison.
Differences → STOP (report the exact library/symbol delta; do not adjust
expectations or regenerate goldens unilaterally).

## Test plan

- Step 1 fixture tests (app discovery, OPTIONS libraries, builtins order,
  symbol extraction, knowledge propagation).
- Step 3 Partial-gating tests (the new behavior — at least 4 cases: unknown
  tag suppressed, unloaded kept, ambiguous-unloaded kept, unknown `{% load %}`
  suppressed).
- Step 4 rewritten invalidation tests: editing a templatetag module file
  re-runs symbol extraction for that module only (event-log pattern);
  editing `settings.py` re-runs library derivation.
- E2E matrix unchanged.

## Done criteria

Machine-checkable. ALL must hold:

- [ ] `rg "refresh_template_libraries|load_template_library_cache|CacheEnvelope" crates/` returns no matches
- [ ] `Project` input no longer has `template_libraries` (field count down by one)
- [ ] `rg "project_introspector\(\)\.query" crates/` returns no matches (the inspector has no remaining callers — precondition for plan 009)
- [ ] `cargo test -q` exits 0, including ≥ 4 new Partial-gating tests
- [ ] `just test` exits 0 AND `just e2e` exits 0 (including the golden-fixture comparison)
- [ ] `just clippy` exits 0
- [ ] Only in-scope files modified (`jj diff --stat`)
- [ ] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- E2E shows missing or extra libraries/symbols vs. the inspector era —
  report the delta (this is the fidelity gate for the whole static
  approach; a human decides whether it's an extraction gap to fix or an
  acceptable difference).
- (Largely retired by plan 013, which verified and exposed the seam:
  `collect_registrations_from_body(&[Stmt])` is pure and db-free.) Only if
  the seam still doesn't fit — report the actual signatures rather than
  restructuring registry.rs.
- Startup latency visibly regresses without the disk cache (measure: the
  `initialized` log line timing on `tests/project`) — report numbers; do
  not re-add a cache on your own.
- You need the inspector for anything — that's the inverted arrow again.

## Maintenance notes

- Builtins are a stable constant here; if a future Django version changes
  default builtins, the fix is version detection (read
  `django/__init__.py` `VERSION` from site-packages — a 20-line recognizer
  in djls-project's extraction module) feeding a version-keyed table.
  Deliberately deferred.
- PR #606's `python/registry.rs` diff contains an `is_register_object`
  precision fix worth comparing while touching this area:
  `jj file show -r static-project-model-consolidated-settings-facts crates/djls-semantic/src/python/registry.rs`.
- Reviewers: scrutinize Step 3's policy table — it is the user-visible
  contract for what the server claims under partial knowledge, and the
  difference between "useful when imperfect" and "wrong".
- After this lands, plan 009 is pure deletion; do it promptly so the dead
  inspector doesn't linger.
