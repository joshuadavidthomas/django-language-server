---
title: Configuration
---

# Configuration

Django Language Server auto-detects your project configuration in most cases. It reads the `DJANGO_SETTINGS_MODULE` environment variable and searches for standard virtual environment directories (`.venv`, `venv`, `env`, `.env`).

**Most users don't need any configuration.** The settings below are for edge cases like non-standard virtual environment locations, editors that don't pass environment variables, or custom template tag definitions.

## Configuration Options

### `django_settings_module`

**Default:** `DJANGO_SETTINGS_MODULE` environment variable

Your Django settings module path (e.g., `"myproject.settings"`).

The server uses this to introspect your Django project and provide template tag completions, diagnostics, and navigation. If not explicitly configured, the server reads the `DJANGO_SETTINGS_MODULE` environment variable. 

**When to configure:**

- Your editor doesn't pass environment variables to LSP servers (e.g., Sublime Text)
- You need to override the environment variable for a specific workspace

### `venv_path`

**Default:** Auto-detects `.venv`, `venv`, `env`, `.env` in project root, then checks `VIRTUAL_ENV` environment variable

Absolute path to your project's virtual environment directory.

The server needs access to your virtual environment to discover installed Django apps and their template tags.

**When to configure:**

- Your virtual environment is in a non-standard location
- Auto-detection fails for your setup

### `debug`

**Default:** `false`

Enable debug logging for troubleshooting language server issues.

### `tagspecs`

**Default:** `[]`

Define custom template tag specifications for tags not included in Django's built-in or popular third-party libraries.

See the [TagSpecs documentation](../crates/djls-conf/TAGSPECS.md) for detailed schema and examples.

## Configuration Methods

When configuration is needed, the server supports multiple methods in priority order (highest to lowest):

1. **[LSP Client](#lsp-client)** - Editor-specific overrides via initialization options
2. **[Project Files](#project-files)** - Project-specific settings (recommended)
3. **[User File](#user-file)** - Global defaults
4. **[Environment Variables](#environment-variables)** - Automatic fallback

### LSP Client

Pass configuration through your editor's LSP client using `initializationOptions`. This has the highest priority and is useful for workspace-specific overrides.

```json
{
  "django_settings_module": "myproject.settings",
  "venv_path": "/path/to/venv"
}
```

See your editor's documentation for specific instructions on passing initialization options.

### Project Files

Project configuration files are the recommended method for explicit configuration. They keep settings with your project and work consistently across editors.

If you use `pyproject.toml`, add a `[tool.djls]` section:

```toml
[tool.djls]
django_settings_module = "myproject.settings"
venv_path = "/path/to/venv"  # Optional: only if auto-detection fails
```

If you prefer a dedicated config file or don't use `pyproject.toml`, you can use `djls.toml` (same settings, no `[tool.djls]` table).

Files are checked in order: `djls.toml` → `.djls.toml` → `pyproject.toml`

### User File

For settings that apply to all your projects, create a user-level config file at:

- **Linux:** `~/.config/djls/djls.toml`
- **macOS:** `~/Library/Application Support/djls/djls.toml`
- **Windows:** `%APPDATA%\djls\config\djls.toml`

The file uses the same format as `djls.toml` shown above.

### Environment Variables

Django Language Server reads standard Python and Django environment variables:

- `DJANGO_SETTINGS_MODULE` - Django settings module path
- `VIRTUAL_ENV` - Virtual environment path

If you're already running Django with these environment variables set, the language server will automatically use them.

If your editor doesn't pass these environment variables to the language server, configure them explicitly using one of the methods above.
