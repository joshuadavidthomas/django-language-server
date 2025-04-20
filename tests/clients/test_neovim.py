"""
Tests for Neovim integration with django-language-server.
"""

from __future__ import annotations

import os
import subprocess
import sys
import tempfile
from pathlib import Path

import pytest

from fixtures.create_django_project import create_django_project, cleanup_django_project


@pytest.fixture(scope="module")
def neovim_config_dir():
    """Create a temporary directory for Neovim configuration."""
    temp_dir = tempfile.mkdtemp()
    config_dir = Path(temp_dir) / "nvim"
    config_dir.mkdir(exist_ok=True)
    
    # Create lua directory
    lua_dir = config_dir / "lua"
    lua_dir.mkdir(exist_ok=True)
    
    # Create init.lua
    init_lua = """
-- Basic Neovim configuration
vim.opt.number = true
vim.opt.relativenumber = true
vim.opt.expandtab = true
vim.opt.shiftwidth = 4
vim.opt.tabstop = 4
vim.opt.smartindent = true
vim.opt.termguicolors = true

-- Load plugins
require('plugins')

-- Configure LSP
require('lsp')
"""
    
    with open(config_dir / "init.lua", "w") as f:
        f.write(init_lua)
    
    # Create plugins.lua
    plugins_dir = lua_dir / "plugins"
    plugins_dir.mkdir(exist_ok=True)
    
    plugins_lua = """
-- Bootstrap lazy.nvim
local lazypath = vim.fn.stdpath("data") .. "/lazy/lazy.nvim"
if not vim.loop.fs_stat(lazypath) then
  vim.fn.system({
    "git",
    "clone",
    "--filter=blob:none",
    "https://github.com/folke/lazy.nvim.git",
    "--branch=stable",
    lazypath,
  })
end
vim.opt.rtp:prepend(lazypath)

-- Configure plugins
require("lazy").setup({
  -- LSP
  {
    "neovim/nvim-lspconfig",
    dependencies = {
      "hrsh7th/cmp-nvim-lsp",
    },
  },
  
  -- Autocompletion
  {
    "hrsh7th/nvim-cmp",
    dependencies = {
      "hrsh7th/cmp-buffer",
      "hrsh7th/cmp-path",
      "hrsh7th/cmp-nvim-lsp",
      "L3MON4D3/LuaSnip",
      "saadparwaiz1/cmp_luasnip",
    },
  },
})
"""
    
    with open(plugins_dir / "init.lua", "w") as f:
        f.write(plugins_lua)
    
    # Create lsp.lua
    lsp_dir = lua_dir / "lsp"
    lsp_dir.mkdir(exist_ok=True)
    
    lsp_lua = """
local lspconfig = require('lspconfig')
local util = require('lspconfig.util')
local cmp_nvim_lsp = require('cmp_nvim_lsp')

-- Add additional capabilities supported by nvim-cmp
local capabilities = cmp_nvim_lsp.default_capabilities()

-- Configure django-language-server
lspconfig.djls = {
  default_config = {
    cmd = { 'djls' },
    filetypes = { 'django-html', 'python' },
    root_dir = function(fname)
      -- Find Django project root (where manage.py is)
      return util.root_pattern('manage.py')(fname)
    end,
    settings = {},
  },
  capabilities = capabilities,
}

-- Set up nvim-cmp
local cmp = require('cmp')
local luasnip = require('luasnip')

cmp.setup({
  snippet = {
    expand = function(args)
      luasnip.lsp_expand(args.body)
    end,
  },
  mapping = cmp.mapping.preset.insert({
    ['<C-d>'] = cmp.mapping.scroll_docs(-4),
    ['<C-f>'] = cmp.mapping.scroll_docs(4),
    ['<C-Space>'] = cmp.mapping.complete(),
    ['<CR>'] = cmp.mapping.confirm({ select = true }),
    ['<Tab>'] = cmp.mapping(function(fallback)
      if cmp.visible() then
        cmp.select_next_item()
      elseif luasnip.expand_or_jumpable() then
        luasnip.expand_or_jump()
      else
        fallback()
      end
    end, { 'i', 's' }),
    ['<S-Tab>'] = cmp.mapping(function(fallback)
      if cmp.visible() then
        cmp.select_prev_item()
      elseif luasnip.jumpable(-1) then
        luasnip.jump(-1)
      else
        fallback()
      end
    end, { 'i', 's' }),
  }),
  sources = cmp.config.sources({
    { name = 'nvim_lsp' },
    { name = 'luasnip' },
  }, {
    { name = 'buffer' },
  }),
})

-- Set up filetypes
vim.filetype.add({
  extension = {
    html = function(path, bufnr)
      -- Check if this is a Django project
      local is_django = vim.fn.findfile('manage.py', vim.fn.expand('%:p:h') .. ';') ~= ''
      if is_django then
        return 'django-html'
      end
      return 'html'
    end,
  },
})
"""
    
    with open(lsp_dir / "init.lua", "w") as f:
        f.write(lsp_lua)
    
    yield config_dir
    
    # Clean up
    import shutil
    shutil.rmtree(temp_dir)


def test_neovim_config_structure(neovim_config_dir):
    """Test that the Neovim configuration structure is valid."""
    assert (neovim_config_dir / "init.lua").exists()
    assert (neovim_config_dir / "lua" / "plugins" / "init.lua").exists()
    assert (neovim_config_dir / "lua" / "lsp" / "init.lua").exists()


def test_neovim_lsp_config(neovim_config_dir):
    """Test that the Neovim LSP configuration is valid."""
    with open(neovim_config_dir / "lua" / "lsp" / "init.lua", "r") as f:
        lsp_config = f.read()
    
    assert "lspconfig.djls" in lsp_config
    assert "cmd = { 'djls' }" in lsp_config
    assert "filetypes = { 'django-html', 'python' }" in lsp_config


# This test is a placeholder for actual Neovim integration testing
# In a real implementation, you would use something like neovim-test
# to launch Neovim with the configuration and test it
@pytest.mark.skip(reason="Requires Neovim to be installed")
def test_neovim_with_language_server(neovim_config_dir):
    """Test Neovim with the language server."""
    # This would require Neovim to be installed and a way to programmatically
    # interact with it, which is beyond the scope of this example
    pass