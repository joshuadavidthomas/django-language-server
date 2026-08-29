# Why a language server?

Open a Django template in most editors and you get HTML support at best. The editor highlights the angle brackets, but `{% block %}`, `{% url %}`, and `{{ value|date }}` are plain text to it. It cannot complete a tag name, flag a misspelled filter, or jump to the template behind an `{% extends %}`, because it knows nothing about Django.

Django Language Server adds that knowledge, in whichever editor you use.

## What a language server is

A language server is a separate program that runs next to your editor and answers questions about your code. The two divide the work. The editor owns everything you see: squiggles, completion menus, jumping between files. The server owns the understanding: which template tags your project can use, whether a filter takes an argument, where `base.html` lives.

They communicate over the [Language Server Protocol](https://microsoft.github.io/language-server-protocol/) (LSP), a standard both sides agree on. Without it, Django support would mean a separately maintained plugin for VS Code, another for Neovim, another for Zed. With it, one server works in any editor with an LSP client.

In practice you never interact with the server directly. Your editor starts `djls serve` in the background when you open a project and talks to it as you type.

## What it does for Django templates

The server reads your project: the settings module, `INSTALLED_APPS`, template directories, and the Python source of the template tag libraries it discovers. It never imports or runs your code. From that it knows which tags and filters are available in each template, what arguments they accept, and how templates relate through `{% extends %}` and `{% include %}`.

In the editor, that knowledge becomes:

- Completions for template tags and filters, including third-party and project-local libraries
- Validation as you type: unclosed blocks, unknown or unloaded tags, filters given the wrong arguments
- Navigation: jump to the template behind an `{% extends %}` or `{% include %}`, the parent of a `{% block %}`, or the Python function that registers a tag or filter
- Hover documentation, quick fixes for missing `{% load %}` lines, folding, and document outlines

See [Template Validation](template-validation.md) for how the analysis works and where its limits are.

## Do I need it?

If you edit Django templates in Neovim, VS Code, Zed, Sublime Text, Helix, Emacs, or any other editor with LSP support: yes. Without a language server, these editors treat a Django template as HTML with unusual punctuation.

If you use PyCharm Professional: probably not. JetBrains builds Django template support directly into the IDE, and it already covers most of what this project does. PyCharm's template tooling is a fair preview of what Django Language Server brings to other editors.

If you want template checking without any editor integration, the [`djls check`](cli.md#djls-check) command runs the same validation in a terminal, and a [pre-commit hook](pre-commit.md) runs it on staged templates before each commit.

## Next steps

[Install the server](installation.md), then [set up your editor](clients/index.md). Most projects need no configuration; the [configuration guide](configuration/index.md) covers the exceptions.
