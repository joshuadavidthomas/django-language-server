# django-language-server

A language server for the Django web framework.

> [!CAUTION]
> This project is in early stages. All features are incomplete and missing.

## Features

**None.**

😅

However, the foundation has been laid:

✅ Working server architecture

- ✅ Server implementing the Language Server Protocol written in Rust
- ✅ Python agent running as a persistent process within the Django project's virtualenv
- ✅ Server-agent communication via Protocol Buffers

✅ Custom template parser to support LSP features

- ✅ Basic HTML parsing, including style and script tags
- ✅ Django variables and filters
- ❌ Django block template tags
  - Early work on extensible template tag parsing specification (TagSpecs)

❌ Actual LSP features (coming soon!... hopefully)

## Requirements

An editor that supports the Language Server Protocol (LSP) is required.

The Django Language Server aims to supports all actively maintained versions of Python and Django. Currently this includes:

- Python 3.9, 3.10, 3.11, 3.12, 3.13
- Django 4.2, 5.0, 5.1

See the [Versioning](#versioning) section for details on how version numbers indicate Django compatibility.

## Installation

The Django Language Server consists of two main components:

- **An LSP server**: Rust binary `djls`, distributed through the Python package `djls-server`
- **A Python agent**: `djls-agent` package that runs in your Django project

Both will need to be available in your Django project in order to function.

The quickest way to get started is to install both the server and agent in your project's environment:

```bash
uv add --dev 'djls[server]'
uv sync

# or

pip install djls[server]
```

> [!NOTE]
> The server should be installed globally on your development machine. The quick-start method above will install the server in each project's environment and is only intended for trying things out. See the [Server](#server) section below for details.

### Server

You can install the pre-built binary package from PyPI, or build from source using cargo.

The server binary is published to PyPI as `djls-server` for easy installation via uv or pipx:

```bash
uv tool install djls-server

# or

pipx install djls-server
```

If you have a Rust toolchain available and prefer to build from source, you can install via cargo:

```bash
cargo install --git https://github.com/joshuadavidthomas/django-language-server
```

### Agent

The agent needs to be installed in your Django project's environment to provide project introspection.

The agent is published to PyPI as `djls-agent` and should be added to your project's development dependencies:

```bash
uv add --dev djls-agent
uv sync

# or

pip install djls-agent
```

## Editor Setup

The Django Language Server should work with any editor that supports the Language Server Protocol (LSP). Got it working in your editor? [Help us add setup instructions!](#testing-and-documenting-editor-setup)

- [Neovim](#neovim)

### Neovim

Using [lazy.nvim](https://github.com/folke/lazy.nvim) and [nvim-lspconfig](https://github.com/neovim/nvim-lspconfig):

```lua
  {
    "neovim/nvim-lspconfig",
    opts = {
      servers = {
        djls = {},
      },
      setup = {
        djls = function(_, opts)
          local configs = require("lspconfig.configs")
          local util = require("lspconfig.util")

          if not configs.djls then
            configs.djls = {
              default_config = {
                cmd = { "djls", "serve" },
                filetypes = { "htmldjango" },
                root_dir = function(fname)
                  local root = util.root_pattern("manage.py", "pyproject.toml")(fname)
                  vim.notify("LSP root dir: " .. (root or "nil"))
                  return root or vim.fn.getcwd()
                end,
                handlers = {
                  ["window/logMessage"] = function(_, params, _)
                    local message_type = {
                      [1] = vim.log.levels.ERROR,
                      [2] = vim.log.levels.WARN,
                      [3] = vim.log.levels.INFO,
                      [4] = vim.log.levels.DEBUG,
                    }
                    vim.notify(params.message, message_type[params.type], {
                      title = "djls",
                    })
                  end,
                },
                on_attach = function(client, bufnr)
                  vim.notify("djls attached to buffer: " .. bufnr)
                end,
              },
            }
          end
          require("lspconfig").djls.setup({})
        end,
      },
    },
  },
```

> [!NOTE]
> This configuration is copied straight from my Neovim setup and includes a logging setup that sends LSP messages to Neovim's notification system. You can remove all the references to `vim.notify` if you don't care about this functionality.

## Versioning

This project adheres to DjangoVer. For a quick overview of what DjangoVer is, here's an excerpt from Django core developer James Bennett's [Introducing DjangoVer](https://www.b-list.org/weblog/2024/nov/18/djangover/) blog post:

> In DjangoVer, a Django-related package has a version number of the form `DJANGO_MAJOR.DJANGO_FEATURE.PACKAGE_VERSION`, where `DJANGO_MAJOR` and `DJANGO_FEATURE` indicate the most recent feature release series of Django supported by the package, and `PACKAGE_VERSION` begins at zero and increments by one with each release of the package supporting that feature release of Django.

In short, `v5.1.x` means the latest version of Django the Django Language Server would support is 5.1 — so, e.g., versions `v5.1.0`, `v5.1.1`, `v5.1.2`, etc. should all work with Django 5.1.

At this moment, all components of the Django Language Server (the `djls` binary, the `djls-agent` agent package on PyPI, and the `djls-binary` binary distribution package on PyPI) will share the same version number. When a new version is released, all packages are updated together regardless of which component triggered the release.

### Breaking Changes

While DjangoVer doesn't encode API stability in the version number, this project strives to follow Django's standard practice of "deprecate for two releases, then remove" policy for breaking changes. Given this is a language server, breaking changes should primarily affect:

- Configuration options (settings in editor config files)
- CLI commands and arguments
- LSP protocol extensions (custom commands/notifications)

The library will provide deprecation warnings where possible and document breaking changes clearly in release notes. For example, if a configuration option is renamed:

- v5.1.0: Old option works but logs deprecation warning
- v5.1.1: Old option still works, continues to show warning
- v5.1.2: Old option removed, only new option works

## Contributing

The project needs help in several areas:

### Testing and Documenting Editor Setup

The server has only been tested with Neovim. Documentation for setting up the language server in other editors is sorely needed, particularly VS Code. However, any editor that has [LSP client](https://langserver.org/#:~:text=for%20more%20information.-,LSP%20clients,opensesame%2Dextension%2Dlanguage_server,-Community%20Discussion%20Forums) support would be welcome.

If you get it working in your editor, please open a PR with the setup instructions.

### Feature Requests

The motivation behind writing the server has been to improve the experience of using Django templates. However, it doesn't need to be limited to just that part of Django. In particular, it's easy to imagine how a language server could improve the experience of using the ORM -- imagine diagnostics warning about potential N+1 queries right in your editor!

After getting the basic plumbing of the server and agent in place, it's personally been hard to think of an area of the framework that *wouldn't* benefit from at least some feature of a language server.

All feature requests should ideally start out as a discussion topic, to gather feedback and consensus.

### Development

The project consists of both Rust and Python components:

- Rust: LSP server, template parsing, and core functionality (`crates/`)
- Python: Django project and environment introspection agent (`packages/`)

Code contributions are welcome from developers of all backgrounds. Rust expertise is especially valuable for the LSP server and core components.

Python and Django developers should not be deterred by the Rust codebase - Django expertise is just as valuable. The Rust components were built by [a simple country CRUD web developer](https://youtu.be/7ij_1SQqbVo?si=hwwPyBjmaOGnvPPI&t=53) learning Rust along the way.

## License

django-language-server is licensed under the MIT license. See the [`LICENSE`](LICENSE) file for more information.
