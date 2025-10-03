vim.lsp.config["djls"] = {
  cmd = { "uvx", "lsp-devtools", "agent", "--", "./target/debug/djls", "serve" },
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

local M = {}

M.state = {
  wins = {},
  bufs = {},
  timers = { devtools = nil, logs = nil },
  devtools = { job = nil, lines = {} },
}

local function strip_ansi(str)
  return str:gsub("\27%[[0-9;]*m", "")
end

local function create_buffer(opts)
  local buf = vim.api.nvim_create_buf(false, true)
  vim.api.nvim_set_current_buf(buf)
  vim.bo.modifiable = true
  vim.bo.buftype = "nofile"
  vim.bo.filetype = opts.filetype or "text"

  local win = vim.api.nvim_get_current_win()
  vim.wo.number = opts.number ~= nil and opts.number or false
  vim.wo.relativenumber = false
  vim.wo.list = false
  vim.wo.wrap = opts.wrap or false

  return buf, win
end

local function auto_scroll(bufnr, line_count)
  local wins = vim.fn.win_findbuf(bufnr)
  for _, win in ipairs(wins) do
    vim.api.nvim_win_set_cursor(win, { line_count, 0 })
  end
end

function M.ensure_devtools_running()
  if M.state.devtools.job then
    return
  end

  M.state.devtools.lines = {}
  M.state.devtools.job = vim.fn.jobstart("just dev devtools record", {
    stdout_buffered = false,
    on_stdout = function(_, data)
      if not data then
        return
      end
      local cleaned = vim.tbl_map(strip_ansi, data)
      local filtered = vim.tbl_filter(function(line)
        return line ~= ""
      end, cleaned)
      for _, line in ipairs(filtered) do
        table.insert(M.state.devtools.lines, line)
      end
    end,
    on_exit = function()
      M.state.devtools.job = nil
    end,
  })
end

function M.update_devtools(bufnr, start_idx)
  if not vim.api.nvim_buf_is_valid(bufnr) then
    return #M.state.devtools.lines
  end

  local new_lines = vim.list_slice(M.state.devtools.lines, start_idx + 1)
  if #new_lines > 0 then
    vim.api.nvim_buf_set_lines(bufnr, -1, -1, false, new_lines)
    auto_scroll(bufnr, vim.api.nvim_buf_line_count(bufnr))
  end

  return #M.state.devtools.lines
end

function M.update_logs(bufnr)
  local log_file = vim.fn.system("ls -t /tmp/djls.log.* 2>/dev/null | head -1"):gsub("%s+$", "")

  if log_file == "" then
    vim.api.nvim_buf_set_lines(bufnr, 0, -1, false, { "Waiting for server logs..." })
    return
  end

  local lines = vim.fn.readfile(log_file)
  vim.api.nvim_buf_set_lines(bufnr, 0, -1, false, lines)
  auto_scroll(bufnr, #lines)
end

function M.close()
  for _, timer in pairs(M.state.timers) do
    if timer then
      timer:stop()
    end
  end
  M.state.timers = { devtools = nil, logs = nil }

  for _, win in ipairs(M.state.wins) do
    if vim.api.nvim_win_is_valid(win) then
      vim.api.nvim_win_close(win, true)
    end
  end
  for _, buf in ipairs(M.state.bufs) do
    if vim.api.nvim_buf_is_valid(buf) then
      vim.api.nvim_buf_delete(buf, { force = true })
    end
  end

  M.state.wins = {}
  M.state.bufs = {}
end

function M.open()
  M.ensure_devtools_running()

  local main_win = vim.api.nvim_get_current_win()

  vim.cmd("vsplit")
  vim.cmd("wincmd l")
  vim.cmd("vertical resize 80")

  -- Top: lsp-devtools
  local devtools_buf, devtools_win = create_buffer({ filetype = "json" })
  if #M.state.devtools.lines > 0 then
    vim.api.nvim_buf_set_lines(devtools_buf, 0, -1, false, M.state.devtools.lines)
  end

  local last_line_count = #M.state.devtools.lines
  M.state.timers.devtools = vim.uv.new_timer()
  M.state.timers.devtools:start(
    100,
    100,
    vim.schedule_wrap(function()
      last_line_count = M.update_devtools(devtools_buf, last_line_count)
    end)
  )

  table.insert(M.state.wins, devtools_win)
  table.insert(M.state.bufs, devtools_buf)

  -- Bottom: server logs
  vim.cmd("split")
  vim.cmd("wincmd j")
  local log_buf, log_win = create_buffer({ filetype = "log", wrap = true })

  M.update_logs(log_buf)
  M.state.timers.logs = vim.uv.new_timer()
  M.state.timers.logs:start(
    500,
    500,
    vim.schedule_wrap(function()
      M.update_logs(log_buf)
    end)
  )

  table.insert(M.state.wins, log_win)
  table.insert(M.state.bufs, log_buf)

  vim.api.nvim_set_current_win(main_win)
end

function M.toggle()
  if #M.state.wins > 0 then
    M.close()
  else
    M.open()
  end
end

vim.api.nvim_create_autocmd("VimLeavePre", {
  callback = function()
    if M.state.devtools.job then
      vim.fn.jobstop(M.state.devtools.job)
    end
  end,
})

vim.api.nvim_create_user_command("DjlsDebugToggle", M.toggle, {})
vim.keymap.set("n", "<leader>dd", M.toggle, { desc = "Toggle DJLS debug windows" })

return {
  {
    "fei6409/log-highlight.nvim",
    opts = {},
  },
}
