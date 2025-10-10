return {
  "joshuadavidthomas/django-language-server",
  config = function()
    vim.notify(
      "lazy.lua spec for django-language-server is deprecated and will be removed in the next release.\n" ..
      "See https://github.com/joshuadavidthomas/django-language-server/blob/main/docs/clients/neovim.md for the new configuration using vim.lsp.config() and vim.lsp.enable().",
      vim.log.levels.WARN
    )
  end,
}
