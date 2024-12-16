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

You'll need an editor that supports the Language Server Protocol (LSP).

The server supports Django projects running on:

- Python 3.9, 3.10, 3.11, 3.12, 3.13
- Django 4.2, 5.0, 5.1

The aim is to support all actively maintained versions of both Python and Django.

## Installation

The Django Language Server consists of a Rust-based LSP server (`djls`) and a Python agent that runs in your Django project.

The quickest way to get started is to install both the server and agent in your project's environment:

```bash
uv add --dev 'djls[binary]'
uv sync
# or
pip install djls[binary]
```

### Server

You can either build from source using cargo, or install the pre-built binary package from PyPI.

Via cargo:

```bash
cargo install --git https://github.com/joshuadavidthomas/django-language-server
```

Or using the PyPI package via uv or pipx:

```bash
uv tool install djls-binary
# or
pipx install djls-binary
```

### Agent

The agent needs to be installed in your Django project's environment to provide project introspection. Add it to your project's development dependencies:

```bash
uv add --dev djls
uv sync
# or
pip install djls
```

## Editor Setup

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
