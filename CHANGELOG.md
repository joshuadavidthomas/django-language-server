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

### Fixed

- Fixed `djls check` with no arguments silently reading stdin instead of discovering template files. Stdin is now triggered explicitly by passing `-` as a path.

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

- TagSpecs v0.4.0 flat format (will be removed in v6.0.2). [migration guide here](docs/configuration/tagspecs.md#migration-from-v040)

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

[unreleased]: https://github.com/joshuadavidthomas/django-language-server/compare/v6.0.1...HEAD
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
