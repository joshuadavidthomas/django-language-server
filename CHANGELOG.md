# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project attempts to adhere to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

<!--
## [${version}]
### Added - for new features
### Changed - for changes in existing functionality
### Deprecated - for soon-to-be removed features
### Removed - for now removed features
### Fixed - for any bug fixes
### Security - in case of vulnerabilities
[${version}]: https://github.com/joshuadavidthomas/django-language-server/releases/tag/v${version}
-->

## [Unreleased]

### Added

- Added quick-fix code actions for loading missing Django template tag libraries.
- Added quick-fix code actions for choosing among ambiguous unloaded Django template tag libraries.
- Added a quick-fix code action for renaming mismatched `{% endblock %}` names.
- Added completion for resolvable template names inside quoted `{% extends %}` and `{% include %}` arguments.
- Added document links for resolvable Django template and template-library references.
- Added opt-in whole-document Django template formatting through `djangofmt`.
- Added startup progress reporting for Django project discovery and IDE cache warm-up.
- Added a public `ROADMAP.md` for current and planned Django/LSP capabilities.
- **Internal**: Added domain glossary docs for canonical project terminology.
- **Internal**: Added block-resolution queries (`parent_block`, `inherited_blocks`, `block_overrides`) over the template inheritance chain.
- **Internal**: Added the `template_inheritance` extends-chain query with explicit `ChainEnd` terminators and Django origin-skip resolution.
- **Internal**: Added a per-file `template_symbols` definition layer (blocks, partials, extends target) to `djls-semantic`.
- **Internal**: Added `just hawk` visibility lint configuration for crate-boundary cleanup.
- **Internal**: Added end-to-end LSP coverage for initialization, diagnostics, navigation, completions, hover, folding ranges, document symbols, and startup progress.
- **Internal**: Added the `djls-testing` crate for shared fixtures, corpus syncing, and Salsa-backed test databases.

### Changed

- Changed Django settings analysis to preserve exact alternative configurations and known field values through unrelated dynamic expressions.
- Changed template semantics and IDE features to use each file's feasible backends and the Template Library definitions active at each source position.
- Changed template-library completion and installed-app guidance to retain known results through unrelated discovery problems, while unknown-library and unknown-symbol diagnostics now require an exhaustive miss.
- Changed template navigation to avoid reporting templates as missing when configuration or filesystem search is incomplete, while retaining known possible destinations.
- Changed unreadable files to be skipped instead of analyzed as empty source.
- Changed Python model and template-spec extraction to retain known facts after recoverable syntax errors.
- Changed template goto definition to return origin ranges for clients that support definition links.
- Changed template tag library discovery to derive libraries from project source and Django settings instead of the runtime inspector.
- Changed static Django discovery to preserve inactive-app evidence for `S118`, `S119`, and `S121` diagnostics without runtime introspection.
- Changed Django discovery to run after LSP initialization, refresh in the background, and warm IDE caches without blocking read requests.
- Changed startup refreshes to run discovery phases in parallel and report counted progress.
- Changed template validation and completion scoping to use localized inventory completeness, suppressing absence diagnostics only where discovery evidence is incomplete.
- Changed template formatting to honor LSP/editor formatting options for indentation and final/trailing whitespace.
- Bumped Rust toolchain from 1.95 to 1.96 and moved workspace crates to Rust 2024.
- **Internal**: Added multi-file scenario support and a pluggable snapshot renderer to the `djls-testing` mdtest harness.
- **Internal**: Reshaped template tag library storage around loadable and builtin mounts.
- **Internal**: Moved the project model and static source recognizers into `djls-project`.
- **Internal**: Moved Python spec extraction and template-origin resolution into `djls-project`, leaving `djls-semantic` as the project-meaning layer.
- **Internal**: Refactored template semantics around `TemplateTree` for validation, references, outlines, folding, and opaque-region handling.
- **Internal**: Reworked template analysis around per-library semantic products, sparse per-Template projections, and generation-gated production priming.
- **Internal**: Updated realistic template benchmarks to use project-backed source extraction, canonical builtin roles, and snapshot-backed workload contracts.
- **Internal**: Moved corpus tooling and shared test fixtures into `djls-testing`.
- **Internal**: Reworked Python settings evaluation around module identities, typed import outcomes, invariant-preserving collections, and correlated list alternatives.

### Removed

- Removed the runtime inspector subprocess, embedded Python zipapp, and `~/.cache/djls/inspector/` disk cache. The server no longer needs a working Django setup to derive project facts.
- Removed the template library snapshot disk cache and startup cache-loading phase.
- **Internal**: Removed `djls-workspace`; workspace/filesystem logic now lives in `djls-source` and `djls-server`.
- **Internal**: Removed the deprecated `extract_rules` extraction path.

### Fixed

- Fixed Python settings evaluation to distinguish list and tuple concatenation while preserving known collection facts through supported iterable extensions.
- Fixed relative imports in Django settings modules, including package aliases and overlapping Python search roots.
- Fixed project reloads so an analysis task panic no longer prevents later reloads.
- Fixed Django relative template paths (`./`, `../`) in `{% extends %}` and `{% include %}` resolution, including inheritance chains, document links, goto definition, and find references.
- Fixed false tag argument errors for manually parsed tags that strip trailing assignment clauses, such as `{% now ... as var %}` and `{% url ... as var %}`.

## [6.0.3]

### Added

- Added document symbols for Django template structure outlines.
- Added hover documentation for Django template tags, filters, libraries, and template references.
- Added folding ranges for Django template block, comment, and import regions.
- Added pre-commit hook for running `djls check` on Django template files.
- Added rg-style file filtering flags to `djls check`: `-g/--glob` for glob patterns, `--no-ignore` to skip ignore files, `-L/--follow` for symlinks, `-d/--max-depth` for recursion depth, `--color always|auto|never`, and `-q/--quiet`.
- Added `env_file` configuration option for loading environment variables from a `.env` file into the inspector process.
- Filesystem cache for template library snapshots (`~/.cache/djls/inspector/`). On subsequent startups, cached data is loaded in ~2ms instead of waiting 200-700ms for the Python subprocess. The cache is validated in the background on every startup.
- **Internal**: Added venv model scanning, workspace model discovery, and Salsa wiring for `compute_model_graph` — the model graph is now populated from both site-packages and workspace `models.py` files with automatic invalidation on edit.
- **Internal**: Added `just dev profile` recipe for local flamegraph profiling of benchmarks.
- **Internal**: Added `[profile.bench]` with `debug = 2` to `Cargo.toml` for symbolized bench profiles.

### Changed

- Bumped Rust toolchain from 1.93 to 1.95.
- **Internal**: Stabilized benchmarks with sized diagnostics fixtures, repeated microbench companions, validation-rendering contract assertions, and deterministic corpus input ordering.
- **Internal**: Combined validation scoping checks for closing and intermediate tags into a single tag-spec pass.
- Changed `djls serve` terminal warning output to plain text without decorative separator lines.
- Improved `djls check --quiet` performance by counting diagnostics without rendering formatted output.
- **Internal**: Consolidated `djls-project` and `djls-python` into `djls-semantic` as the unified Django semantic model.
- **Internal**: Renamed template library snapshot types and cache APIs around backend-neutral semantic snapshots instead of inspector responses.
- **Internal**: Hid template library inspector request plumbing behind a semantic snapshot fetch API.
- **Internal**: Replaced the database inspector handle with a backend-neutral project introspector.
- **Internal**: Added a semantic `validate_template_file` convenience query for file-level template validation.
- **Internal**: Unified `djls-corpus` to repo-only format, removing the PyPI package path. All corpus entries are now `[[repo]]` in `manifest.toml`, fetched as git archives. Removed `sha2`, `toml_edit` dependencies and the `add` CLI command.
- **Internal**: Added license file fetching to `djls-corpus lock`. License text is saved to `crates/djls-corpus/licenses/` for attribution.
- **Internal**: Replaced manual `AvailableSymbols` cache in `TemplateValidator` with `SymbolIndex`. Symbol availability is now precomputed per `{% load %}` boundary and memoized across revisions, eliminating per-walk rebuilds and hand-rolled invalidation logic.
- **Internal**: Moved `check_file` orchestration and diagnostic rendering into `djls-db`. Removed `djls-ide`, `djls-templates`, and `djls-project` dependencies from the CLI binary.
- **Internal**: Moved extraction orchestration from `djls-project` to `djls-db`. Removed unused `djls-workspace` dependency from `djls-project`.
- Widened templatetag extraction to catch any uncaught exception in compilation functions, not just `TemplateSyntaxError`. Tags that raise `ValueError`, `TypeError`, or other exceptions in guards now produce validation constraints.
- Parallelized inspector subprocess query and filesystem library discovery during startup, hiding discovery latency behind the slower inspector call.
- **Internal**: `LoadState` now borrows `&str` from `LoadedLibraries` instead of cloning strings, and `compute_loaded_libraries` returns a reference via `returns(ref)`. Eliminated all string allocations in `available_at`.
- **Internal**: Consolidated `TagIndex` from three separate tracked fields into a single `roles` map, reducing `classify` from 3 Salsa field accesses to 1. `TagClass` now borrows from Salsa storage instead of cloning.
- **Internal**: Replaced template `BlockTree`/`SemanticForest` structure analysis with `TemplateTree`, a structural semantic projection used by opaque-region computation and future outline features.

### Removed

- Removed support for the deprecated TagSpecs v0.4.0 flat format.
- Dropped Django 4.2 from supported versions.
- Dropped Django 5.1 from supported versions.

### Fixed

- Fixed template parser diagnostics to preserve structured parser errors and source spans while retaining the existing `T100` diagnostic code. Malformed filter expressions now produce parser errors instead of being silently ignored.
- Suppressed `failed to send notification` ERROR log spam during server shutdown by disabling the LSP log forwarding layer on shutdown.
- Fixed `djls check` silently ignoring file arguments when `-` (stdin) was also passed. This command now returns an error for mixed stdin/path input.
- Fixed `djls serve --connection-type tcp` silently using stdio. The command now errors with an unsupported-mode message.

## [6.0.2]

### Changed

- Bumped TagSpecs v0.4.0 deprecation removal target from v6.0.2 to v6.0.3.

### Fixed

- Fixed `djls check` with no arguments silently reading stdin instead of discovering template files. Stdin is now triggered explicitly by passing `-` as a path.
- Fixed structural validation (unclosed tags, unbalanced blocks) being silently skipped when no Python inspector data was available. `builtin_tag_specs()` now provides Django's standard tag definitions as a fallback.

## [6.0.1]

### Added

- Added `djls check` CLI command for validating Django template files.
- Added automated extraction of tag and filter validation rules from Python source code.
- Added diagnostics for tag argument counts (S117), library resolution (S120, S121), and symbol scoping (S118, S119).
- Added structural validation for {% extends %} placement (S122, S123).

### Changed

- **Internal**: Extracted concrete Salsa database into new `djls-db` crate.
- **Internal**: Re-architected core analysis pipeline using Salsa for incremental computation.
- **Internal**: Consolidated template validation into a single-pass visitor.
- **Internal**: Updated completions and snippets to use extracted argument structures.
- Bumped Rust toolchain from 1.90 to 1.91.

### Fixed

- Fixed a typo in the TagSpecs v0.4.0 deprecation notice; removal is targeted for v6.0.2 (matching the "two releases of warning" policy), not v6.2.0.

## [6.0.0]

### Changed

- Updated TagSpecs to v0.6.0 format with hierarchical `[[tagspecs.libraries]]` structure

### Deprecated

- TagSpecs v0.4.0 flat format (will be removed in v6.0.2). See the [TagSpecs docs](docs/configuration/tagspecs.md).

### Removed

- Removed deprecated `lazy.lua` Neovim plugin spec.

### Fixed

- Fixed false positive "accepts at most N arguments" errors for expressions with operators (e.g., `{% if x > 0 %}`)
- Fixed false positive errors for quoted strings with spaces (e.g., `{% translate "Contact the owner" %}`)

## [5.2.4]

### Added

- Added `diagnostics.severity` configuration option for configuring diagnostic severity levels
- Added `pythonpath` configuration option for specifying additional Python import paths
- Added documentation for VS Code extension
- Added documentation for Zed extension
- Added documentation for setting up Sublime Text
- Added documentation for setting up Neovim 0.11+ using `vim.lsp.config()` and `vim.lsp.enable()`

### Changed

- Changed user configuration directory paths to use application name only, removing organization identifiers
- Changed log directory to use XDG cache directories (e.g., `~/.cache/djls` on Linux) with `/tmp` fallback
- **Internal**: Refactored workspace to use domain types (`FileKind`) instead of LSP types (`LanguageId`)
- **Internal**: Added client detection for LSP-specific workarounds (e.g., Sublime Text's `html` language ID handling)

### Deprecated

- Deprecated `lazy.lua` Neovim plugin spec. It now only shows a migration warning and will be removed in the next release. See [Neovim client docs](docs/clients/neovim.md) for the new Neovim 0.11+ configuration approach.

### Removed

- Removed `clients/nvim/` directory. Neovim 0.11+ has built-in LSP configuration via `vim.lsp.config()` which replaces the need for the custom plugin.

## [5.2.3]

### Added

- Added support for Python 3.14

### Removed

- Dropped support for Python 3.9

## [5.2.2]

### Added

- Added support for `djhtml` file extension
- Added standalone binary builds for direct installation
- Added debug Neovim plugin for lsp-devtools and server logs

### Changed

- Reorganized clients directory
- **Internal**: Optimized lexer performance with memchr and byte-level whitespace parsing
- **Internal**: Simplified background tasks with `SessionSnapshot`
- **Internal**: Refactored LSP boundary with extension traits

### Fixed

- Fixed stale diagnostics and references for templates open in the editor
- Fixed template file tracking by moving Db methods to SourceDb

## [5.2.1]

### Added

- Added support for Django 6.0
- Added find references for extends/include tags
- Added go to definition for extends/include tags

### Changed

- Bumped Rust toolchain from 1.88 to 1.90
- **Internal**: Switched Python inspector from PyO3 to IPC-based approach
- **Internal**: Refactored various internal components
- **Internal**: Improved template parsing performance with token caching
- **Internal**: Optimized parser performance
- **Internal**: Added benchmarks for performance testing

## [5.2.0]

### Added

- Added context-aware completions with snippets
- Added support for loading server settings from user files (`~/.config/djls/djls.toml`) and project files (`djls.toml`, `.djls.toml`, and `pyproject.toml` via `[tool.djls]` table`)
- Implemented dynamic settings reloading via `workspace/didChangeConfiguration`
- Added `venv_path` setting to allow explicit configuration of Python virtual environment
- Added unified file and LSP logging using tracing to server
- Added virtual `FileSystem` for workspace file management
- Implemented `textDocument/didSave` LSP method
- Added typed argspecs for LSP snippets to tagspecs configuration

### Changed

- Refactored tagspecs configuration to use array of tables and consistent fields
- Bumped Rust toolchain from 1.87 to 1.88
- Bumped PyO3/maturin-action to 1.49.3
- Bumped Salsa crate from git hash to 0.23.0
- **Internal**: Moved task queueing functionality to `djls-server` crate, renamed from `Worker` to `Queue`, and simplified API
- **Internal**: Improved Python environment handling, including refactored activation logic
- **Internal**: Centralized Python linking build logic into a shared `djls-dev` crate to reduce duplication
- **Internal**: Started Salsa integration for incremental computation with database structure and initial Python environment discovery functionality
- **Internal**: Reorganized server crate by moving workspace related code to submodule
- **Internal**: Simplified Salsa database management with `Clone` + `Arc<Mutex<Session>>`
- **Internal**: Moved Salsa database ownership from `Workspace` to `Session`
- **Internal**: Removed vestigial concrete Project database, keeping trait
- **Internal**: Removed global client state in favor of direct `Client` on server
- **Internal**: Simplified span struct and removed Salsa tracking
- **Internal**: Added logging macros for tracing migration
- **Internal**: Swapped tmux shell script for Rust binary
- **Internal**: Added `system` module to improve reliability of environment discovery tests
- **Internal**: Fixed Django project detection to prioritize LSP workspace folder
- **Internal**: Added `Cargo.lock` and relaxed some dependency version constraints

## [5.2.0a0]

### Added

- Added `html-django` as an alternative Language ID for Django templates
- Added support for Django 5.2.

### Changed

- Bumped Rust toolchain from 1.83 to 1.86
- Bumped PyO3 to 0.24.
- **Internal**: Renamed template parsing crate to `djls-templates`
- **Internal**: Swapped from `tower-lsp` to `tower-lsp-server` for primary LSP framework.

### Removed

- Dropped support for Django 5.0.

## [5.1.0a2]

### Added

- Support for system-wide installation using `uv tool` or `pipx` with automatic Python environment detection and virtualenv discovery

### Changed

- Server no longer requires installation in project virtualenv, including robust Python dependency resolution using `PATH` and `site-packages` detection

## [5.1.0a1]

### Added

- Basic Neovim plugin

## [5.1.0a0]

### Added

- Created basic crate structure:
    - `djls`: Main CLI interface
    - `djls-project`: Django project introspection
    - `djls-server`: LSP server implementation
    - `djls-template-ast`: Template parsing
    - `djls-worker`: Async task management
- Initial Language Server Protocol support:
    - Document synchronization (open, change, close)
    - Basic diagnostics for template syntax
    - Initial completion provider
- Basic Django template parsing foundation and support
- Project introspection capabilities
- Django templatetag completion for apps in a project's `INSTALLED_APPS`

### New Contributors

- Josh Thomas <josh@joshthomas.dev> (maintainer)

[unreleased]: https://github.com/joshuadavidthomas/django-language-server/compare/v6.0.3...HEAD
[5.1.0a0]: https://github.com/joshuadavidthomas/django-language-server/releases/tag/v5.1.0a0
[5.1.0a1]: https://github.com/joshuadavidthomas/django-language-server/releases/tag/v5.1.0a1
[5.1.0a2]: https://github.com/joshuadavidthomas/django-language-server/releases/tag/v5.1.0a2
[5.2.0a0]: https://github.com/joshuadavidthomas/django-language-server/releases/tag/v5.2.0a0
[5.2.0]: https://github.com/joshuadavidthomas/django-language-server/releases/tag/v5.2.0
[5.2.1]: https://github.com/joshuadavidthomas/django-language-server/releases/tag/v5.2.1
[5.2.2]: https://github.com/joshuadavidthomas/django-language-server/releases/tag/v5.2.2
[5.2.3]: https://github.com/joshuadavidthomas/django-language-server/releases/tag/v5.2.3
[5.2.4]: https://github.com/joshuadavidthomas/django-language-server/releases/tag/v5.2.4
[6.0.0]: https://github.com/joshuadavidthomas/django-language-server/releases/tag/v6.0.0
[6.0.1]: https://github.com/joshuadavidthomas/django-language-server/releases/tag/v6.0.1
[6.0.2]: https://github.com/joshuadavidthomas/django-language-server/releases/tag/v6.0.2
[6.0.3]: https://github.com/joshuadavidthomas/django-language-server/releases/tag/v6.0.3
