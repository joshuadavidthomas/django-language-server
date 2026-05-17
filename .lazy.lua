vim.lsp.config["djls"] = {
  cmd = { "uvx", "lsp-devtools", "agent", "--", "cargo", "run", "-p", "djls", "--", "serve" },
  cmd_env = {
    RUST_LOG = "djls_server=debug,djls_ide=debug",
  },
  filetypes = { "htmldjango" },
  root_markers = { "manage.py", "pyproject.toml" },
}
vim.lsp.enable("djls")

local dev_group = vim.api.nvim_create_augroup("djls-local-dev", { clear = true })
local format_group = vim.api.nvim_create_augroup("djls-local-format", { clear = true })

vim.api.nvim_create_autocmd({ "BufRead", "BufNewFile" }, {
  group = dev_group,
  pattern = "*/tests/**/*.html",
  callback = function()
    vim.bo.filetype = "htmldjango"
  end,
})

local function format_with_djls(buf)
  vim.lsp.buf.format({
    bufnr = buf,
    name = "djls",
    timeout_ms = 3000,
  })
end

vim.api.nvim_create_autocmd("LspAttach", {
  group = dev_group,
  callback = function(args)
    local client = vim.lsp.get_client_by_id(args.data.client_id)
    local buf = args.buf
    local has_conform = pcall(require, "conform")
    if not client or client.name ~= "djls" or has_conform then
      return
    end
    if not client:supports_method("textDocument/formatting", buf) then
      return
    end
    if vim.b[buf].djls_lsp_formatting then
      return
    end

    vim.b[buf].djls_lsp_formatting = true
    vim.keymap.set("n", "<leader>cf", function()
      format_with_djls(buf)
    end, { buffer = buf, desc = "Format with DJLS" })

    vim.api.nvim_create_autocmd("BufWritePre", {
      group = format_group,
      buffer = buf,
      callback = function()
        format_with_djls(buf)
      end,
    })
  end,
})

local devtools = {
  width = nil,
  record_job = nil,
  record_stdout = {},
  record_stderr = {},
}

function devtools.db_path()
  return vim.fn.getcwd() .. "/djls.db"
end

function devtools.force_terminal_mode()
  local buf = vim.api.nvim_get_current_buf()
  if vim.bo[buf].buftype == "terminal" then
    vim.cmd.startinsert()
  end
end

function devtools.pin_terminal_mode(buf)
  if vim.b[buf].djls_devtools_terminal_pinned then
    return
  end

  vim.b[buf].djls_devtools_terminal_pinned = true
  vim.api.nvim_create_autocmd({ "BufEnter", "WinEnter", "ModeChanged" }, {
    group = dev_group,
    buffer = buf,
    callback = function()
      vim.schedule(devtools.force_terminal_mode)
    end,
  })
end

function devtools.terminal_opts()
  return {
    cwd = vim.fn.getcwd(),
    interactive = true,
    auto_insert = true,
    start_insert = true,
    win = {
      position = "right",
      width = devtools.width or 0.33,
      wo = {
        number = false,
        relativenumber = false,
        signcolumn = "no",
        foldcolumn = "0",
        statuscolumn = "",
        list = false,
        wrap = false,
      },
      on_buf = function(win)
        devtools.pin_terminal_mode(win.buf)
      end,
    },
  }
end

function devtools.get_terminal(create)
  local cmd = { "uvx", "lsp-devtools", "inspect", devtools.db_path() }
  return Snacks.terminal.get(cmd, vim.tbl_extend("force", devtools.terminal_opts(), { create = create }))
end

function devtools.remember_width(win)
  if win and vim.api.nvim_win_is_valid(win) then
    devtools.width = vim.api.nvim_win_get_width(win)
    return
  end

  local terminal = devtools.get_terminal(false)
  if terminal and terminal:valid() then
    devtools.width = vim.api.nvim_win_get_width(terminal.win)
  end
end

function devtools.toggle()
  local terminal = devtools.get_terminal(false)

  if terminal and terminal:valid() then
    devtools.remember_width(terminal.win)
    terminal:hide()
    return
  end

  if not terminal then
    terminal = devtools.get_terminal(true)
    return
  end

  if devtools.width then
    terminal.opts.width = devtools.width
  end
  terminal:show()
  terminal:focus()
end

function devtools.ensure_recording()
  if devtools.record_job then
    return
  end

  local db_path = devtools.db_path()
  local record_cmd = { "uvx", "lsp-devtools", "record", "--to-sqlite", db_path }

  vim.fn.delete(db_path)
  devtools.record_stdout = {}
  devtools.record_stderr = {}

  devtools.record_job = vim.fn.jobstart(record_cmd, {
    cwd = vim.fn.getcwd(),
    stdout_buffered = true,
    stderr_buffered = true,
    on_stdout = function(_, data)
      if data and #data > 0 then
        devtools.record_stdout = data
      end
    end,
    on_stderr = function(_, data)
      if data and #data > 0 then
        devtools.record_stderr = data
      end
    end,
    on_exit = function(_, code)
      devtools.record_job = nil
      if code ~= 0 then
        local output = vim.tbl_filter(function(line)
          return line ~= ""
        end, vim.list_extend(vim.deepcopy(devtools.record_stderr), devtools.record_stdout))
        vim.notify(
          "lsp-devtools record exited with code " .. code .. "\n" .. table.concat(output, "\n"),
          vim.log.levels.ERROR
        )
      end
    end,
  })

  if devtools.record_job <= 0 then
    vim.notify("Failed to start lsp-devtools record", vim.log.levels.ERROR)
    devtools.record_job = nil
  end
end

function devtools.stop_recording()
  if devtools.record_job then
    vim.fn.jobstop(devtools.record_job)
  end
end

vim.api.nvim_create_autocmd("WinResized", {
  group = dev_group,
  callback = function()
    local terminal = devtools.get_terminal(false)
    if terminal and terminal:valid() then
      devtools.remember_width(terminal.win)
    end
  end,
})

vim.api.nvim_create_autocmd("WinClosed", {
  group = dev_group,
  callback = function(args)
    local terminal = devtools.get_terminal(false)
    if terminal and terminal.win and tonumber(args.match) == terminal.win then
      devtools.remember_width(terminal.win)
    end
  end,
})

vim.api.nvim_create_autocmd("VimLeavePre", {
  group = dev_group,
  callback = devtools.stop_recording,
})

vim.api.nvim_create_user_command("DjlsDebugToggle", devtools.toggle, {})
vim.keymap.set({ "n", "t" }, "<leader>dd", devtools.toggle, { desc = "Toggle DJLS devtools inspector" })
vim.schedule(devtools.ensure_recording)

return {
  {
    "folke/snacks.nvim",
    lazy = false,
    priority = 1000,
    opts = {
      terminal = { enabled = true },
    },
  },
  {
    "stevearc/conform.nvim",
    optional = true,
    opts = {
      default_format_opts = {
        lsp_format = "fallback",
      },
      formatters_by_ft = {
        htmldjango = {},
      },
    },
  },
}
