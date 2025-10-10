---
title: Neovim
---

# Neovim

## Requirements

- Neovim 0.11+
- Django Language Server (`djls`) installed on your system. See [Installation](../index.md#installation).

## Configuration

The server can be configured using Neovim's built-in [`vim.lsp.config()`](https://neovim.io/doc/user/lsp.html#lsp-config).

You can define the configuration inline in your `init.lua`:

```lua
-- In init.lua
vim.lsp.config('djls', {
  cmd = { 'djls', 'serve' },
  filetypes = { 'htmldjango', 'html', 'python' },
  root_markers = { 'manage.py', 'pyproject.toml' },
  single_file_support = true,
})

vim.lsp.enable('djls')
```

Or create a dedicated config file in your runtimepath at `lsp/djls.lua`:

```lua
-- In <rtp>/lsp/djls.lua (e.g., ~/.config/nvim/lsp/djls.lua)
return {
  cmd = { 'djls', 'serve' },
  filetypes = { 'htmldjango', 'html', 'python' },
  root_markers = { 'manage.py', 'pyproject.toml' },
  single_file_support = true,
}
```

Then just enable it in your `init.lua`:

```lua
-- In init.lua
vim.lsp.enable('djls')
```

### Django Settings

For Django project settings and other server options, see [Configuration](../configuration.md).

To pass settings via Neovim's LSP client, use `init_options`:

```lua
vim.lsp.config('djls', {
  -- ... basic config from above
  init_options = {
    django_settings_module = "myproject.settings",
    venv_path = "/path/to/venv",
  },
})
```

### File Type Detection

Django templates need the correct filetype for LSP activation. The simplest approach is a pattern match:

```lua
vim.filetype.add({
  pattern = {
    [".*/templates/.*%.html"] = "htmldjango",
  },
})
```

For more intelligent detection that checks if an HTML file is actually in a Django project by walking up the directory tree from the file's location to find Django project markers (`manage.py` or `pyproject.toml` with django):

```lua
-- Detect Django projects
local function is_django_project(path)
  local current = path
  while current ~= "/" do
    -- Check for manage.py
    if vim.fn.filereadable(current .. "/manage.py") == 1 then
      return true
    end

    -- Check for pyproject.toml with django dependency
    -- Note: This is a naive check that just searches for "django" in the file.
    -- A more robust approach would parse the TOML and check dependencies properly.
    local pyproject = current .. "/pyproject.toml"
    if vim.fn.filereadable(pyproject) == 1 then
      local content = vim.fn.readfile(pyproject)
      for _, line in ipairs(content) do
        if line:match("django") then
          return true
        end
      end
    end

    current = vim.fn.fnamemodify(current, ":h")
  end
  return false
end

-- Auto-detect htmldjango filetype for Django projects
vim.api.nvim_create_autocmd({ "BufRead", "BufNewFile" }, {
  pattern = "*.html",
  callback = function(args)
    local file_dir = vim.fn.fnamemodify(args.file, ":p:h")
    if is_django_project(file_dir) then
      vim.bo[args.buf].filetype = "htmldjango"
    end
  end,
})
```

## Using with nvim-lspconfig

[nvim-lspconfig](https://github.com/neovim/nvim-lspconfig) does not currently include a `djls` configuration, but we plan to submit one. Once available, if you have nvim-lspconfig installed, it will provide the configuration automatically and you can skip defining it yourself.

## Troubleshooting

Run `:checkhealth vim.lsp` to diagnose issues.

Common problems:

- **Server not starting**: Verify `djls` is installed (`which djls`)
- **No completions**: Check that `DJANGO_SETTINGS_MODULE` is configured
- **Wrong filetype**: Verify with `:set filetype?` that templates show `htmldjango` or `django-html`
