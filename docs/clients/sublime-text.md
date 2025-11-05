# Sublime Text

## Requirements

- Sublime Text 4 build 4132+
- [Package Control](https://packagecontrol.io/installation)
- The [LSP](https://github.com/sublimelsp/LSP/) client package (install via Package Control)
- Django template syntax support - install [Djaneiro](https://github.com/squ1b3r/Djaneiro) via Package Control to get the `text.html.django` filetype (other Django syntax packages like [Django Syntax](https://packagecontrol.io/packages/Django%20Syntax) also work, but may require adjusting the `selector` value)

## Configuration

To use Django Language Server with Sublime Text, you'll need to configure two things: the LSP client settings to enable and run the server, and your Python environment and Django project settings so the server can introspect your project.

### LSP client

Add the following to your LSP settings (`Preferences > Package Settings > LSP > Settings`):

```json
{
    "clients": {
        "djls": {
            "enabled": true,
            "command": [
                "/path/to/djls",
                "serve"
            ],
            "selector": "text.html.django",
        },
    },
}
```

Replace `/path/to/djls` with the actual path to your `djls` installation:

- If installed via `uv tool install` or `pipx`: typically `~/.local/bin/djls`
- If installed via `cargo install`: typically `~/.cargo/bin/djls`
- Run `which djls` in your terminal to find the exact path

> [!NOTE]
> GUI applications on Linux and macOS don't inherit your shell's PATH, so using the full path ensures Sublime Text can find `djls`. If you encounter issues, see the [LSP documentation on PATH configuration](https://lsp.sublimetext.io/troubleshooting/#updating-the-path-used-by-lsp-servers).

### Python/Django

The language server requires two settings to provide full functionality:

- `django_settings_module` **(required)**: Your Django settings module (e.g., `"myproject.settings"`)
- `venv_path` **(optional)**: Path to your virtual environment

Without these, the language server can't introspect your Django project and will only provide basic built-in template tag completions. The language server attempts to auto-detect these settings from the `DJANGO_SETTINGS_MODULE` and `VIRTUAL_ENV` environment variables, and will also search for standard virtual environment directories (`.venv`, `venv`, `env`, `.env`) in your project root.

However, **Sublime Text launches language servers as subprocesses that don't inherit your terminal's environment**, so you must explicitly configure at least `django_settings_module`, and likely `venv_path` as well unless your venv uses a standard name.

#### Configuration methods

##### Tool specific file

Add a `[tool.djls]` section to your `pyproject.toml`:

```toml
[tool.djls]
django_settings_module = "myproject.settings"
venv_path = "/path/to/venv"
```

You can also create a dedicated `djls.toml` or `.djls.toml` file in your project root if you prefer to keep tool-specific configuration separate.

##### Sublime project file

Both settings can also be configured via their corresponding environment variables (`DJANGO_SETTINGS_MODULE` and `VIRTUAL_ENV`) in your `.sublime-project` file:

```json
{
    "folders": [{"path": "."}],
    "settings": {
        "LSP": {
            "djls": {
                "env": {
                    "DJANGO_SETTINGS_MODULE": "myproject.settings",
                    "VIRTUAL_ENV": "/path/to/venv"
                }
            }
        }
    }
}
```
