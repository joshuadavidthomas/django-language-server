# Agent Guidelines

## Build/Test Commands
```bash
cargo build -q                      # Build all crates
cargo clippy -q --all-targets --all-features --fix -- -D warnings  # Lint with fixes
cargo +nightly fmt              # Format code (requires nightly)
cargo test -q                      # Run all tests  
cargo test test_name            # Run single test by name
cargo test -p crate_name        # Test specific crate
just test                       # Run tests via nox (with Django matrix)
just lint                       # Run pre-commit hooks
# NEVER use `cargo doc --open` - it requires browser interaction
```

## Code Style
- **IMPORTANT LSP**: Use `tower-lsp-server` NOT `tower-lsp`. Imports are `tower_lsp_server::*` NOT `tower_lsp::*`
- **LSP Types**: Use `tower_lsp_server::lsp_types` - we don't add `lsp-types` directly, it comes transitively from tower-lsp-server
- **Imports**: One per line, grouped (std/external/crate), vertical layout per `.rustfmt.toml`
- **Errors**: Use `anyhow::Result` for binaries, `thiserror` for libraries
- **Naming**: snake_case functions/variables, CamelCase types, SCREAMING_SNAKE constants
- **Comments**: Avoid unless essential; use doc comments `///` for public APIs only
- **Testing**: Use `insta` for snapshot tests in template parser. NEVER write standalone test files - always add test cases to the existing test modules in the codebase
- **Python**: Inspector runs via zipapp, test against Django 4.2/5.1/5.2/main

## Project Structure
- `crates/djls/` - Main CLI binary and PyO3 interface
- `crates/djls-server/` - LSP server implementation  
- `crates/djls-templates/` - Django template parser
- `crates/djls-workspace/` - Workspace/document management
- `crates/djls-extraction/` - Python AST extraction for tag/filter validation rules (uses Ruff parser)
- `crates/djls-ide/` - Completions, diagnostics, and IDE features
- `crates/djls-semantic/` - Semantic analysis, load resolution, validation
- `crates/djls-project/` - Project/inspector types, Salsa inputs, Python IPC
- `crates/djls-conf/` - Settings and configuration types
- `crates/djls-source/` - Source file abstractions
- `crates/djls-bench/` - Benchmarks

## Operational Notes

### API Shape
- `TemplateTags` does not implement `Deref` — use `.iter()`, `.tags()`, `.len()`, `.is_empty()`
- `TemplateTag` has no `.module()` — use `.defining_module()`, `.registration_module()`, or `.library_load_name()`
- Return `&str` not `&String` from new accessors — clippy flags this
- All public accessors/constructors need `#[must_use]` — clippy enforces `must_use_candidate`
- Pass `&Settings` not `Settings` — clippy flags needless pass by value on large types

### Salsa Patterns
- `#[salsa::tracked]` functions require `&dyn Trait` — cannot use concrete `&DjangoDatabase`
- Tracked return types need `PartialEq` — add derive if missing (e.g., `TagSpecs`)
- Input setters require `use salsa::Setter` — the `.to()` method is a trait method, not inherent
- `DjangoDatabase` already has `#[cfg(test)]` event logging via `logs: Arc<Mutex<Option<Vec<String>>>>` in `db.rs` — reuse for invalidation tests
- `DjangoDatabase::default()` (test-only) creates an `InMemoryFileSystem` and wires up event logging

### Build & Inspector
- After editing `queries.py`, `cargo build` triggers pyz rebuild via `build.rs`
- Inspector rebuild warnings in clippy output (`Building Python inspector...`) are expected, not errors
- `TemplatetagsRequest`/`TemplatetagsResponse` and `inspector_query` are exported from `djls-project`

### Module Layout
- This project uses `foo.rs` + `foo/` sibling pattern — NEVER `foo/mod.rs`
- `djls-semantic` templatetags module: `src/templatetags.rs` (re-exports) + `src/templatetags/` dir (contains `specs.rs`, `builtins.rs`)
- `djls-conf` tagspec types have `PartialEq` but NOT `Eq` — `serde_json::Value` in `extra` field prevents `Eq`
- `djls-extraction` is flat module layout: `src/lib.rs` + `src/{types,error,parser,registry,context,rules,structural,filters,patterns}.rs`
- `djls-extraction` public API: `extract_rules(source) -> ExtractionResult` orchestrates parse→registry→context→rules→structural→filters

### Trait Impls — Update ALL Locations When Changing Traits
Adding a method to `djls-semantic`'s `crate::Db` trait requires updating **6 impl blocks**:
1. `crates/djls-server/src/db.rs` — `impl SemanticDb for DjangoDatabase`
2. `crates/djls-bench/src/db.rs` — `impl SemanticDb for Db`
3. `crates/djls-semantic/src/arguments.rs` — `impl crate::Db for TestDatabase` (in `#[cfg(test)]`)
4. `crates/djls-semantic/src/blocks/tree.rs` — `impl crate::Db for TestDatabase` (in `#[cfg(test)]`)
5. `crates/djls-semantic/src/semantic/forest.rs` — `impl crate::Db for TestDatabase` (in `#[cfg(test)]`)
Test impls typically return `None` / default values. Forgetting even one causes `error[E0046]`.

### Test Dependencies
- `djls-semantic` test modules that use `djls_project` types need `djls-project` in `[dev-dependencies]` in `Cargo.toml`
- Each test `TestDatabase` also needs `impl djls_source::Db` and `impl djls_templates::Db` — check existing test databases for the full trait hierarchy

### Type Evolution
- `InspectorInventory` replaces `TemplateTags` as the `Project.inspector_inventory` field type — all new code should use `InspectorInventory`
- `TemplateTags` still exists for the legacy `templatetags` tracked query — do NOT remove yet, but do NOT use for new features
- `InspectorInventory::new()` takes 4 args: `libraries`, `builtins`, `tags`, `filters`
- `TemplateFilter` accessors return `&str` (not `&String` like `TemplateTag`) — this is the correct pattern per clippy
- `Node::Variable { var, filters, span }` has `filters: Vec<Filter>` — `Filter` has `name: String`, `arg: Option<FilterArg>`, `span: Span`. `FilterArg` has `value: String`, `span: Span`
- Parser's `VariableScanner` is quote-aware — pipes/colons inside `'...'` or `"..."` are not treated as delimiters
- `refresh_inspector()` uses `TemplateInventoryRequest` ("template_inventory" query) — single IPC round trip for tags + filters

### Clippy Patterns
- Use inline format variables: `format!("{var}")` not `format!("{}", var)` — clippy flags `uninlined_format_args`
- `usize as u32` casts require `#[allow(clippy::cast_possible_truncation)]` block — see `calculate_byte_offset` in completions.rs
- `i64 as usize` casts also flagged — any narrowing or sign-changing cast needs the truncation allow
- `#[must_use]` NOT required on methods returning `impl Iterator` — only on pure accessors/constructors
- Functions must not exceed 100 lines — clippy flags `too_many_lines`. Extract helpers to stay under the limit
- Doc comments must use backticks around code/identifiers — clippy flags `doc_markdown` (e.g., write `\`split_contents\`` not `split_contents`)
- Stub structs with fields not yet used need `#[allow(dead_code)]` — common when scaffolding crates phase-by-phase
- Stub functions returning `Result` that can't yet fail need `#[allow(clippy::unnecessary_wraps)]`
- Use `r"..."` not `r#"..."#` when the string contains no `"` — clippy flags `unnecessary_raw_string_hashes`
- Prefer `.map_or(default, f)` → `.is_some_and(f)` or simpler form — clippy flags overly complex `map_or`

### Validation Architecture
- Validation errors (enum variants): `crates/djls-semantic/src/errors.rs` (`ValidationError`)
- Diagnostic code mapping: `crates/djls-ide/src/diagnostics.rs` (maps `ValidationError` variants → S-codes)
- New validation passes are wired into `validate_nodelist()` in `crates/djls-semantic/src/lib.rs`
- Existing codes: S101-S107 (structural), S108 (UnknownTag), S109 (UnloadedLibraryTag), S110 (AmbiguousUnloadedTag), S111 (UnknownFilter), S112 (UnloadedLibraryFilter), S113 (AmbiguousUnloadedFilter)
- Next available diagnostic code: S114

### Build Timeouts
- First build after adding Ruff deps or corpus crate can exceed 10s — use `timeout: 120` or no timeout for cargo builds
- `cargo build -q` and `cargo test -q` suppress normal progress output — only errors shown

### Extraction Feature Gating
- `djls-extraction` has `parser` feature (default on): gates Ruff parser deps and `extract_rules()` function
- `djls-project` depends on `djls-extraction` with `default-features = false` (types only)
- `djls-server` depends on `djls-extraction` with default features (parser enabled)
- `djls-semantic` depends on `djls-extraction` with `default-features = false` (types only, for `TagSpec` fields)
- `Project` salsa input now has `sys_path: Vec<Utf8PathBuf>` and `extracted_external_rules: FxHashMap<String, ExtractionResult>` fields
- `TagSpec` now has `opaque: bool` and `extracted_rules: Vec<ExtractedRule>` — set to `false`/`Vec::new()` in all static builtins
- `PythonEnvRequest`/`PythonEnvResponse` in `djls-project` for `sys_path` query

### Ruff Parser Dependencies
- Ruff crates pinned to tag 0.9.10 (SHA `0dfa810e9aad9a465596768b0211c31dd41d3e73`) in root `Cargo.toml`
- Use `ruff_python_parser`, `ruff_python_ast`, `ruff_text_size` as workspace deps
- `ParsedModule` in `crates/djls-extraction/src/parser.rs` wraps `ruff_python_parser::parse_module`
- `ParsedModule::ast()` returns `&ModModule` directly (not `Mod` enum) — iterate `module.body` for statements
- `extract_name_from_call` is decorator-kind-aware: only `Tag` and `HelperWrapper` use first positional string as name; `inclusion_tag`/`simple_tag`/`simple_block_tag` only support `name=` keyword
- `simple_block_tag` has a special `end_name` keyword arg for custom closer names — always check decorator kwargs before applying convention fallback
- Ruff AST `Expr::StringLiteral` contains `.value.to_str()` for string extraction — `.as_str()` is NOT available on all string types
- Ruff AST `Parameters` has no `defaults` field — defaults are inline on each `ParameterWithDefault` as `default: Option<Box<Expr>>`. Check `arg.default.is_some()` not `params.defaults.is_empty()`

### Extraction Crate Patterns
- Each module has inline `#[cfg(test)] mod tests` — NOT separate test files
- Test sources use `r#"..."#` for multi-line Python code (but prefer `r"..."` if no quotes needed)
- `extract_rules(source)` is the single entry point — returns `ExtractionResult { tags, filters }`
- `RegistrationInfo` has `#[allow(dead_code)]` on some fields until downstream phases consume them
- `FunctionContext::from_registration()` finds the function body and detects `split_contents()` call variable name
- `extract_block_spec()` uses three inference strategies in priority order: (1) explicit `end_name` from decorator, (2) singleton `parser.parse()` call, (3) Django convention fallback (`end{tag_name}`)
- Control-flow recursion in `structural.rs` and `context.rs` must handle `if`/`for`/`while`/`try`/`with` blocks to find nested `parser.parse()` calls and `split_contents()` assignments

### Corpus & Integration Tests
- `djls-corpus` crate: `crates/djls-corpus/` — downloads and extracts PyPI sdists and GitHub tarballs (uses `reqwest` blocking + `flate2` + `tar`)
- `reqwest` needs `features = ["blocking", "json"]` for `.json()` method — `json` feature is NOT default
- Corpus data lives in `crates/djls-corpus/.corpus/` (gitignored), synced via `just corpus-sync`
- Extraction golden tests: `crates/djls-extraction/tests/golden.rs` + `tests/fixtures/` + `tests/snapshots/`
- Corpus tests in `crates/djls-extraction/tests/corpus.rs` skip gracefully when corpus not synced — check dir existence and return early
- Integration test crates (`tests/` dir) use `include_str!` for fixture loading — NOT filesystem reads at runtime

### If-Expression Validation
- `validate_if_expression(bits: &[String]) -> Option<String>` — Pratt parser mirroring Django's `smartif.py`
- Location: `crates/djls-semantic/src/if_expression.rs`
- Two-word operators: `"not" + "in"` → `NotIn`, `"is" + "not"` → `IsNot` — must be handled during tokenization
- Operator precedence: `or(6)`, `and(7)`, `not(8)`, comparisons/in/is(9-10) — matches Django exactly
- `not` is prefix-only (unary), all other operators are infix — the parser distinguishes via `is_prefix`/`is_infix` flags
- Returns `None` for valid expressions, `Some(error_message)` for invalid — error messages match Django's format

### File Locations (avoid repeated lookups)
- Salsa database + tracked queries: `crates/djls-server/src/db.rs`
- Project salsa input: `crates/djls-project/src/project.rs`
- TemplateTags, TagProvenance, InspectorInventory, FilterProvenance, TemplateFilter: `crates/djls-project/src/django.rs`
- Tag specs + `from_config_def`: `crates/djls-semantic/src/templatetags/specs.rs`
- Django builtins specs: `crates/djls-semantic/src/templatetags/builtins.rs`
- Completions (tag names, library names, filter names): `crates/djls-ide/src/completions.rs` (most-edited file — 38 edits across sessions)
- Semantic Db trait: `crates/djls-semantic/src/db.rs`
- Load resolution + tag/filter scoping validation: `crates/djls-semantic/src/load_resolution.rs`
- Validation error types: `crates/djls-semantic/src/errors.rs`
- Diagnostic code mapping: `crates/djls-ide/src/diagnostics.rs`
- Inspector Python queries: `crates/djls-project/inspector/queries.py`
- Session/server wiring: `crates/djls-server/src/session.rs`, `crates/djls-server/src/server.rs`
- Settings/config types: `crates/djls-conf/src/`
- Template parser: `crates/djls-templates/src/parser.rs`
- Node types (Variable, Tag, Filter, FilterArg, etc.): `crates/djls-templates/src/nodelist.rs`
- Parser snapshots: `crates/djls-templates/src/snapshots/`
- Extraction crate types: `crates/djls-extraction/src/types.rs`
- Extraction registration discovery: `crates/djls-extraction/src/registry.rs`
- Extraction function context detection: `crates/djls-extraction/src/context.rs`
- Extraction rule extraction: `crates/djls-extraction/src/rules.rs`
- Extraction block spec inference: `crates/djls-extraction/src/structural.rs`
- Extraction filter arity: `crates/djls-extraction/src/filters.rs` (stub — M5P6)
- Extraction AST pattern helpers: `crates/djls-extraction/src/patterns.rs`
- Extraction orchestration: `crates/djls-extraction/src/lib.rs` (`extract_rules()` public API)
- Extraction golden tests: `crates/djls-extraction/tests/golden.rs` + `tests/fixtures/` + `tests/snapshots/`
- Extraction corpus tests: `crates/djls-extraction/tests/corpus.rs`
- Corpus crate: `crates/djls-corpus/src/` (manifest, sync, enumerate)
- If-expression validation: `crates/djls-semantic/src/if_expression.rs`

## Task Management
Use `/dex` to break down complex work, track progress across sessions, and coordinate multi-step implementations.
