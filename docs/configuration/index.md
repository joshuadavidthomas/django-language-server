# Configuration

Django Language Server auto-detects your project configuration in most cases. It reads the `DJANGO_SETTINGS_MODULE` environment variable and searches for standard virtual environment directories (`.venv`, `venv`, `env`, `.env`).

**Most users don't need any configuration.** The settings below are for edge cases like non-standard virtual environment locations, editors that don't pass environment variables, or custom template tag definitions.

## Handling environment variables

Django projects commonly read secrets and configuration from environment variables — whether through `os.environ`, `django-environ`, `environs`, `python-decouple`, or similar libraries.

When a required variable is missing, the language server's inspector process fails to initialize Django and you'll see an error like:

> Missing required environment variable: DJANGO_SECRET_KEY. Django settings failed to load because 'DJANGO_SECRET_KEY' is not set in the editor's environment.

This happens because editors launched from desktop environments (app launchers, dock icons) don't inherit shell variables set in `.bashrc`, `.zshrc`, or similar.

### How environment variables reach the inspector

The inspector subprocess inherits the full environment of its parent process (the language server, which inherits from the editor). Variables set in your shell are available automatically **if the editor was launched from that shell**. The [`env_file`](#env_file) option exists for cases where that inheritance doesn't work.

### Recommended setup

If your Django settings depend on environment variables, create a `.env` file in your project root:

```shell
# .env
DJANGO_SECRET_KEY=not-a-real-secret
DATABASE_URL=postgres://localhost/mydb
```

The server automatically detects `.env` in the project root and loads its variables into the inspector process — no configuration needed. This is the same format used by `python-dotenv` and similar tools.

!!! tip
    The values only need to be valid enough for Django to initialize. For a language server, a placeholder `SECRET_KEY` works fine.

If your env file has a different name or location, point to it explicitly:

```toml
[tool.djls]
env_file = ".env.local"
```

### Other approaches

If you prefer not to use an env file:

- **Launch your editor from the terminal** where the variables are already set. Most editors inherit the shell's environment when started this way.
- **Configure your editor** to set environment variables before starting language servers. See your editor's documentation for details.
- **Set `django_settings_module`** in your djls config if the only missing variable is `DJANGO_SETTINGS_MODULE`.

## Options

### `project_model`

**Default:** `"auto"`

Selects how djls builds Django project facts for template directories, active template tag libraries, builtins, and symbols.

Supported values:

- `"auto"` — Build the static project model first. When static facts are known, djls uses them without starting the Python inspector. When static facts are partial or unknown, djls can fall back to the inspector during the transition period.
- `"static"` — Use only the static project model. This avoids the Python inspector, `django.setup()`, and runtime imports. If settings are too dynamic to understand statically, djls degrades conservatively and suppresses diagnostics that would risk false positives.
- `"inspector"` — Use the legacy Python/Django inspector path first. This keeps the runtime behavior used by older releases while the static model rollout continues.

```toml
[tool.djls]
project_model = "auto"
```

Use `"static"` when you want editor behavior that does not run Django. Use `"inspector"` if a dynamic project still needs the runtime fallback while static support catches up.

### `django_settings_module`

**Default:** `DJANGO_SETTINGS_MODULE` environment variable

Your Django settings module path (e.g., `"myproject.settings"`).

The server uses this to build Django project facts for template tag completions, diagnostics, and navigation. If not explicitly configured, the server reads the `DJANGO_SETTINGS_MODULE` environment variable.

**When to configure:**

- Your editor doesn't pass environment variables to LSP servers (e.g., Sublime Text)
- You need to override the environment variable for a specific workspace

### `django_environments`

**Default:** `[]` (empty list)

Path-scoped Django settings modules for monorepos or workspaces with more than one Django project.

```toml
[tool.djls]
pythonpath = ["projects", "apps"]

[[tool.djls.django_environments]]
root = "projects/site1"
django_settings_module = "site1.settings.dev"

[[tool.djls.django_environments]]
root = "projects/site2"
django_settings_module = "site2.settings.dev"
```

Use this when one workspace contains multiple Django settings modules and each applies to a different subtree. Static project model facts stay tied to their environment. Completions can use the union of known facts, while diagnostics only use facts that are safe for the template's environment and suppress unsafe absence claims.

### `django_settings_file_patterns`

**Default:** `[]` (empty list)

Glob patterns for discovering Django settings files and mapping them back to module names through the configured module search paths.

```toml
[tool.djls]
pythonpath = ["projects", "apps"]
django_settings_file_patterns = ["projects/*/settings/dev.py"]
```

Use this for repetitive monorepo layouts where each project follows the same split-settings pattern. Do not configure this together with `django_environments`; explicit environments are clearer when roots do not follow a pattern.

### `venv_path`

**Default:** Auto-detects `.venv`, `venv`, `env`, `.env` in project root, then checks `VIRTUAL_ENV` environment variable

Absolute path to your project's virtual environment directory.

The server needs access to your virtual environment to discover installed Django apps and their template tags.

**When to configure:**

- Your virtual environment is in a non-standard location
- Auto-detection fails for your setup

### `pythonpath`

**Default:** `[]` (empty list)

Additional directories to add to Python's import search path. Static project model discovery uses these paths as module search roots. The inspector also adds them to `PYTHONPATH` alongside the project root and any existing `PYTHONPATH` environment variable.

**When to configure:**

- Your project has a non-standard structure where Django code imports from directories outside the project root
- You're working in a monorepo where Django imports shared packages from other directories
- Your project depends on internal libraries in non-standard locations
- You need to make additional packages importable for Django introspection

### `env_file`

**Default:** `.env` in the project root (auto-detected, no error if missing)

Path to an environment file (relative to the project root) whose variables are injected into the inspector subprocess.

Many Django projects read secrets and configuration from environment variables at settings load time. When the editor is launched from a desktop environment rather than a terminal, those variables aren't available and the inspector fails. This option tells the server to read a `.env` file and forward the variables to the inspector process, so Django settings load correctly without duplicating secrets into config files.

If no `env_file` is configured, the server looks for a `.env` file in the project root automatically. If the file doesn't exist, nothing happens. When `env_file` is set explicitly and the file is missing, a warning is logged.

**When to configure:**

- Your `.env` file has a non-standard name (e.g., `.env.local`, `.env.development`)
- Your `.env` file lives in a subdirectory

!!! note
    `env_file` is only needed by runtime Django settings. In `project_model = "static"`, djls does not execute your settings module, but static extraction may still need `django_settings_module` and `pythonpath` to find the right files.

### `tagspecs`

Optional manual TagSpecs configuration.

djls primarily derives tag structure and argument rules automatically from Python source code. For edge cases (dynamic tags, unusual registration patterns, complex parsing), you can provide TagSpecs as a fallback.

See [TagSpecs](tagspecs.md).

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
- `T100` - Parser errors for malformed template constructs, empty tags, and malformed variable/filter expressions
- `T900` - IO errors (file read/write issues)
- `T901` - Configuration errors

**Semantic Validation Errors (S-series):**

*Block Structure (S100–S103):*

- `S100` - Unclosed tag (missing end tag)
- `S101` - Unbalanced structure (mismatched block tags)
- `S102` - Orphaned tag (intermediate tag without parent)
- `S103` - Unmatched block name (e.g., `{% endblock foo %}` doesn't match `{% block bar %}`)

!!! info "Migration from v5.x"

    In v6.0.0, several diagnostic codes were renumbered for consistency. If you have custom severity settings for the old codes, please update your configuration:

    - `S104` → `S108` (Unknown tag)
    - `S105` → `S109` (Unloaded tag)
    - `S106` → `S111` (Unknown filter)
    - `S107` → `S112` (Unloaded filter)

    Update your `pyproject.toml` or `djls.toml` like this:

    ```toml
    [tool.djls.diagnostics.severity]
    # Old: S104 = "warning"
    S108 = "warning" # New
    ```

*Tag Scoping (requires [project model facts](../template-validation.md#project-model-availability)):*

- `S108` - Unknown tag (not found in any known library or in the Python environment)
- `S109` - Unloaded tag (requires `{% load %}` for a specific library)
- `S110` - Ambiguous unloaded tag (defined in multiple libraries)

*Filter Scoping (requires [project model facts](../template-validation.md#project-model-availability)):*

- `S111` - Unknown filter (not found in any known library or in the Python environment)
- `S112` - Unloaded filter (requires `{% load %}` for a specific library)
- `S113` - Ambiguous unloaded filter (defined in multiple libraries)

*Expression & Filter Arity:*

- `S114` - Expression syntax error in `{% if %}` / `{% elif %}`
- `S115` - Filter requires an argument but none was provided
- `S116` - Filter does not accept an argument but one was provided

*Tag Argument Validation:*

- `S117` - Tag argument rule violation (e.g., wrong number of arguments, missing required keyword)

*Environment-Aware Resolution (requires [project model facts](../template-validation.md#project-model-availability) and [environment scanner](../template-validation.md#environment-scanner)):*

- `S118` - Tag not in `INSTALLED_APPS` (installed package, but Django app not activated)
- `S119` - Filter not in `INSTALLED_APPS` (installed package, but Django app not activated)
- `S120` - Unknown template tag library (not found in the project model or Python environment)
- `S121` - Library not in `INSTALLED_APPS` (installed package, but Django app not activated)

*Extends Validation:*

- `S122` - `{% extends %}` must be the first tag in the template (no tags or variables before it)
- `S123` - `{% extends %}` cannot appear more than once in a template

!!! note "Automatic Validation"

    Template tag validation rules (argument counts, required keywords, block structure) are derived automatically from Python source code via static AST analysis.

    For edge cases where extraction can't infer enough information, you can optionally provide manual [TagSpecs](tagspecs.md) as a fallback.

See [Template Validation](../template-validation.md) for details on how these diagnostics work and their limitations.

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
T100 = "hint"   # But show parser errors as hints
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
  "project_model": "auto",
  "django_settings_module": "myproject.settings",
  "venv_path": "/path/to/venv",
  "pythonpath": ["/path/to/shared/libs"],
  "env_file": ".env",
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
project_model = "auto"
django_settings_module = "myproject.settings"
venv_path = "/path/to/venv"  # Optional: only if auto-detection fails
pythonpath = ["/path/to/shared/libs"]  # Optional: additional import paths
env_file = ".env"  # Optional: path to env file (auto-detects .env by default)

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

If your editor doesn't pass these environment variables to the language server, configure them explicitly using one of the methods above. See [Handling environment variables](#handling-environment-variables) for details on `.env` file support.
