# Configuration

Django Language Server auto-detects your project configuration in most cases. It reads the `DJANGO_SETTINGS_MODULE` environment variable and searches for standard virtual environment directories (`.venv`, `venv`, `env`, `.env`), active environments, and Python on `PATH`.

**Most users don't need any configuration.** The settings below are for edge cases like non-standard virtual environment locations, editors that don't pass environment variables, or custom template tag definitions.

## Handling environment variables

Environment variables matter when they identify project configuration.

If `django_settings_module` is not configured, the language server reads `DJANGO_SETTINGS_MODULE` from the environment inherited from your editor. Editors launched from desktop environments (app launchers, dock icons) often do not inherit shell variables set in `.bashrc`, `.zshrc`, or similar files.

The most reliable setup is to configure the Django settings module explicitly:

```toml
[tool.djls]
django_settings_module = "myproject.settings"
```

The language server also reads `.env` in the project root, or the file configured by [`env_file`](#env_file), during static project introspection. The file uses the same format as `python-dotenv` and similar tools.

```toml
[tool.djls]
env_file = ".env.local"
```

## Options

### `django_settings_module`

**Default:** `DJANGO_SETTINGS_MODULE` environment variable

Your Django settings module name (e.g., `"myproject.settings"`).

The server uses this to statically introspect your Django project for template tag completions, diagnostics, template navigation, hover, and document links for templates and template libraries. If not explicitly configured, the server reads the `DJANGO_SETTINGS_MODULE` environment variable.

**When to configure:**

- Your editor doesn't pass environment variables to LSP servers (e.g., Sublime Text)
- You need to override the environment variable for a specific workspace

### `django_version`

**Default:** Inferred from project dependencies, ultimately the oldest supported LTS (`"5.2"`).

An explicit override for the Django feature-release line to use when Django source
cannot be found in your project or Python environment. Supported values are
`"5.2"`, `"6.0"`, and `"6.1"`. Usually this setting can be omitted.

```toml
[tool.djls]
django_version = "5.2"
```

DJLS embeds compressed Python source and templates from one pinned point release
per supported line. No Python interpreter, Django installation, or network access
is needed for this fallback. Pins are updated manually with DJLS releases; the
fallback represents the project's feature line, not its exact patch version.
For example, `Django==6.0.3` selects the bundled 6.0 snapshot.

When Django is not installed, selection follows this order:

1. An explicit `django_version` setting.
2. A Django version in the project root's `uv.lock`, `poetry.lock`, `pdm.lock`,
   `Pipfile.lock`, or `pylock.toml` (checked in that order), provided it satisfies
   the dependency declarations read below.
3. The lowest supported feature line allowed by `[project].dependencies` in
   `pyproject.toml`, legacy `[tool.poetry.dependencies]`, `[options] install_requires`
   in `setup.cfg`, `requirements.txt`, and `requirements.in`. Requirements files can
   include other files with `-r`/`--requirement` and `-c`/`--constraint`.
4. The oldest supported LTS when no usable version information is available.

Constraints restrict versions but do not establish that Django is required.
Lock entries must be runtime dependencies whose markers and declarations admit
the same environment. uv locks are traversed from the project at `.` through
runtime dependency edges, including explicitly requested dependency extras;
dev groups and unselected extras are not roots. Locks without an identifiable
project root and ambiguous dependency edges are not used as version evidence.
Lock-wide and package Python restrictions also apply, including pylock's
`requires-python` and `environments`, uv's `requires-python`, and Poetry's
`python-versions`.

Legacy Poetry declarations support exact versions, PEP 440 comparisons, wildcards,
caret/tilde ranges, comma or whitespace conjunctions, `||` alternatives, and conditional tables/arrays using
`markers`, `python`, or `platform`. Optional Poetry dependencies and dependency
groups are not activated. `Pipfile.lock` contributes only its `default` section;
Poetry lock entries must belong to `main` and not be optional; PDM entries must
explicitly belong to `default`. Older entries lacking runtime-group evidence
are ignored. `setup.cfg` supports inline and multiline requirement
values, but does not evaluate interpolation or `file:` directives.

Lock entries using unsupported marker syntax, including pylock's set-valued
`extras` and `dependency_groups`, are ignored rather than activating unknown groups.

Multiple conditional dependencies or locked versions are treated as possible
alternatives, selecting the lowest compatible line rather than using the host's
Python version. Project Python bounds (`project.requires-python`, Poetry's
`python` dependency, and `setup.cfg`'s `python_requires`) exclude incompatible
branches in declarations and lock entries. Optional dependency groups and extras
are not assumed to be active. Direct URL requirements, Poetry Git/URL/path/file
declarations, and versionless pylock source entries do not supply a version,
but retain runtime dependency evidence so accompanying constraints still apply.
`Pipfile` declarations and executable
`setup.py` metadata are not read; use an explicit override if needed.
Named locks such as `pylock.production.toml` and standalone requirements files
such as `requirements/dev.txt` are not automatically selected; requirements files
are read when included from the root `requirements.txt` or `requirements.in`.
Dependency files are reread during project discovery; restart the language server
after changing them if no project reload has occurred.

If known requirements or a locked version have no supported bundle, DJLS warns
and does not substitute an incompatible LTS. An explicit override can opt in to
a different feature line.

Installed or project-local Django always takes precedence, including local
modifications. DJLS never fills gaps in that package with bundled modules.
Without project settings, core tags, filters, and Django's standard loadable
libraries remain available. Unobserved contrib/custom library names are inconclusive,
not reported as definitely absent. Contrib libraries and templates still follow
`INSTALLED_APPS` and template loader configuration.

Bundled sources and templates are read directly from the compressed archive as
needed. Analysis does not require a writable cache. Navigation materializes only
the requested target in the platform's DJLS cache directory
(`~/.cache/djls/django` on Linux), under a content-addressed path. These files are
read-only views: edits to cached files or their editor buffers do not change
analysis. The log reports the selected release line without exposing the source
path. If the cache cannot be written, navigation to bundled files is unavailable,
but analysis still works. Removing the cache causes individual targets to be
recreated on navigation.

### `venv_path`

**Default:** Auto-detects `.venv`, `venv`, `env`, `.env` in the project root, then checks `VIRTUAL_ENV`, `CONDA_PREFIX`, and Python on `PATH`

Absolute path to your project's virtual-environment directory.

The server uses conventional filesystem layouts to infer the selected environment's import roots and locate installed Django apps and their template tags. Automatic discovery can also inspect Python installations found on `PATH`, but the server does not execute Python, project code, or executable lines in `.pth` files. Opaque version-manager shims that do not expose their target as a filesystem symlink require `venv_path` to identify the environment directly.

**When to configure:**

- Your virtual environment is in a non-standard location
- You use an environment outside the project
- Auto-detection fails for your setup

### `pythonpath`

**Default:** `[]` (empty list)

Additional directories to add to the Python import search paths used for static project introspection. These paths are searched alongside the project root when the server resolves settings modules, installed apps, template tag libraries, and Python sources for diagnostics, hover, completions, template navigation, and `{% load %}` document links.

**When to configure:**

- Your project has a non-standard structure where Django code imports from directories outside the project root
- You're working in a monorepo where Django imports shared packages from other directories
- Your project depends on internal libraries in non-standard locations
- You need to make additional packages visible to static introspection

### `env_file`

**Default:** `.env` in the project root (auto-detected, no error if missing)

Path to an environment file (relative to the project root) whose variables are read during static project introspection.

Many Django projects keep local configuration in `.env` files. The language server parses the configured file and records the variables with the project state used by static analysis.

If no `env_file` is configured, the server looks for a `.env` file in the project root automatically. If the file doesn't exist, nothing happens. When `env_file` is set explicitly and the file is missing, a warning is logged.

**When to configure:**

- Your `.env` file has a non-standard name (e.g., `.env.local`, `.env.development`)
- Your `.env` file lives in a subdirectory

### `tagspecs`

Optional manual TagSpecs configuration.

djls primarily derives tag structure and argument rules automatically from Python source code. For edge cases (dynamic tags, unusual registration patterns, complex parsing), you can provide TagSpecs as a fallback.

See [TagSpecs](tagspecs.md).

### `format`

Configure Django template formatting. Formatting is disabled by default and must be enabled explicitly.

```toml
[format]
enabled = true
backend = "djangofmt"
```

**Options:**

- `enabled` — Enable LSP whole-document formatting for Django templates. Default: `false`.
- `backend` — Formatter backend. Currently supported: `"djangofmt"`. Default: `"djangofmt"`.

When enabled, editor "format document" requests are handled by `djangofmt`. DJLS passes through standard editor formatting options when the client provides them, including tab width, spaces vs tabs, trailing whitespace trimming, final newline insertion, and final newline trimming.

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

*Tag Scoping:*

- `S108` - Unknown tag (not found in any active or inactive library)
- `S109` - Unloaded tag (requires `{% load %}` for a specific library)
- `S110` - Ambiguous unloaded tag (defined in multiple active libraries)
- `S118` - Tag exists in a library whose app is not in `INSTALLED_APPS`

*Filter Scoping:*

- `S111` - Unknown filter (not found in any active or inactive library)
- `S112` - Unloaded filter (requires `{% load %}` for a specific library)
- `S113` - Ambiguous unloaded filter (defined in multiple active libraries)
- `S119` - Filter exists in a library whose app is not in `INSTALLED_APPS`

*Expression & Filter Arity:*

- `S114` - Expression syntax error in `{% if %}` / `{% elif %}`
- `S115` - Filter requires an argument but none was provided
- `S116` - Filter does not accept an argument but one was provided

*Tag Argument Validation:*

- `S117` - Tag argument rule violation (e.g., wrong number of arguments, missing required keyword)

*Library Resolution:*

- `S120` - Unknown template tag library (not found among known template tag libraries)
- `S121` - Template tag library exists on the Python search paths, but its app is not in `INSTALLED_APPS`

*Extends Validation:*

- `S122` - `{% extends %}` must be the first tag in the template (no tags or variables before it)
- `S123` - `{% extends %}` cannot appear more than once in a template
- `S124` - Loaded library has unreadable registrations (Hint by default; unrecognized tags and filters are not reported)

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

Pass configuration through your editor's LSP client using `initializationOptions`. This has the highest priority and is useful for workspace-specific overrides. Only fields present in `initializationOptions` override file settings; explicit `false` values and empty lists or maps clear the corresponding file setting.

```json
{
  "django_settings_module": "myproject.settings",
  "venv_path": "/path/to/venv",
  "pythonpath": ["/path/to/shared/libs"],
  "env_file": ".env",
  "format": {
    "enabled": true,
    "backend": "djangofmt"
  },
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
env_file = ".env"  # Optional: path to env file (auto-detects .env by default)

[tool.djls.format]
enabled = true
backend = "djangofmt"

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

- `DJANGO_SETTINGS_MODULE` - Django settings module name
- `VIRTUAL_ENV` - Virtual environment path
- `CONDA_PREFIX` - Active Conda environment prefix
- `PATH` - Fallback discovery of `python3` or `python`

If you're already running Django with these environment variables set, the language server will automatically use them.

If your editor doesn't pass these environment variables to the language server, configure them explicitly using one of the methods above. See [Handling environment variables](#handling-environment-variables) for details on `.env` file support.
