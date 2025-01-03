---
title: Neovim
---

# djls.nvim

A Neovim plugin for the Django Language Server.

!!! note

    This plugin is a temporary solution until the project is mature enough to be integrated into [mason.nvim](https://github.com/williamboman/mason.nvim) and [nvim-lspconfig](https://github.com/neovim/nvim-lspconfig).

## Installation

### [lazy.nvim](https://github.com/folke/lazy.nvim)

Minimal setup:

```lua
{
  "joshuadavidthomas/django-language-server",
}
```

The plugin takes advantage of lazy.nvim's spec loading by providing a `lazy.lua` at the root of the repository to handle setup and runtime path configuration automatically. This handles adding the plugin subdirectory to Neovim's runtime path and initializing the LSP client:

```lua
{
  "joshuadavidthomas/django-language-server",
  dependencies = {
    "neovim/nvim-lspconfig",
  },
  config = function(plugin, opts)
    vim.opt.rtp:append(plugin.dir .. "/editors/nvim")
    require("djls").setup(opts)
  end,
}
```

The spec can also serve as a reference for a more detailed installation if needed or desired.

## Configuration

Default configuration options:

```lua
{
  cmd = { "djls", "serve" },
  filetypes = { "django-html", "htmldjango", "python" },
  root_dir = function(fname)
    local util = require("lspconfig.util")
    local root = util.root_pattern("manage.py", "pyproject.toml")(fname)
    return root or vim.fn.getcwd()
  end,
  settings = {},
}
```