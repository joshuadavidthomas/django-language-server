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

### `pythonpath`

**Default:** `[]` (empty list)

Additional directories to add to Python's import search path when the inspector process runs. These paths are added to `PYTHONPATH` alongside the project root and any existing `PYTHONPATH` environment variable.

**When to configure:**

- Your project has a non-standard structure where Django code imports from directories outside the project root
- You're working in a monorepo where Django imports shared packages from other directories
- Your project depends on internal libraries in non-standard locations
- You need to make additional packages importable for Django introspection

### `debug`

**Default:** `false`

Enable debug logging for troubleshooting language server issues.

### `diagnostics`

Configure which diagnostics are enabled and their severity levels. Inspired by Ruff's approach with `select` and `ignore` patterns.

**Default:**
```toml
[diagnostics]
select = ["ALL"]  # All diagnostics enabled
ignore = []       # None disabled
```

#### `diagnostics.select`

Diagnostic codes or prefixes to enable. Supports pattern matching:
- `"ALL"` - Enable all diagnostics (default)
- `"S"` - Enable all semantic validation errors (S100-S107)
- `"T"` - Enable all template errors (T100, T900, T901)
- `"S1"` - Enable S100-S199 range
- `"T9"` - Enable T900-T999 range
- Exact codes like `"S100"`, `"T100"`

#### `diagnostics.ignore`

Diagnostic codes or prefixes to disable. Applied after `select`. Supports the same pattern matching as `select`.

#### `diagnostics.severity`

Override severity levels for specific diagnostic codes. Available levels:
- `"error"` (default) - Shows as error
- `"warning"` - Shows as warning
- `"info"` - Shows as information
- `"hint"` - Shows as hint

#### Available Diagnostic Codes

**Template Errors (T-series):**
- `T100` - Parser errors (syntax issues in templates)
- `T900` - IO errors (file read/write issues)
- `T901` - Configuration errors (invalid tagspecs)

**Semantic Validation Errors (S-series):**
- `S100` - Unclosed tag (missing end tag)
- `S101` - Unbalanced structure (mismatched block tags)
- `S102` - Orphaned tag (intermediate tag without parent)
- `S103` - Unmatched block name (e.g., `{% endblock foo %}` doesn't match `{% block bar %}`)
- `S104` - Missing required arguments
- `S105` - Too many arguments
- `S106` - Invalid literal argument
- `S107` - Invalid argument choice

#### Examples

**Disable specific diagnostics:**
```toml
[diagnostics]
select = ["ALL"]
ignore = ["S100", "T100"]  # Disable unclosed tags and parser errors
```

**Enable only semantic errors:**
```toml
[diagnostics]
select = ["S"]  # Only S-series diagnostics
```

**Disable all S100-S109 diagnostics:**
```toml
[diagnostics]
select = ["ALL"]
ignore = ["S10"]  # Disables S100-S109
```

**Change severity levels:**
```toml
[diagnostics]
select = ["ALL"]

[diagnostics.severity]
S101 = "warning"  # Unbalanced structure as warning instead of error
S103 = "hint"     # Unmatched block name as hint
```

**Complex configuration:**
```toml
[diagnostics]
select = ["S", "T"]    # Enable semantic and template errors
ignore = ["S100", "S101"]  # But disable these specific ones

[diagnostics.severity]
S102 = "warning"  # Make orphaned tags a warning
T100 = "hint"     # Make parser errors hints
```

**When to configure:**

- A diagnostic is producing false positives for your use case
- You want to focus on specific types of issues (e.g., only semantic errors)
- You want certain diagnostics to appear as warnings instead of errors
- You're working around a known issue in the diagnostic system

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
  "venv_path": "/path/to/venv",
  "pythonpath": ["/path/to/shared/libs"],
  "diagnostics": {
    "select": ["ALL"],
    "ignore": ["S100", "T100"],
    "severity": {
      "S101": "warning"
    }
  }
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
pythonpath = ["/path/to/shared/libs"]  # Optional: additional import paths

[tool.djls.diagnostics]
select = ["ALL"]
ignore = ["S100", "T100"]

[tool.djls.diagnostics.severity]
S101 = "warning"
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
