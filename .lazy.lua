vim.lsp.config["djls"] = {
  cmd = { "uvx", "lsp-devtools", "agent", "--", "target/debug/djls", "serve" },
  cmd_env = { DJLS_DEBUG = "1" },
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

vim.api.nvim_create_user_command("DjlsDumpState", function()
  local clients = vim.lsp.get_active_clients({ name = "djls" })
  if #clients == 0 then
    vim.notify("Django Language Server is not active", vim.log.levels.WARN)
    return
  end

  -- Execute the custom LSP command
  local client = clients[1]
  client.request("workspace/executeCommand", {
    command = "djls/dumpState",
    arguments = {},
  }, function(err, result)
    if err then
      vim.notify("Error dumping state: " .. vim.inspect(err), vim.log.levels.ERROR)
    else
      local message = result and result.message or "State dumped successfully"
      vim.notify(message, vim.log.levels.INFO)
    end
  end)
end, { desc = "Dump Django LS internal state for debugging (requires DJLS_DEBUG=1)" })

return {}
