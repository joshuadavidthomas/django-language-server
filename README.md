# django-language-server

<!-- [[[cog
import subprocess
import cog

from noxfile import DJ_VERSIONS
from noxfile import PY_VERSIONS
from noxfile import display_version

django_versions = [display_version(version) for version in DJ_VERSIONS]

cog.outl("[![PyPI](https://img.shields.io/pypi/v/django-language-server)](https://pypi.org/project/django-language-server/)")
cog.outl("![PyPI - Python Version](https://img.shields.io/pypi/pyversions/django-language-server)")
cog.outl(f"![Django Version](https://img.shields.io/badge/django-{'%20%7C%20'.join(django_versions)}-%2344B78B?labelColor=%23092E20)")
]]] -->
[![PyPI](https://img.shields.io/pypi/v/django-language-server)](https://pypi.org/project/django-language-server/)
![PyPI - Python Version](https://img.shields.io/pypi/pyversions/django-language-server)
![Django Version](https://img.shields.io/badge/django-4.2%20%7C%205.1%20%7C%205.2%20%7C%206.0%20%7C%20main-%2344B78B?labelColor=%23092E20)
<!-- [[[end]]] -->

A language server for the Django web framework.

> [!CAUTION]
> This project is in early stages. ~All~ Most features are incomplete and missing.

## Features

- [x] **Completions** - Template tag autocompletion with snippets
  ![Completions](./docs/assets/autocomplete.png)

- [x] **Diagnostics** - Real-time error checking and validation
  ![Diagnostics](./docs/assets/diagnostics.png)

- [ ] **Go to definition** - Jump to template, block, or variable definitions
- [ ] **Find references** - See where templates and blocks are used
- [ ] **Hover** - View documentation and type info on hover
- [ ] **Rename** - Refactor names across files
- [ ] **Formatting** - Auto-format templates
- [ ] **Code actions** - Quick fixes and refactorings
- [ ] **Document symbols** - Outline view of template structure
- [ ] **Workspace symbols** - Search across all project templates
- [ ] **Signature help** - Parameter hints while typing

## Requirements

An editor that supports the Language Server Protocol (LSP) is required.

The Django Language Server aims to supports all actively maintained versions of Python and Django. Currently this includes:

<!-- [[[cog
import subprocess
import cog

from noxfile import DJ_VERSIONS
from noxfile import PY_VERSIONS
from noxfile import display_version

django_versions = [
    display_version(version) for version in DJ_VERSIONS if version != "main"
]

cog.outl(f"- Python {', '.join(PY_VERSIONS)}")
cog.outl(f"- Django {', '.join(django_versions)}")
]]] -->
- Python 3.9, 3.10, 3.11, 3.12, 3.13
- Django 4.2, 5.1, 5.2, 6.0
<!-- [[[end]]] -->

See the [Versioning](#versioning) section for details on how this project's version indicates Django compatibility.

## Installation

The Django Language Server can be installed using your preferred Python package manager.

For system-wide availability using either `uv` or `pipx`:

```bash
uv tool install django-language-server

# or

pipx install django-language-server
```

Or to try it out in your current project:

```bash
uv add --dev django-language-server
uv sync

# or

pip install django-language-server
```

The package provides pre-built wheels with the Rust-based LSP server compiled for common platforms. Installing it adds the `djls` command-line tool to your environment.

> [!NOTE]
> The server will automatically detect and use your project's Python environment when you open a Django project. It needs access to your project's Django installation and other dependencies, but should be able to find these regardless of where the server itself is installed.
>
> It's recommended to use `uv` or `pipx` to install it system-wide for convenience, but installing in your project's environment will work just as well to give it a test drive around the block.

## Editor Setup

The Django Language Server works with any editor that supports the Language Server Protocol (LSP). We currently have setup instructions for:

- [Neovim](docs/editors/neovim.md)

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

The project is written in Rust with IPC for Python communication. Here is a high-level overview of the project and the various crates:

- Main CLI interface ([`crates/djls/`](./crates/djls/))
- Configuration management ([`crates/djls-conf/`](./crates/djls-conf/))
- Django and Python project introspection ([`crates/djls-project/`](./crates/djls-project/))
- LSP server implementation ([`crates/djls-server/`](./crates/djls-server/))
- Template parsing ([`crates/djls-templates/`](./crates/djls-templates/))
- Workspace and document management ([`crates/djls-workspace/`](./crates/djls-workspace/))

Code contributions are welcome from developers of all backgrounds. Rust expertise is valuable for the LSP server and core components, but Python and Django developers should not be deterred by the Rust codebase - Django expertise is just as valuable. Understanding Django's internals and common development patterns helps inform what features would be most valuable.

So far it's all been built by a [a simple country CRUD web developer](https://youtu.be/7ij_1SQqbVo?si=hwwPyBjmaOGnvPPI&t=53) learning Rust along the way - send help!

## License

django-language-server is licensed under the Apache License, Version 2.0. See the [`LICENSE`](LICENSE) file for more information.

---

django-language-server is not associated with the Django Software Foundation.

Django is a registered trademark of the Django Software Foundation.
