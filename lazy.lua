return {
  "joshuadavidthomas/django-language-server",
  config = function(plugin, opts)
    vim.opt.rtp:append(plugin.dir .. "/editors/nvim")
    require("djls").setup(opts)
  end,
  dependencies = {
    "neovim/nvim-lspconfig",
  },
}
