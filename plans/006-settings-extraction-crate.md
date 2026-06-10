# Plan 006: Build `djls-extraction` — a bounded, pure settings.py recognizer

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat 922cc4d7..HEAD -- Cargo.toml crates/djls-semantic/src/project.rs`
> (This plan mostly creates a new crate; drift only matters for the workspace
> manifest and for plan 001 having landed — `crates/djls-semantic/src/project/static_model.rs`
> must NOT exist. If it still exists, STOP: plan 001 is a prerequisite.)

## Status

- **Priority**: P1
- **Effort**: L
- **Risk**: LOW (new crate, nothing wired in yet)
- **Depends on**: plans/001-delete-static-scaffolding.md
- **Category**: direction (static Django discovery)
- **Planned at**: commit `922cc4d7`, 2026-06-10

## Why this matters

The goal of the whole plan series: resolve Django project facts —
`INSTALLED_APPS`, `TEMPLATES` (dirs, APP_DIRS, OPTIONS libraries/builtins) —
from Python source without running Python. Two prior attempts (PRs #606,
#626) proved feasibility but drowned in unbounded evaluation. The reference
projects show the right size: ty extracts `__all__` — the same problem shape
as `INSTALLED_APPS` (a module-level list of strings built by assignment,
`+=`, `.append()`/`.extend()`, cross-module composition, and conditional
branches) — in one 444-line bounded walker
(`reference/ruff/crates/ty_python_semantic/src/dunder_all.rs`) with an
`invalid` latch instead of guessing. ty's own philosophy comment
(`reference/ruff/crates/ty_module_resolver/src/resolve.rs:1515-1521`):
"This is all syntax-only analysis so it *could* be fooled but it's really
unlikely… this is better than nothing!" This plan builds that walker for
Django settings as a **pure crate** — AST in, facts out, no Salsa, no I/O —
so plan 007 can wire it into tracked queries.

## Current state

- The workspace already pins the parser the walker needs
  (root `Cargo.toml:60-61`):

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

- No `crates/djls-extraction` exists. Plan 001 has deleted the conflicting
  `Fact<T>`/`ImportRoot`/`ResolvedModule` vocabulary (verify in the drift
  check).

### Reference implementations to model (read these before coding)

All under `reference/ruff/` in this repo:

| Technique | Where | What to copy |
|---|---|---|
| Invalid-latch walker | `crates/ty_python_semantic/src/dunder_all.rs:26-66, 199-209` | per-name latch: unrecognized idiom → that fact is Unknown with a reason, never a guess |
| `.append`/`.extend`/`.remove` handling | `dunder_all.rs:106-148` | single-positional-arg call idioms on the watched name |
| Re-assignment clears prior state | `dunder_all.rs:57-66` (`update_origin`) | `INSTALLED_APPS = [...]` after a previous assignment replaces, not extends |
| Statically evaluated `if`/`elif`/`else` | `dunder_all.rs:364-390` | walk only the live arm when truthiness is decidable |
| `+`-chain folding | `crates/ruff_python_semantic/src/model/all.rs:109-134` | linearize left-leaning BinOp chains, extract elements per operand |
| Partial-with-flags (don't whole-bail) | `model/all.rs` (`DunderAllFlags`) | skip a bad element, record why, keep extracting |
| Literal-int tuple comparison | `crates/ruff_linter/src/rules/pyupgrade/rules/outdated_version_block.rs:439-452` | the shape of a bounded comparator (only if you implement version checks — optional) |

## Commands you will need

| Purpose      | Command                            | Expected on success |
|--------------|------------------------------------|---------------------|
| Build        | `cargo build -q -p djls-extraction`| exit 0              |
| Test (crate) | `cargo test -q -p djls-extraction` | exit 0, all pass    |
| Test (all)   | `cargo test -q`                    | exit 0, all pass    |
| Lint         | `just clippy`                      | exit 0, no warnings |
| Format       | `just fmt`                         | exit 0              |
| Hooks        | `just lint`                        | exit 0              |

## Scope

**In scope** (the only files you should modify/create):
- `crates/djls-extraction/` (create everything)
- Root `Cargo.toml` (workspace member)

**Out of scope** (do NOT touch, even though they look related):
- `crates/djls-semantic/` — wiring happens in plan 007. This plan must land
  with zero behavior change to the language server.
- File reading, module resolution against search paths, Salsa — the crate is
  pure by design; the caller supplies source text and answers star-import
  questions through a callback.
- A general Python evaluator. Explicitly rejected: list `.sort()`/
  `.reverse()` emulation, importing path-returning *functions* from sibling
  modules, dict/set algebra beyond the TEMPLATES shapes below, arbitrary
  function-call evaluation. (These are where PR #606 went off the rails;
  neither ruff nor ty models any of them.)

## Git workflow

jj repo — no mutating `git`. Commit per step or logical unit, message style
matching `git log` (e.g. `"add settings extraction walker"`,
`"test: cover star-import layering"`). Do NOT push.

## Steps

### Step 1: Scaffold

`crates/djls-extraction/Cargo.toml` modeled on `crates/djls-conf/Cargo.toml`
(version `0.0.0`, workspace lints/edition). Dependencies (all
`workspace = true`): `camino`, `ruff_python_ast`, `ruff_python_parser`,
`rustc-hash`, `serde` only if an existing sibling justifies it (default: no).
**No** `salsa`, **no** `djls-source`, **no** `djls-semantic`.

**Verify**: `cargo build -q -p djls-extraction` → exit 0.

### Step 2: Define the boundary types (`src/lib.rs` + `src/facts.rs`)

```rust
/// How much to trust an extracted fact. The ONLY confidence vocabulary in
/// this codebase (plan 007 migrates djls-semantic onto it).
pub enum Knowledge { Known, Partial, Unknown }

/// Why a fact is Partial/Unknown. Human-readable, shown in logs/diagnostics.
pub struct Reason { pub message: String }   // + span/source fields as needed

/// A best-effort string list (INSTALLED_APPS shape).
pub struct StringListFact {
    pub values: Vec<String>,
    pub knowledge: Knowledge,
    pub reasons: Vec<Reason>,
}

/// One entry of TEMPLATES.
pub struct TemplateBackendFact {
    pub backend: Option<String>,            // BACKEND string literal
    pub dirs: Vec<PathValue>,               // DIRS entries
    pub app_dirs: Option<bool>,             // APP_DIRS literal bool
    pub libraries: Vec<(String, String)>,   // OPTIONS["libraries"]: name -> module
    pub builtins: Vec<String>,              // OPTIONS["builtins"]
    pub knowledge: Knowledge,
    pub reasons: Vec<Reason>,
}

/// Everything extracted from one settings module (after layering).
pub struct SettingsFacts {
    pub installed_apps: StringListFact,
    pub template_backends: Vec<TemplateBackendFact>,
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

`SettingsEnv` is the walker's working state (watched-name bindings + aux
constants like `BASE_DIR`); keep it `pub` with a `into_facts()` finisher.
Internal representation is yours, but the public API above is fixed.

**Verify**: `cargo build -q -p djls-extraction` → exit 0.

### Step 3: The walker (`src/walker.rs`)

Public entry:

```rust
pub fn extract_settings(
    source: &str,
    module_path: &camino::Utf8Path,     // for resolving Path(__file__) and relative dirs
    resolver: &mut dyn StarImportResolver,
) -> SettingsFacts
```

Parse with `ruff_python_parser::parse_module`; on syntax errors, still walk
the recovered AST (the parser always produces one) but mark all facts
`Partial` with a "syntax errors present" reason.

Walk **module-level statements only** — never descend into function or class
bodies (dunder_all does the same). DO walk into `if`/`for`/`while`/`with`/
`try` bodies (`dunder_all.rs:392-398` walks these).

Supported shapes per watched list name (`INSTALLED_APPS`; design the walker
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
   deviation in the walker's module doc.
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

Path micro-evaluator (`src/paths.rs`) — closed grammar, evaluated relative to
`module_path`; everything else `PathValue::Unknown`:
- `Path(__file__).resolve().parent` chains (each `.parent` pops one segment)
- aux-name references (`BASE_DIR`) whose value was set by a recognized path
  expression earlier in the walk
- `<path expr> / "literal"` (BinOp Div), `.joinpath("literal")`
- `os.path.join(<path expr>, "literal", ...)`, `os.path.dirname(<path expr>)`
- `str(<path expr>)`
Note: ty does *no* path-value evaluation at all (`Path.__truediv__` is just a
type to ty) — this evaluator is deliberately ours and deliberately this small.

**Verify**: `cargo test -q -p djls-extraction` → unit tests from Step 4 pass.

### Step 4: Tests (`src/walker.rs` `#[cfg(test)]` + `tests/` fixtures)

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
- syntax-error source still yields Partial facts, no panic

**Verify**: `cargo test -q -p djls-extraction` → all pass;
`cargo test -q` → workspace unaffected.

### Step 5: Full validation

**Verify**: `just clippy`, `just fmt`, `just lint` → exit 0.

## Test plan

Covered in Step 4 (this plan IS mostly tests — the walker is small). Pattern
to model: table-style fixture tests as in
`crates/djls-semantic/src/python/` test modules. No snapshot tests needed in
v1; plain asserts on the fact structs are clearer here.

## Done criteria

Machine-checkable. ALL must hold:

- [ ] `cargo test -q -p djls-extraction` exits 0 with ≥ 18 tests
- [ ] `rg "salsa|djls_source|djls_semantic" crates/djls-extraction/Cargo.toml` returns no matches (purity)
- [ ] `rg "sort\(|reverse\(" crates/djls-extraction/src/` returns no list-method-emulation matches (scope guard)
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

## Maintenance notes

- The walker's supported-shapes list is the crate's contract — keep it in the
  module doc, and extend it only with evidence (a real-world settings file
  the corpus can't handle). The corpus infrastructure
  (`crates/djls-corpus`) is the right place to later add
  extraction-over-real-settings snapshot coverage.
- `MIDDLEWARE`, `STATICFILES_DIRS`, etc. are future watched names — the
  table-driven design makes each a one-entry addition.
- Plan 007 consumes this crate; its `StarImportResolver` impl is a Salsa
  query with a cycle-safe seed (the `dunder_all_names`
  `cycle_initial=None` pattern, `dunder_all.rs:15`).
