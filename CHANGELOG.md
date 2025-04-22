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
[${version}]: https://github.com/joshuadavidthomas/django-bird/releases/tag/v${version}
-->

## [Unreleased]

### Changed

- **Internal**: Moved task queueing functionality to `djls-server` crate, renamed from `Worker` to `Queue`, and simplified API.

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

[unreleased]: https://github.com/joshuadavidthomas/django-language-server/compare/v5.2.0a0...HEAD
[5.1.0a0]: https://github.com/joshuadavidthomas/django-language-server/releases/tag/v5.1.0a0
[5.1.0a1]: https://github.com/joshuadavidthomas/django-language-server/releases/tag/v5.1.0a1
[5.1.0a2]: https://github.com/joshuadavidthomas/django-language-server/releases/tag/v5.1.0a2

[5.2.0a0]: https://github.com/joshuadavidthomas/django-language-server/releases/tag/v5.2.0a0
