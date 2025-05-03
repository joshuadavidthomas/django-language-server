vim.lsp.config["djls"] = {
  cmd = { "djls", "serve" },
  filetypes = { "htmldjango" },
  root_markers = { "manage.py", "pyproject.toml" },
}
vim.lsp.enable("djls")
return {}
