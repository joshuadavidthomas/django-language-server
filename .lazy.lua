vim.lsp.config["djls"] = {
  cmd = { "djls", "serve" },
  filetypes = { "htmldjango" },
  root_markers = { "manage.py", "pyproject.toml" },
}
vim.lsp.enable("djls")

vim.api.nvim_create_autocmd({ "BufRead", "BufNewFile" }, {
  pattern = "*/tests/**/*.html",
  callback = function()
    vim.bo.filetype = "htmldjango"
  end,
})

return {}
