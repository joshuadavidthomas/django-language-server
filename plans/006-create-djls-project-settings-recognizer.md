# Plan 006: Create `djls-project` with the bounded settings recognizer

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat 95e30371..HEAD -- Cargo.toml crates/djls-semantic/src/project.rs`
> (This plan mostly creates a new crate; drift only matters for the workspace
> manifest and for plan 001 having landed — `crates/djls-semantic/src/project/static_model.rs`
> must NOT exist. If it still exists, STOP: plan 001 is a prerequisite.)

## Status

- **Priority**: P1
- **Effort**: L
- **Risk**: LOW (new crate, nothing wired in yet)
- **Depends on**: plans/001-delete-static-scaffolding.md
- **Category**: direction (static Django discovery)
- **Planned at**: commit `922cc4d7`, 2026-06-10; retargeted from a separate
  `djls-extraction` crate to `djls-project` at `95e30371`, 2026-06-10
  (crate-count review — see `plans/README.md` reconciliation log)
- **Status**: DONE
- **Implemented at**: source commit `c6bd8ac2`, 2026-06-10
- **Amended at**: source commit `c6bd8ac2`, 2026-06-10 — renamed extraction outputs per `plans/memo-pr659-extraction-vocabulary.md` (`DjangoSettings`, `StringListSetting`, `TemplateBackend`), converted `Reason` to a closed enum, renamed the implementation module/type to `extractor`/`SettingsExtractor`, and kept the crate public surface empty until plan 007 introduces the first external consumer.
- **Bookmark**: `plan-006-djls-project-settings-recognizer`
- **PR**: https://github.com/joshuadavidthomas/django-language-server/pull/659
- **Merged as**: `cf89c96f add djls-project settings extractor (#659)`, 2026-06-11

## Why this matters

The goal of the whole plan series: resolve Django project facts —
`INSTALLED_APPS`, `TEMPLATES` (dirs, APP_DIRS, OPTIONS libraries/builtins) —
from Python source without running Python. Two prior attempts (PRs #606,
#626) proved feasibility but drowned in unbounded evaluation. The reference
projects show the right size: ty extracts `__all__` — the same problem shape
as `INSTALLED_APPS` (a module-level list of strings built by assignment,
`+=`, `.append()`/`.extend()`, cross-module composition, and conditional
branches) — in one 444-line bounded collector
(`reference/ruff/crates/ty_python_semantic/src/dunder_all.rs`) with an
`invalid` latch instead of guessing. ty's own philosophy comment
(`reference/ruff/crates/ty_module_resolver/src/resolve.rs:1515-1521`):
"This is all syntax-only analysis so it *could* be fooled but it's really
unlikely… this is better than nothing!" This plan builds that extractor for
Django settings — AST in, settings out, no Salsa, no I/O — so plan 007 can
wire it into tracked queries.

**Where it lives**: this plan creates the `djls-project` crate and the
extractor becomes its `extraction` module. `djls-project` is the future home
of the whole mechanical project model (plan 015 moves it in post-009); the
extractor is that model's input adapter, so it lives with its consumer — the
same colocation ty uses (`dunder_all.rs` sits inside `ty_python_semantic`,
its consumer, not in a separate extraction crate). Until plan 015 lands, the crate
contains *only* the extraction module, so its manifest has no `salsa` and
no `djls-source` — the purity contract below is crate-level-checkable for
the entire static track. Plan 015 brings the project model (and salsa) into
the crate and converts the purity check to a module-scoped one.

## Current state

- The workspace already pins the parser the extractor needs
  (root `Cargo.toml:60-61`, re-verified at `95e30371`):

  ```toml
  ruff_python_ast = { git = "https://github.com/astral-sh/ruff.git", rev = "ce5f7b6127a5d684e96fd0f8e387f73c41c7a1b0" }  # 0.15.0
  ruff_python_parser = { git = "https://github.com/astral-sh/ruff.git", rev = "ce5f7b6127a5d684e96fd0f8e387f73c41c7a1b0" }  # 0.15.0
  ```

- `crates/djls-semantic/src/python/` already consumes the ruff AST for
  templatetag extraction — read the `djls-ruff-ast` notes/skill if available
  in your environment for known AST-shape gotchas (boxed expressions,
  f-strings, parameters).

- `djls-semantic`'s `Knowledge` enum (`project/symbols.rs:169-172`) is
  `{ Known, Unknown }` — no `Partial`. This crate introduces the canonical
  three-state version; plan 007 migrates semantic onto it.

- No `crates/djls-project` exists. Plan 001 has deleted the conflicting
  `Fact<T>`/`ImportRoot`/`ResolvedModule` vocabulary (verify in the drift
  check).

### Reference implementations to model (read these before coding)

All under `reference/ruff/` in this repo:

| Technique | Where | What to copy |
|---|---|---|
| Invalid-latch extractor | `crates/ty_python_semantic/src/dunder_all.rs:26-66, 199-209` | per-name latch: unrecognized idiom → that setting is Unknown with a closed `Reason`, never a guess |
| `.append`/`.extend`/`.remove` handling | `dunder_all.rs:106-148` | single-positional-arg call idioms on the watched name |
| Re-assignment clears prior state | `dunder_all.rs:57-66` (`update_origin`) | `INSTALLED_APPS = [...]` after a previous assignment replaces, not extends |
| Statically evaluated `if`/`elif`/`else` | `dunder_all.rs:364-390` | walk only the live arm when truthiness is decidable |
| `+`-chain folding | `crates/ruff_python_semantic/src/model/all.rs:109-134` | linearize left-leaning BinOp chains, extract elements per operand |
| Partial-with-flags (don't whole-bail) | `model/all.rs` (`DunderAllFlags`) | skip a bad element, record why, keep extracting |
| Literal-int tuple comparison | `crates/ruff_linter/src/rules/pyupgrade/rules/outdated_version_block.rs:439-452` | the shape of a bounded comparator (only if you implement version checks — optional) |

## Commands you will need

| Purpose      | Command                          | Expected on success |
|--------------|----------------------------------|---------------------|
| Build        | `cargo build -q -p djls-project` | exit 0              |
| Test (crate) | `cargo test -q -p djls-project`  | exit 0, all pass    |
| Test (all)   | `cargo test -q`                  | exit 0, all pass    |
| Lint         | `just clippy`                    | exit 0, no warnings |
| Format       | `just fmt`                       | exit 0              |
| Hooks        | `just lint`                      | exit 0              |

## Suggested executor toolkit

- The `djls-workspace-conventions` skill (if available): crate manifest
  layout, workspace dependency grouping, new-crate setup.
- The `djls-ruff-ast` skill (if available): pinned-rev AST shape gotchas.

## Scope

**In scope** (the only files you should modify/create):
- `crates/djls-project/` (create everything)
- Root `Cargo.toml` (workspace member)

**Out of scope** (do NOT touch, even though they look related):
- `crates/djls-semantic/` — wiring happens in plan 007. This plan must land
  with zero behavior change to the language server.
- The project model itself (inputs, search paths, module resolution) — it
  stays in `djls-semantic/src/project/` until plan 015 moves it here. This
  plan creates the crate with the extraction module only.
- File reading, module resolution against search paths, Salsa — the extractor
  is pure by design; the caller supplies source text and answers star-import
  questions through a callback.
- A general Python evaluator. Explicitly rejected: list `.sort()`/
  `.reverse()` emulation, importing path-returning *functions* from sibling
  modules, dict/set algebra beyond the TEMPLATES shapes below, arbitrary
  function-call evaluation. (These are where PR #606 went off the rails;
  neither ruff nor ty models any of them.)

## Git workflow

jj repo — no mutating `git`. Commit per step or logical unit, message style
matching the repo log (e.g. `"add djls-project settings extractor"`,
`"test: cover star-import layering"`). Do NOT push.

## Steps

### Step 1: Scaffold

`crates/djls-project/Cargo.toml` modeled on `crates/djls-conf/Cargo.toml`
(version `0.0.0`, workspace lints/edition). Dependencies (all
`workspace = true`): `camino`, `ruff_python_ast`, `ruff_python_parser`,
`rustc-hash`, `serde` only if an existing sibling justifies it (default: no).
**No** `salsa`, **no** `djls-source`, **no** `djls-semantic` — those arrive
with the project model in plan 015, not before.

`src/lib.rs` keeps the extraction module private until plan 007 introduces
the first external consumer:

```rust
// Plan 006 lands the extractor before Plan 007 wires it into djls-semantic.
#[allow(dead_code)]
mod extraction;
```

Do not use `pub mod` here and do not add optimistic facade exports. The
extractor is tested in-crate only in this plan. Plan 007 adds explicit
`pub use` items in `lib.rs` for the symbols it actually consumes.

`src/extraction.rs` is the private module façade (repo rule: `folder.rs`, not
`folder/mod.rs`); it declares the submodules from Steps 2–3 and does not
re-export the boundary API yet.

**Verify**: `cargo build -q -p djls-project` → exit 0.

### Step 2: Define the boundary types (`src/extraction/settings.rs`)

```rust
/// How much to trust an extracted value. The ONLY confidence vocabulary in
/// this codebase (plan 007 migrates djls-semantic onto it).
pub enum Knowledge { Known, Partial, Unknown }

/// Why an extracted value is Partial/Unknown. Keep this closed; render
/// messages through `Display` instead of storing stringly reasons.
pub enum Reason {
    SyntaxErrors,
    UnresolvedStarImport,
    UnsupportedAssignment,
    UnsupportedMutation,
    NonLiteralElement,
    NonLiteralKey,
    UnsupportedValue,
    DictUnpack,
    AmbiguousCondition,
    UnsupportedPathExpression,
}

/// A best-effort string list setting (INSTALLED_APPS shape).
pub struct StringListSetting {
    pub values: Vec<String>,
    pub knowledge: Knowledge,
    pub reasons: Vec<Reason>,
}

/// One entry of TEMPLATES.
pub struct TemplateBackend {
    pub backend: Option<String>,            // BACKEND string literal
    pub dirs: Vec<PathValue>,               // DIRS entries
    pub app_dirs: Option<bool>,             // APP_DIRS literal bool
    pub libraries: Vec<(String, String)>,   // OPTIONS["libraries"]: name -> module
    pub builtins: Vec<String>,              // OPTIONS["builtins"]
    pub knowledge: Knowledge,
    pub reasons: Vec<Reason>,
}

/// The statically extracted subset of a Django settings module.
pub struct DjangoSettings {
    pub installed_apps: StringListSetting,
    pub template_backends: Vec<TemplateBackend>,
    pub templates_knowledge: Knowledge,
}

/// A path expression evaluated against the settings file's own location.
pub enum PathValue { Resolved(camino::Utf8PathBuf), Unknown(Reason) }

/// `from X import *` — the caller resolves and recurses.
pub struct StarImport { pub level: u32, pub module: Option<String> }

/// Caller-supplied recursion for star imports (plan 007 implements this with
/// a Salsa query; tests implement it with a HashMap of fixture sources).
pub trait StarImportResolver {
    /// Return the already-extracted env of the referenced module, or None if
    /// it cannot be resolved (the affected names degrade to Partial).
    fn resolve(&mut self, import: &StarImport) -> Option<SettingsEnv>;
}
```

`SettingsEnv` is the extractor's working state (watched-name bindings + aux
constants like `BASE_DIR`); keep it available inside the crate with an
`into_settings()` finisher. Internal representation is yours, but the
Plan 007-facing API shape above is fixed.

**Verify**: `cargo build -q -p djls-project` → exit 0.

### Step 3: The extractor (`src/extraction/extractor.rs`)

Extractor entry point (kept private to the crate until Plan 007 exports it):

```rust
pub fn extract_settings(
    source: &str,
    module_path: &camino::Utf8Path,     // for resolving Path(__file__) and relative dirs
    resolver: &mut dyn StarImportResolver,
) -> DjangoSettings
```

Parse with `ruff_python_parser::parse_module`; at the pinned Ruff revision,
`parse_module` exposes no recovered AST on error. On syntax errors, return
Partial settings without panicking.

Walk **module-level statements only** — never descend into function or class
bodies (dunder_all does the same). DO walk into `if`/`for`/`while`/`with`/
`try` bodies (`dunder_all.rs:392-398` walks these).

Supported shapes per watched list name (`INSTALLED_APPS`; design the extractor
table-driven so adding `MIDDLEWARE` later is one entry):

1. `NAME = [...]` / `NAME = (...)` / annotated — replaces prior state
   (the `update_origin` rule, `dunder_all.rs:57-66`)
2. `NAME += [...]` and `NAME = NAME + [...] + OTHER` — fold `+`-chains
   (`model/all.rs:109-134`); a non-list operand that is another watched
   name splices its current value; anything else → Partial + reason
3. `NAME.append(x)` / `.extend([...])` / `.insert(i, x)` / `.remove(x)` —
   single recognized mutation calls (`dunder_all.rs:106-148`)
4. Elements must be string literals (implicit concatenation comes folded for
   free via `StringLiteralValue::to_str()`). A non-literal element (e.g.
   `env("EXTRA_APP")`) → **skip the element, add a Reason, demote to
   Partial** — per-element skip like ruff's `DunderAllFlags`, NOT ty's
   whole-query bail. Justification: settings lists with one env-driven entry
   are common; a Partial list is far more useful than no list. Document this
   deviation in the extractor module doc.
5. Any other statement that *writes* a watched name (subscript store, star
   target, `for NAME in ...`, etc.) → that name is `Unknown` + reason
   (per-name latch, not per-module).

`TEMPLATES` recognizer (v1 bounds — document them):
- literal assignment: list of dict literals; recognized keys `"BACKEND"`,
  `"DIRS"` (list of path expressions), `"APP_DIRS"` (bool literal),
  `"OPTIONS"` (dict literal with `"libraries"` dict-of-string-literals and
  `"builtins"` list-of-string-literals). Unrecognized keys are ignored;
  unrecognized *values* for recognized keys → Partial + reason.
- mutations: `TEMPLATES[<int literal>]["DIRS"].append/extend(...)` and
  `+=` on that subscript path. Anything else touching `TEMPLATES` → Unknown.

Branch handling (`if`/`elif`/`else`):
- Implement `evaluate_test_expr` returning
  `Truthiness { AlwaysTrue, AlwaysFalse, Ambiguous }` for: `True`/`False`
  literals, `not <decidable>`, and names whose watched aux value is a bool
  literal (covers `DEBUG = True` earlier in the same file). Everything else
  (env reads, imports, comparisons) → `Ambiguous`.
- `AlwaysTrue`/`AlwaysFalse` → walk only the live arm (the exact arm-walk
  logic at `dunder_all.rs:364-390` — port it).
- `Ambiguous` → walk **all** arms in source order and demote every watched
  name written in any arm to `Partial` with a "condition not statically
  decidable" reason. (Policy: union-ish and honest — unknown ≠ false.)

Star imports: on `from X import *` at module level, call
`resolver.resolve(...)`; on `Some(env)`, merge it as the new base for watched
names (later statements in this module then override/mutate — ordered-walk
shadowing). On `None`, demote all watched names to Partial with a reason.

Path micro-evaluator (`src/extraction/paths.rs`) — closed grammar, evaluated
relative to `module_path`; everything else `PathValue::Unknown`:
- `Path(__file__).resolve().parent` chains (each `.parent` pops one segment)
- aux-name references (`BASE_DIR`) whose value was set by a recognized path
  expression earlier in the walk
- `<path expr> / "literal"` (BinOp Div), `.joinpath("literal")`
- `os.path.join(<path expr>, "literal", ...)`, `os.path.dirname(<path expr>)`
- `str(<path expr>)`
Note: ty does *no* path-value evaluation at all (`Path.__truediv__` is just a
type to ty) — this evaluator is deliberately ours and deliberately this small.

**Verify**: `cargo test -q -p djls-project` → unit tests from Step 4 pass.

### Step 4: Tests (`src/extraction/extractor.rs` `#[cfg(test)]` + `tests/` fixtures)

Plain `&str` sources, a `HashMap<String, SettingsEnv>`-backed fake
`StarImportResolver`. Minimum cases (name each test after the idiom):

- literal list / tuple / annotated assignment → Known
- `+=`, `+`-chain with two literal lists, chain splicing another watched name
- `.append`, `.extend`, `.insert(0, x)`, `.remove`
- re-assignment after mutations replaces prior values
- non-literal element skipped → Partial with 1 reason, other values intact
- `INSTALLED_APPS = get_apps()` → Unknown
- `if True:` / `if False:` arms picked; `if DEBUG:` with `DEBUG = True`
  earlier → decidable; `if os.environ.get("X"):` → both arms applied, Partial
- star-import layering: base defines list, importer `+=` → merged, Known;
  unresolvable star import → Partial
- TEMPLATES: full literal dict; `TEMPLATES[0]["DIRS"].append(...)`;
  OPTIONS libraries/builtins extraction; non-literal BACKEND → Partial
- paths: `BASE_DIR = Path(__file__).resolve().parent.parent` then
  `BASE_DIR / "templates"` → Resolved with correct absolute path;
  `os.path.join(BASE_DIR, "templates")`; unknown call → `PathValue::Unknown`
- syntax-error source still yields Partial settings, no panic

**Verify**: `cargo test -q -p djls-project` → all pass;
`cargo test -q` → workspace unaffected.

### Step 5: Full validation

**Verify**: `just clippy`, `just fmt`, `just lint` → exit 0.

## Test plan

Covered in Step 4 (this plan IS mostly tests — the extractor is small). Pattern
to model: table-style fixture tests as in
`crates/djls-semantic/src/python/` test modules. No snapshot tests needed in
v1; plain asserts on the setting structs are clearer here.

## Done criteria

Machine-checkable. ALL must hold:

- [ ] `cargo test -q -p djls-project` exits 0 with ≥ 18 tests
- [ ] `rg "salsa|djls_source|djls_semantic" crates/djls-project/Cargo.toml` returns no matches (crate-level purity — holds until plan 015 brings the project model in; 015's done criteria carry the module-scoped successor check)
- [ ] `rg "sort\(|reverse\(" crates/djls-project/src/extraction/` returns no list-method-emulation matches (scope guard)
- [ ] `rg -n "Fact|Reason::new|SettingsFacts|StringListFact|TemplateBackendFact|into_facts|facts::|walker|Walker|SettingsWalker" crates/djls-project/src/` returns no matches (API vocabulary guard)
- [ ] `rg -n "^pub mod|^pub use" crates/djls-project/src/lib.rs crates/djls-project/src/extraction.rs` returns no matches (no optimistic public surface before plan 007)
- [ ] `cargo test -q` exits 0 (rest of workspace untouched)
- [ ] `just clippy` exits 0
- [ ] Only in-scope files modified (`jj diff --stat`)
- [ ] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- Plan 001 has not landed (`crates/djls-semantic/src/project/static_model.rs`
  exists) — vocabulary collision.
- The pinned ruff AST rev lacks an API this plan assumes (e.g.
  `StringLiteralValue::to_str`) — report the actual API surface rather than
  vendoring workarounds.
- You find yourself adding a fourth `Knowledge` variant, evaluating a
  function body, or tracking aliases (`apps = INSTALLED_APPS`) — that is the
  PR-#606 slope; the answer is a `Reason` + demotion, not more machinery.
- You find yourself adding `salsa`, `djls-source`, or any file I/O to this
  crate "temporarily" — that is plan 015's job, and only for the project
  model, never for the extraction module.

## Maintenance notes

- **The extraction module's purity is the firewall** against the PR-#606
  failure mode (unbounded evaluation): `extract_settings` takes a `&str`
  and a callback, never a database or filesystem. Until plan 015, the
  crate manifest enforces this; after 015 (salsa enters the crate with the
  project model), the enforcement is the module-scoped grep
  `rg "salsa|djls_source" crates/djls-project/src/extraction/` → no
  matches, which plan 015 adds to its done criteria. Reviewers of any
  later change to `src/extraction/` should re-run it.
- The extractor's supported-shapes list is the module's contract — keep it in
  the module doc, and extend it only with evidence (a real-world settings
  file the corpus can't handle). The corpus infrastructure
  (`crates/djls-corpus`, moving into `djls-testing` in plan 016) is the
  right place to later add extraction-over-real-settings snapshot coverage.
- `MIDDLEWARE`, `STATICFILES_DIRS`, etc. are future watched names — the
  table-driven design makes each a one-entry addition.
- Plan 007 consumes this module; its `StarImportResolver` impl is a Salsa
  query with a cycle-safe seed (the `dunder_all_names`
  `cycle_initial=None` pattern, `dunder_all.rs:15`).
- Plan 015 later moves the project model into this crate and the
  registration-scanner half of `djls-semantic`'s `python/registry.rs` into
  this module (it becomes `src/extraction/registry.rs`).
