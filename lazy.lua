return {
  "joshuadavidthomas/django-language-server",
  dependencies = {
    "neovim/nvim-lspconfig",
  },
  config = function(plugin, opts)
    vim.opt.rtp:append(plugin.dir .. "/clients/nvim")
    require("djls").setup(opts)
  end,
}
