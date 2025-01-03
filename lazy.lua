return {
  "joshuadavidthomas/django-language-server",
  config = function(plugin)
    vim.notify(plugin.dir)
    vim.opt.rtp:append(plugin.dir .. "/editors/nvim")
    require("djls").setup()
  end,
  dependencies = {
    "neovim/nvim-lspconfig",
  },
}
