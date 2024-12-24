# django-language-server

A language server for the Django web framework.

> [!CAUTION]
> This project is in early stages. All features are incomplete and missing.

## Features

**Almost none!**

😅

Well, we've achieved the bare minimum of "technically something":

- [x] Template tag autocompletion
    - It works! ...when you type `{%`
    - That's it. That's the feature.

The foundation is solid though:

- [x] Working server architecture
    - [x] Language Server Protocol implementation in Rust
    - [x] Direct Django project interaction through PyO3
    - [x] Single binary distribution with Python packaging
- [x] Custom template parser to support LSP features
    - [x] Basic HTML parsing, including style and script tags
    - [x] Django variables and filters
    - [ ] Django block template tags
        - Early work has been done on an extensible template tag parsing specification (TagSpecs)
- [ ] More actual LSP features (coming soon!... hopefully)
    - We got one! Well, half of one. Only like... dozens more to go? 🎉

Django wasn't built in a day, and neither was a decent Django language server. 😄

## Requirements

An editor that supports the Language Server Protocol (LSP) is required.

The Django Language Server aims to supports all actively maintained versions of Python and Django. Currently this includes:

- Python 3.9, 3.10, 3.11, 3.12, 3.13
- Django 4.2, 5.0, 5.1

See the [Versioning](#versioning) section for details on how this project's version indicates Django compatibility.

## Installation

Install the Django Language Server in your project's environment:

```bash
uv add --dev django-language-server
uv sync

# or

pip install django-language-server
```

The package provides pre-built wheels with the Rust-based LSP server compiled for common platforms. Installing it adds the `djls` command-line tool to your environment.

> [!NOTE]
> The server must currently be installed in each project's environment as it needs to run using the project's Python interpreter to access the correct Django installation and other dependencies.
>
> Global installation is not yet supported as it would run against a global Python environment rather than your project's virtualenv. The server uses [PyO3](https://pyo3.rs) to interact with Django, and we aim to support global installation in the future, allowing the server to detect and use project virtualenvs, but this is a tricky problem involving PyO3 and Python interpreter management.
>
> If you have experience with [PyO3](https://pyo3.rs) or [maturin](https://maturin.rs) and ideas on how to achieve this, please check the [Contributing](#contributing) section below.

## Editor Setup

The Django Language Server works with any editor that supports the Language Server Protocol (LSP). We currently have setup instructions for:

- [Neovim](docs/editor-setup/neovim.md)

Got it working in your editor? [Help us add setup instructions!](#testing-and-documenting-editor-setup)

## Versioning

This project adheres to DjangoVer. For a quick overview of what DjangoVer is, here's an excerpt from Django core developer James Bennett's [Introducing DjangoVer](https://www.b-list.org/weblog/2024/nov/18/djangover/) blog post:

> In DjangoVer, a Django-related package has a version number of the form `DJANGO_MAJOR.DJANGO_FEATURE.PACKAGE_VERSION`, where `DJANGO_MAJOR` and `DJANGO_FEATURE` indicate the most recent feature release series of Django supported by the package, and `PACKAGE_VERSION` begins at zero and increments by one with each release of the package supporting that feature release of Django.

In short, `v5.1.x` means the latest version of Django the Django Language Server would support is 5.1 — so, e.g., versions `v5.1.0`, `v5.1.1`, `v5.1.2`, etc. should all work with Django 5.1.

### Breaking Changes

While DjangoVer doesn't encode API stability in the version number, this project strives to follow Django's standard practice of "deprecate for two releases, then remove" policy for breaking changes. Given this is a language server, breaking changes should primarily affect:

- Configuration options (settings in editor config files)
- CLI commands and arguments
- LSP protocol extensions (custom commands/notifications)

The project will provide deprecation warnings where possible and document breaking changes clearly in release notes. For example, if a configuration option is renamed:

- **`v5.1.0`**: Old option works but logs deprecation warning
- **`v5.1.1`**: Old option still works, continues to show warning
- **`v5.1.2`**: Old option removed, only new option works

## Contributing

The project needs help in several areas:

### Testing and Documenting Editor Setup

The server has only been tested with Neovim. Documentation for setting up the language server in other editors is sorely needed, particularly VS Code. However, any editor that has [LSP client](https://langserver.org/#:~:text=for%20more%20information.-,LSP%20clients,opensesame%2Dextension%2Dlanguage_server,-Community%20Discussion%20Forums) support should work.

If you run into issues setting up the language server:

1. Check the existing documentation in `docs/editors/`
2. [Open an issue](../../issues/new) describing your setup and the problems you're encountering
   - Include your editor and any relevant configuration
   - Share any error messages or unexpected behavior
   - The more details, the better!

If you get it working in your editor:

1. Create a new Markdown file in the `docs/editors/` directory (e.g., `docs/editors/vscode.md`)
2. Include step-by-step setup instructions, any required configuration snippets, and tips for troubleshooting

Your feedback and contributions will help make the setup process smoother for everyone! 🙌

### Feature Requests

The motivation behind writing the server has been to improve the experience of using Django templates. However, it doesn't need to be limited to just that part of Django. In particular, it's easy to imagine how a language server could improve the experience of using the ORM -- imagine diagnostics warning about potential N+1 queries right in your editor!

After getting the basic plumbing of the server and agent in place, it's personally been hard to think of an area of the framework that *wouldn't* benefit from at least some feature of a language server.

All feature requests should ideally start out as a discussion topic, to gather feedback and consensus.

### Development

The project is written in Rust using PyO3 for Python integration:

- LSP server implementation (`crates/djls/`)
- Template parsing and core functionality (`crates/djls-template-ast/`)
- Python integration via PyO3 for Django project introspection

Code contributions are welcome from developers of all backgrounds. Rust expertise is especially valuable for the LSP server and core components.

One significant challenge we're trying to solve is supporting global installation of the language server while still allowing it to detect and use project virtualenvs for Django introspection. Currently, the server must be installed in each project's virtualenv to access the project's Django installation. If you have experience with PyO3 and ideas about how to achieve this, we'd love your help!

Python and Django developers should not be deterred by the Rust codebase - Django expertise is just as valuable. Understanding Django's internals and common development patterns helps inform what features would be most valuable. The Rust components were built by [a simple country CRUD web developer](https://youtu.be/7ij_1SQqbVo?si=hwwPyBjmaOGnvPPI&t=53) learning Rust along the way.

## License

django-language-server is licensed under the Apache License, Version 2.0. See the [`LICENSE`](LICENSE) file for more information.
