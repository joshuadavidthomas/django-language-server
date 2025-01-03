local M = {}

M.defaults = {
  cmd = { "djls", "serve" },
  filetypes = { "django-html", "htmldjango", "python" },
  root_dir = function(fname)
    local util = require("lspconfig.util")
    local root = util.root_pattern("manage.py", "pyproject.toml")(fname)
    return root or vim.fn.getcwd()
  end,
  settings = {},
}

function M.setup(opts)
  opts = vim.tbl_deep_extend("force", M.defaults, opts or {})

  local configs = require("lspconfig.configs")
  if not configs.djls then
    configs.djls = {
      default_config = opts,
    }
  end

  require("lspconfig").djls.setup(opts)
end

return M
