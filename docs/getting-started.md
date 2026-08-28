# Getting started

This guide goes from nothing to a working editor setup. It follows the recommended path; every other installation method is in the [Installation](installation.md) reference.

## Prerequisites

- A Django project on a supported Python and Django version (see [Versioning](versioning.md))
- An editor with LSP support

## 1. Install the server

Install `djls` as a global tool with [uv](https://docs.astral.sh/uv/) or [pipx](https://pipx.pypa.io/):

```bash
uv tool install django-language-server
# or: pipx install django-language-server
```

Check that the binary is on your `PATH`:

```bash
djls --version
```

The server runs as its own program, outside your project's virtual environment. It discovers and reads your project's environment on its own, so one global install serves every project.

!!! note "No install at all"

    Most editors can run the server on demand with `uvx --from django-language-server djls serve` as the server command, no installation required.

## 2. Set up your editor

- [VS Code](clients/vscode.md): install the extension from the marketplace
- [Neovim](clients/neovim.md): enable the built-in LSP configuration
- [Zed](clients/zed.md): install the Django extension, which can download the server itself
- [Sublime Text](clients/sublime-text.md): configure the LSP package

Using something else? Any editor with an LSP client can run `djls serve`; see [Editor setup](clients/index.md).

## 3. Open a template

Open a template file from your project and try it out:

- Type `{% lo` and you should be offered `load` as a completion.
- Hover a built-in tag like `{% block %}` and you should see its documentation.
- Type `{% block content %}` without a matching `{% endblock %}` and an unclosed-tag diagnostic should appear.

If completions and diagnostics show up, the rest (navigation, hover, quick fixes) is working from the same analysis.

## If nothing happens

The two usual causes:

**The editor doesn't treat the file as a Django template.** The server only attaches to files your editor identifies as Django templates. Plain `.html` files often need a filetype or syntax rule; each editor page shows how to set one up.

**The server can't find your project's settings or environment.** Both are auto-detected in standard layouts: a `.venv` next to the project, `DJANGO_SETTINGS_MODULE` in the environment. If your project differs, set `django_settings_module` explicitly; see [Configuration](configuration/index.md).

Beyond that, each editor page has its own troubleshooting notes, and setting [`debug = true`](configuration/index.md#debug) in the configuration turns on server logging.
