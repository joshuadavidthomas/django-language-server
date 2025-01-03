return {
  "joshuadavidthomas/django-language-server",
  dependencies = {
    "neovim/nvim-lspconfig",
  },
  config = function(plugin, opts)
    vim.opt.rtp:append(plugin.dir .. "/editors/nvim")
    require("djls").setup(opts)
  end,
}
