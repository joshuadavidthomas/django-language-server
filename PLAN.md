# `djls check` — Design Plan

A standalone CLI command that walks template files on disk, runs the same
parse → validate → collect pipeline that the LSP uses, and prints diagnostics
to the terminal with an appropriate exit code. Think `ruff check` or `ty check`
but for Django templates.

```
djls check [PATHS...] [--format text|json|github] [--select S100,S101] [--ignore S108]
```

## Prerequisites

Read these before starting implementation:
- **[RESEARCH.md](RESEARCH.md)** — Complete chain analysis of how template
  validation works, from LSP entry points through to the semantic engine.
  Essential context for all three phases.
- **[AGENTS.md](AGENTS.md)** — Project conventions (module style, workspace
  deps, Salsa patterns, testing, etc.)

## What already exists and is reusable

The validation pipeline is already well-factored across crates — the check
command would be a new *consumer* of existing infrastructure, not a rewrite:

| Layer | What exists | Reusable as-is? |
|---|---|---|
| **Parsing** | `djls_templates::parse_template(db, file)` → `NodeList` | ✅ Yes |
| **Validation** | `djls_semantic::validate_nodelist(db, nodelist)` | ✅ Yes |
| **Diagnostic collection** | `djls_ide::collect_diagnostics(db, file, nodelist)` | ✅ Yes (returns `Vec<lsp_types::Diagnostic>`) |
| **Severity filtering** | `djls_conf::DiagnosticsConfig` | ✅ Yes |
| **File kind detection** | `FileKind::from(path)` — `.html`, `.htm`, `.djhtml` → Template | ✅ Yes |
| **Database** | `DjangoDatabase::new(fs, settings, project_path)` | ✅ Yes |
| **Settings** | `djls_conf::Settings::new(path, client_settings)` | ✅ Yes |
| **Template dir discovery** | `template_dirs(db, project)` (via inspector/Python) | ✅ Yes |
| **Library discovery** | `discover_template_libraries(sys_paths)` | ✅ Yes |

## What's missing (new code needed)

1. **Template file walker** — Walk template directories (or user-specified
   paths) on disk, find all files matching `FileKind::Template` extensions.
   Nothing like this exists today; the LSP only validates files that editors
   open.

2. **Diagnostic rendering engine ([#397][gh-397])** — A shared rendering
   implementation for `ValidationError` that produces human-readable output
   with source context snippets and error pointers. This is tracked as a
   separate issue and will be a dependency of the check command. Key points
   from #397:

   - **Crate selection**: `annotate-snippets` is preferred for alignment with
     Ruff/ty. `miette` and `ariadne` are also candidates.
   - **Two modes**: Rich terminal output (colors, underlines) and a plain mode
     for deterministic snapshot files.
   - **Coordinate mapping**: Uses `djls_source::LineIndex` to map byte offsets
     from `ValidationError` spans to line/column numbers.
   - **Pluggable**: Accessible to both the `djls-semantic` test suite and the
     CLI check command.

   The check command's text formatter would be a thin consumer of this engine.
   JSON and GitHub Actions annotation formats would be additional output modes
   layered on top.

3. **The `check` command itself** — CLI arg parsing, database initialization,
   file walking, pipeline orchestration, exit code logic.

[gh-397]: https://github.com/joshuadavidthomas/django-language-server/issues/397

## Key design decisions

### 1. Where does `DjangoDatabase` live? → New `djls-db` crate

**Decision: Create a new `djls-db` crate for the concrete database.**

Moving to `djls-project` would create a dependency cycle: `djls-semantic`
depends on `djls-project` (for `TemplateLibraries`, `Knowledge`, symbol types),
and the concrete database must implement `SemanticDb` (from `djls-semantic`).
So the concrete DB must live in a crate above both.

`djls-db` sits at the top of the Salsa dependency stack — the only crate that
can see all trait layers and implement all of them:

```
djls (CLI binary)
  ├── commands/serve  → djls-server (LSP protocol) → djls-db (database)
  └── commands/check  → djls-db (database)

djls-db (new crate — the "brain")
  ├── DjangoDatabase (concrete Salsa database struct)
  ├── queries (compute_tag_specs, compute_tag_index, compute_filter_arity_specs)
  ├── settings update logic (SettingsUpdate)
  ├── inspector refresh logic
  └── implements: SourceDb, WorkspaceDb, TemplateDb, SemanticDb, ProjectDb

djls-db depends on:
  ├── djls-project    (Project, Inspector, TemplateLibraries, settings types)
  ├── djls-semantic   (SemanticDb trait, validation, TagSpecs, TagIndex)
  ├── djls-templates  (TemplateDb trait)
  ├── djls-workspace  (WorkspaceDb trait, FileSystem)
  ├── djls-source     (SourceDb trait, File, FxDashMap)
  └── djls-conf       (Settings, DiagnosticsConfig)
```

**What moves from `djls-server` → `djls-db`:**
- `db.rs` (`DjangoDatabase` struct + all `#[salsa::db]` trait impls + tests)
- `queries.rs` (`compute_tag_specs`, `compute_tag_index`,
  `compute_filter_arity_specs`, `collect_workspace_extraction_results`)
- `settings.rs` (`SettingsUpdate`, `set_settings`,
  `update_project_from_settings`)
- `inspector.rs` (`refresh_inspector`, `query_inspector_template_libraries`,
  `extract_external_rules`, `update_discovered_template_libraries`)

**What stays in `djls-server`:**
- LSP protocol handlers (`server.rs`)
- `Session` (wraps `DjangoDatabase` with LSP-specific document lifecycle)
- Client capabilities detection
- Push/pull diagnostic publishing
- Logging setup

### 2. How are template files discovered? → Both modes

**Decision: Support both. No args = project discovery, explicit paths override.**

- **No args (default)** — Use `template_dirs(db, project)` to discover
  Django's configured template directories via the inspector, then walk those.
  This is the "just works" mode for projects with a Django environment.
- **Explicit paths** — `djls check templates/` or
  `djls check templates/base.html` — walk the given paths, filter by
  `FileKind::Template`. Useful for CI, scripts, or checking a subset.

### 3. Output format → `text` at launch, others incrementally

**Decision: Ship `text` only at launch. Add other formats later.**

| Format | Use case | When |
|---|---|---|
| `text` (default) | Rich terminal output via the rendering engine from [#397][gh-397] — source snippets, error pointers, colors | Launch |
| `concise` | One-line-per-diagnostic, ruff-style: `path:line:col: S100 Unclosed tag...` | Later |
| `json` | Machine-readable, one JSON object per diagnostic or a JSON array | Later |
| `github` | GitHub Actions annotations: `::error file=...,line=...,col=...::message` | Later |

The `text` format uses the diagnostic rendering engine ([#397][gh-397]) which
will provide `annotate-snippets`-style output with source context. Other formats
are thin formatters over the same underlying error data.

### 4. Exit codes → ruff/ty convention

**Decision: Follow ruff/ty convention.**

- `0` — No errors found (warnings don't count)
- `1` — Errors found
- `2` — Invocation error (bad args, can't find project, etc.)

### 5. What about the inspector / Python dependency? → Both modes

**Decision: Default to full inspector. Explicit paths bypass it.**

The LSP server uses the inspector (Python subprocess) for:

- `template_dirs` — which directories contain templates
- `django_available` — whether Django is importable
- Template library discovery via inspector

The check command supports both modes:

- **No args (default)** — Full inspector, query Django for template dirs and
  library info. Most accurate.
- **Explicit paths** — Skip template dir discovery, just check the given
  files/dirs. Inspector still used for library scoping unless `--no-python`
  is added later.

## Dependencies

The check command depends on [#397][gh-397] (diagnostic rendering engine) for
its `text` output mode. The rendering engine should be implemented first or in
parallel, since it also improves the test suite independently of the check
command.

## Proposed architecture

```
crates/djls/src/commands/check.rs    ← CLI args, orchestration
                                       Uses DjangoDatabase from djls-db
                                       Walks files, runs pipeline, formats output

crates/djls-db/                      ← NEW: concrete database crate
  src/db.rs                            DjangoDatabase struct + trait impls
  src/queries.rs                       compute_tag_specs, compute_tag_index, etc.
  src/settings.rs                      SettingsUpdate, set_settings
  src/inspector.rs                     refresh_inspector, extract_external_rules

New public functions needed:
  - walk_template_files(paths, extensions) → Vec<PathBuf>
  - Rendering engine from #397 handles text/concise formatting
  - JSON and GitHub formatters are small additions
```

## Proposed flow

```
1. Parse CLI args (paths, format, select/ignore, --no-python)
2. Resolve project root (cwd or explicit)
3. Load settings (pyproject.toml / djls.toml)
4. Create DjangoDatabase (with OsFileSystem, settings, project_path)
5. Determine files to check:
   a. If paths given → walk and filter by FileKind::Template
   b. If no paths → query template_dirs via inspector, walk those
6. For each template file:
   a. Create/get Salsa File handle
   b. parse_template(db, file) → Option<NodeList>
   c. validate_nodelist(db, nodelist) [if parse succeeded]
   d. collect_diagnostics(db, file, nodelist) → Vec<Diagnostic>
7. Format and print all diagnostics
8. Exit with appropriate code
```

## Implementation plan

Three phases, in order. Each phase is independently shippable.

### Phase 1: Extract `djls-db` crate ✅

> Landed in [#402](https://github.com/joshuadavidthomas/django-language-server/pull/402).

Structural refactor. No new features, no behavior change. All tests pass
identically before and after.

**1.1 — Create `djls-db` crate skeleton**

- `crates/djls-db/Cargo.toml` — all dependency versions must go in
  `[workspace.dependencies]` in the root `Cargo.toml` per project convention.
  Needs: `djls-project`, `djls-semantic`, `djls-templates`, `djls-workspace`,
  `djls-source`, `djls-conf`, `djls-python`, `salsa`, `camino`, `rustc-hash`,
  `serde`, `serde_json`, `tracing`. Copy the exact dependency set from
  `djls-server/Cargo.toml` for the modules being moved.
- `crates/djls-db/src/lib.rs` with module declarations
- Add to workspace `Cargo.toml` members and `[workspace.dependencies]`
- Uses `folder.rs` convention, NOT `folder/mod.rs` (per AGENTS.md)

**1.2 — Move `DjangoDatabase` struct**

Move from `djls-server/src/db.rs` → `djls-db/src/db.rs`:
- `DjangoDatabase` struct definition
- `DjangoDatabase::new()`, `DjangoDatabase::set_project()`
- All `#[salsa::db]` trait impls (`salsa::Database`, `SourceDb`, `WorkspaceDb`,
  `TemplateDb`, `SemanticDb`, `ProjectDb`)
- `#[cfg(test)] Default` impl
- `invalidation_tests` module

**1.3 — Move query functions**

Move `djls-server/src/queries.rs` → `djls-db/src/queries.rs`:
- `compute_tag_specs()`
- `compute_tag_index()`
- `compute_filter_arity_specs()`
- `collect_workspace_extraction_results()`

**1.4 — Move settings update logic**

Move `djls-server/src/settings.rs` → `djls-db/src/settings.rs`:
- `SettingsUpdate` struct
- `DjangoDatabase::set_settings()`
- `DjangoDatabase::update_project_from_settings()`

**1.5 — Move inspector refresh logic**

Move `djls-server/src/inspector.rs` → `djls-db/src/inspector.rs`:
- `DjangoDatabase::refresh_inspector()`
- `DjangoDatabase::query_inspector_template_libraries()`
- `DjangoDatabase::extract_external_rules()`
- `DjangoDatabase::update_discovered_template_libraries()`

**1.6 — Update `djls-server` to import from `djls-db`**

- Add `djls-db` dependency to `djls-server/Cargo.toml`
- `session.rs`: import `DjangoDatabase` from `djls_db` instead of `crate::db`
- `server.rs`: import `SettingsUpdate` from `djls_db`
- `lib.rs`: remove `pub mod db`, `mod queries`, `mod settings`, `mod inspector`
  module declarations. The server's `lib.rs` currently exports `pub mod db` —
  check if anything outside the crate imports `djls_server::db::*` and update
  those imports to `djls_db::*`
- Remove `db.rs`, `queries.rs`, `settings.rs`, `inspector.rs` from
  `djls-server/src/`
- Review `djls-server/Cargo.toml` — may be able to drop direct dependencies
  that are now transitive through `djls-db`

**1.7 — Check `djls-bench` impact**

- `djls-bench` has its own `BenchDatabase` that implements `SemanticDb`.
  Verify it doesn't import from `djls-server::db`. If it does, update to
  import from `djls-db`. If it only uses trait impls from lower crates, no
  change needed.

**1.8 — Update `djls` binary crate**

- Add `djls-db` dependency to `djls/Cargo.toml` (needed for the check command
  later, but also ensures it compiles now)

**1.9 — Verify**

- `cargo build -q`
- `cargo clippy -q --all-targets --all-features --fix -- -D warnings`
- `cargo test -q`
- `just test` (Django matrix)
- `just lint`

### Phase 2: Diagnostic rendering engine (#397) ⬜

Tracked separately in [#397][gh-397]. Summary of what's needed:

**2.1 — Evaluate and select rendering crate**

`annotate-snippets` (preferred), `miette`, or `ariadne`. Build a small
proof-of-concept with one `ValidationError` variant to compare.

**2.2 — Implement renderer**

- Lives in `djls-source` — the renderer is generic over any diagnostic, not
  tied to `ValidationError`. Its API takes source-level primitives that
  `djls-source` already owns:
  ```rust
  render_diagnostic(source, span: Span, line_index: &LineIndex, code, message, severity)
  ```
- `djls-source` already owns `Span`, `LineIndex`, `Offset`, `LineCol` — the
  renderer is the presentation side of these same types
- Call sites (semantic tests, check command) extract span/code/message from
  their error types and pass them to the generic renderer
- Two modes: rich (colors, underlines) and plain (deterministic for snapshots)
- **Future consideration:** A dedicated `djls-diagnostic` crate could
  eventually consolidate all diagnostic concerns — the renderer, severity
  types (from `djls-conf`), `DiagnosticError` trait (from `djls-ide`),
  `collect_diagnostics` (from `djls-ide`), and LSP conversion. Starting in
  `djls-source` is fine for now; if diagnostics grow beyond rendering, that's
  the signal to extract

**2.3 — Adopt in snapshot tests**

- Update `djls-semantic` snapshot tests to use the renderer in plain mode,
  extracting span/code/message from `ValidationError` at the call site
- Run `cargo insta test --accept --unreferenced delete`
- Verify snapshots are more readable than raw struct output

### Phase 3: `djls check` command ⬜

Depends on Phase 1 ✅ and Phase 2 (rendering engine for output).

**3.1 — Template file walker**

Add to `djls-db` (it already depends on `djls-source` for `FileKind`):
- `walk_template_files(paths: &[Utf8PathBuf]) -> Vec<Utf8PathBuf>` —
  recursively walk directories, filter by `FileKind::Template` extensions
  (`.html`, `.htm`, `.djhtml`)
- Uses `walkdir` or `ignore` crate for efficient directory traversal
- For single files, just check the extension and include directly

**3.2 — CLI arg parsing**

Add `crates/djls/src/commands/check.rs`:
```rust
#[derive(Debug, Parser)]
pub struct Check {
    /// Files or directories to check. If omitted, discovers from Django
    /// template directories.
    paths: Vec<PathBuf>,

    /// Select specific diagnostic codes to enable
    #[arg(long)]
    select: Vec<String>,

    /// Ignore specific diagnostic codes
    #[arg(long)]
    ignore: Vec<String>,
}
```

Register in `commands.rs` as `DjlsCommand::Check(check::Check)`.

**3.3 — Check orchestration**

In `Check::execute()`:
1. Resolve project root (cwd)
2. Load `Settings::new(root, None)`, merge `--select`/`--ignore` into
   `DiagnosticsConfig`
3. Create file system: `Arc::new(djls_workspace::OsFileSystem)` (no overlay
   needed — check reads from disk only, no editor buffers)
4. Create `djls_db::DjangoDatabase::new(fs, settings, project_root)`
5. Determine files:
   - If stdin is not a terminal → read stdin as a single template
   - Else if `self.paths` is non-empty → `walk_template_files(&self.paths)`
   - Else → `template_dirs(db, project)` then walk those
6. For each file: `db.get_or_create_file(path)`, `parse_template(db, file)`,
   `validate_nodelist(db, nodelist)`, `collect_diagnostics(db, file, nodelist)`
7. Convert LSP diagnostics to rendered text: `collect_diagnostics` returns
   `Vec<lsp_types::Diagnostic>` — extract range/code/message from each and
   pass to the rendering engine from Phase 2. The LSP types are a transitive
   dependency (via `djls-ide`) and carry all needed data.
8. Print to stdout, return appropriate exit code

**3.4 — Stdin support**

Detect `!std::io::stdin().is_terminal()`:
- Read all of stdin into a string
- Create an in-memory file (use a synthetic path like `<stdin>`)
- Run through the same pipeline
- Print diagnostics and exit

**3.5 — Exit codes**

- `0` — no errors (warnings/hints don't count)
- `1` — one or more errors found
- `2` — invocation error (bad args, project not found, etc.)

Map to the existing `Exit` type in `crates/djls/src/exit.rs`.

**3.6 — Integration test**

Add a basic integration test that:
- Creates a temp directory with a template containing known errors
- Runs `djls check <dir>` as a subprocess
- Asserts exit code is 1
- Asserts output contains expected error codes (S100, etc.)

## Open questions

1. ~~**Where should the concrete database live?**~~ **Decided: New `djls-db`
   crate.** Can't live in `djls-project` due to a dependency cycle
   (`djls-semantic` → `djls-project`). `djls-db` sits above both and owns
   the concrete database, queries, and settings logic. Both the LSP server
   and CLI check command are thin entry-point shells.

2. ~~**Should the inspector (Python subprocess) be required or optional?**~~
   **Decided: Both modes.** No args = project-aware discovery via inspector.
   Explicit paths bypass discovery. A `--no-python` flag may still be useful
   later for structural-only checks in environments without Python.

3. ~~**Which output formats from day one?**~~ **Decided: `text` only at
   launch.** Rich snippet output via the rendering engine ([#397][gh-397]).
   `concise`, `json`, and `github` formats added incrementally.

4. ~~**Should explicit paths be supported from the start, or only project-aware
   discovery?**~~ **Decided: Both from the start.** Explicit paths are simple
   and work without Django. Project-aware discovery is the default when no
   args are given.

5. ~~**Any additional CLI flags to plan for?**~~ **Decided: Minimal for MVP.**
   - `--quiet` / `--verbose` — already global in `args.rs`, works for free
   - `--select` / `--ignore` — CLI overrides for diagnostic codes, merged
     into `DiagnosticsConfig` (plumbing already exists in `djls-conf`)
   - stdin — detect piped input automatically (no flag needed), check as a
     single template
   - Future candidates (not MVP): `--fix`, `--statistics`, `--diff`

6. ~~**Should the diagnostic rendering engine ([#397][gh-397]) be implemented
   before or in parallel with the check command?**~~ **Decided: #397 first.**
   The rendering engine is independently valuable — it unlocks nicer snapshot
   testing across the whole project. Once landed, the check command gets its
   `text` formatter for free.
