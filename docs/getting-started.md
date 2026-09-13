# Getting started

Follow these steps to install the server, set up your editor, and check that it works. For other installation methods, see [Installation](installation.md).

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

The server runs outside your project's virtual environment. It discovers each project's environment separately, so one global install can serve multiple projects.

!!! note "No install at all"

    Most editors can run the server on demand with `uvx --from django-language-server djls serve` as the server command, no installation required.

## 2. Set up your editor

- [VS Code](clients/vscode.md): install the extension from the marketplace
- [Neovim](clients/neovim.md): configure and enable `djls` with Neovim's built-in LSP client
- [Zed](clients/zed.md): install the Django extension, which can download the server itself
- [Sublime Text](clients/sublime-text.md): configure the LSP package

Any other editor with an LSP client can run `djls serve`; see [Editor setup](clients/index.md).

## 3. Open a template

Open a template file from your project and try it out:

- Type `{% lo` and you should get `load` as a completion.
- Hover a built-in tag like `{% block %}` and you should see its documentation.
- Type `{% block content %}` without a matching `{% endblock %}` and an unclosed-tag diagnostic should appear.

If completions and diagnostics show up, the rest (navigation, hover, quick fixes) comes from the same analysis.

## If nothing happens

The two usual causes:

**The editor doesn't treat the file as a Django template.** The server only attaches to files your editor identifies as Django templates. Plain `.html` files often need a filetype or syntax rule; each editor page shows how to set one up.

**The server can't find your project's settings or environment.** It auto-detects standard layouts, including a `.venv` in the project root and `DJANGO_SETTINGS_MODULE` in the editor's environment. For other layouts, set `django_settings_module` or `venv_path` explicitly; see [Configuration](configuration/index.md).

Beyond that, each editor page has its own troubleshooting notes.
