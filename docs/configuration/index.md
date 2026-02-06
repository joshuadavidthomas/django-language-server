# Configuration

Django Language Server auto-detects your project configuration in most cases. It reads the `DJANGO_SETTINGS_MODULE` environment variable and searches for standard virtual environment directories (`.venv`, `venv`, `env`, `.env`).

**Most users don't need any configuration.** The settings below are for edge cases like non-standard virtual environment locations, editors that don't pass environment variables, or custom template tag definitions.

!!! tip "Understanding Template Validation"

    For details on how djls validates templates, what it can and cannot detect, and how inspector availability affects diagnostics, see [Template Validation](../template-validation.md).

## Options

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

Configure diagnostic severity levels. All diagnostics are enabled by default at "error" severity level.

**Default:** All diagnostics shown as errors

#### `diagnostics.severity`

Map diagnostic codes or prefixes to severity levels. Supports:
- **Exact codes:** `"S100"`, `"T100"`
- **Prefixes:** `"S"` (all S-series), `"T"` (all T-series), `"S1"` (S100-S199), `"T9"` (T900-T999)
- **Resolution:** More specific patterns override less specific (exact > longer prefix > shorter prefix)

**Available severity levels:**
- `"off"` - Disable diagnostic completely
- `"hint"` - Show as subtle hint
- `"info"` - Show as information
- `"warning"` - Show as warning
- `"error"` - Show as error (default)

#### Available diagnostic codes

**Template Errors (T-series):**

| Code | Error | Description |
|------|-------|-------------|
| `T100` | Parser error | Syntax issues in templates (unclosed tags, malformed expressions) |
| `T900` | IO error | File read/write issues |
| `T901` | Configuration error | Invalid tagspecs or configuration |

**Semantic Validation Errors (S-series):**

Semantic errors are grouped by validation category. Some errors depend on [inspector availability](../template-validation.md#inspector-availability) and may be suppressed when the inspector cannot query your Django project.

##### Block Structure (S100-S107)

These errors detect structural issues in template block tags.

| Code | Error | Description | Typical Fix |
|------|-------|-------------|-------------|
| `S100` | Unclosed tag | Block tag missing its end tag | Add `{% endif %}`, `{% endfor %}`, etc. |
| `S101` | Unbalanced structure | Mismatched block tags | Fix tag nesting order |
| `S102` | Orphaned tag | Intermediate tag without parent block | Move `{% else %}` inside `{% if %}` block |
| `S103` | Unmatched block name | End tag name doesn't match opening | Fix `{% endblock name %}` to match `{% block name %}` |
| `S104` | Missing required arguments | Tag requires arguments not provided | Add required arguments per tag documentation |
| `S105` | Too many arguments | Tag given more arguments than expected | Remove extra arguments |
| `S106` | Invalid literal argument | Argument value not recognized | Use valid literal value |
| `S107` | Invalid argument choice | Argument not in allowed choices | Use one of the allowed values |

##### Tag Scoping (S108-S110)

These errors validate `{% load %}` requirements for template tags. They depend on inspector availability.

| Code | Error | Description | Typical Fix | Suppression |
|------|-------|-------------|-------------|-------------|
| `S108` | Unknown tag | Tag not in Django's registry | Check spelling, install library, or define [TagSpec](tagspecs.md) | Suppressed when inspector unavailable |
| `S109` | Unloaded library tag | Tag requires `{% load %}` | Add `{% load library_name %}` before usage | Suppressed when inspector unavailable |
| `S110` | Ambiguous unloaded tag | Tag exists in multiple libraries | Load one of the listed libraries | Suppressed when inspector unavailable |

##### Filter Scoping (S111-S113)

These errors validate `{% load %}` requirements for template filters. They depend on inspector availability.

| Code | Error | Description | Typical Fix | Suppression |
|------|-------|-------------|-------------|-------------|
| `S111` | Unknown filter | Filter not in Django's registry | Check spelling, install library | Suppressed when inspector unavailable |
| `S112` | Unloaded library filter | Filter requires `{% load %}` | Add `{% load library_name %}` before usage | Suppressed when inspector unavailable |
| `S113` | Ambiguous unloaded filter | Filter exists in multiple libraries | Load one of the listed libraries | Suppressed when inspector unavailable |

##### Expression & Filter Arity (S114-S116)

These errors validate expression syntax and filter argument requirements.

| Code | Error | Description | Typical Fix | Suppression |
|------|-------|-------------|-------------|-------------|
| `S114` | Expression syntax error | Invalid `{% if %}` expression | Fix operator/operand syntax | Never suppressed |
| `S115` | Filter missing argument | Filter requires an argument | Add argument: `{{ x\|filter:arg }}` | Suppressed when inspector unavailable or arity unknown |
| `S116` | Filter unexpected argument | Filter doesn't accept arguments | Remove argument: `{{ x\|filter }}` | Suppressed when inspector unavailable or arity unknown |

!!! note "Filter Arity Extraction"

    S115 and S116 depend on djls extracting filter arity (argument requirements) from Python source. If extraction fails or the filter's signature is ambiguous, these diagnostics are skipped rather than guessing. This is expected behavior, not a bug.

#### Examples

**Disable specific diagnostics:**
```toml
[diagnostics.severity]
S100 = "off"  # Don't show unclosed tag errors
T100 = "off"  # Don't show parser errors
```

**Disable all template errors:**
```toml
[diagnostics.severity]
"T" = "off"  # Prefix matches all T-series
```

**Disable with specific override:**
```toml
[diagnostics.severity]
"T" = "off"     # Disable all template errors
T100 = "hint"   # But show parser errors as hints (specific overrides prefix)
```

**Make all semantic errors warnings:**
```toml
[diagnostics.severity]
"S" = "warning"  # All semantic errors as warnings
```

**Complex configuration:**
```toml
[diagnostics.severity]
# Disable all template errors
"T" = "off"

# But show parser errors as hints
T100 = "hint"

# Make all semantic errors warnings
"S" = "warning"

# Except completely disable unclosed tags
S100 = "off"

# And make S10x (S100-S109) info level
"S10" = "info"
```

**Resolution order example:**
```toml
[diagnostics.severity]
"S" = "warning"    # Base: all S-series are warnings
"S1" = "info"      # Override: S100-S199 are info
S100 = "off"       # Override: S100 is off

# Results:
# S100 → off (exact match)
# S101 → info ("S1" prefix)
# S200 → warning ("S" prefix)
```

**When to configure:**

- Disable false positives: Set problematic diagnostics to `"off"`
- Gradual adoption: Downgrade to `"warning"` or `"hint"` during migration
- Focus attention: Disable entire categories with prefix patterns
- Fine-tune experience: Mix prefix patterns with specific overrides

### `tagspecs`

**Default:** Empty (no custom tagspecs)

Define custom template tag specifications for tags not included in Django's built-in or popular third-party libraries.

!!! warning "Deprecation Warning"

    The v0.4.0 flat `[[tagspecs]]` format is deprecated and will be removed in v6.2.0.

    Please migrate to the [v0.6.0 hierarchical format](./tagspecs.md#migration-from-v040).

See the [TagSpecs documentation](./tagspecs.md) for detailed schema and examples.

## Methods

When configuration is needed, the server supports multiple methods in priority order (highest to lowest):

1. **[LSP Client](#lsp-client)** - Editor-specific overrides via initialization options
2. **[Project Files](#project-files)** - Project-specific settings (recommended)
3. **[User File](#user-file)** - Global defaults
4. **[Environment Variables](#environment-variables)** - Automatic fallback

### LSP client

Pass configuration through your editor's LSP client using `initializationOptions`. This has the highest priority and is useful for workspace-specific overrides.

```json
{
  "django_settings_module": "myproject.settings",
  "venv_path": "/path/to/venv",
  "pythonpath": ["/path/to/shared/libs"],
  "diagnostics": {
    "severity": {
      "S100": "off",
      "S101": "warning",
      "T": "off",
      "T100": "hint"
    }
  }
}
```

See your editor's documentation for specific instructions on passing initialization options.

### Project files

Project configuration files are the recommended method for explicit configuration. They keep settings with your project and work consistently across editors.

If you use `pyproject.toml`, add a `[tool.djls]` section:

```toml
[tool.djls]
django_settings_module = "myproject.settings"
venv_path = "/path/to/venv"  # Optional: only if auto-detection fails
pythonpath = ["/path/to/shared/libs"]  # Optional: additional import paths

[tool.djls.diagnostics.severity]
S100 = "off"
S101 = "warning"
"T" = "off"
T100 = "hint"
```

If you prefer a dedicated config file or don't use `pyproject.toml`, you can use `djls.toml` (same settings, no `[tool.djls]` table).

Files are checked in order: `djls.toml` → `.djls.toml` → `pyproject.toml`

### User file

For settings that apply to all your projects, create a user-level config file at:

- **Linux:** `~/.config/djls/djls.toml`
- **macOS:** `~/Library/Application Support/djls/djls.toml`
- **Windows:** `%APPDATA%\djls\config\djls.toml`

The file uses the same format as `djls.toml` shown above.

### Environment variables

Django Language Server reads standard Python and Django environment variables:

- `DJANGO_SETTINGS_MODULE` - Django settings module path
- `VIRTUAL_ENV` - Virtual environment path

If you're already running Django with these environment variables set, the language server will automatically use them.

If your editor doesn't pass these environment variables to the language server, configure them explicitly using one of the methods above.
