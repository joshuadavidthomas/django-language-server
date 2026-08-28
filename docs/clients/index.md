# Editor setup

Django Language Server works with any editor that supports the Language Server Protocol (LSP). Setup guides exist for:

- [Neovim](neovim.md)
- [Sublime Text](sublime-text.md)
- [VS Code](vscode.md)
- [Zed](zed.md)

## Using another editor

Any editor with an [LSP client](https://langserver.org/) can use the server: configure the client to run `djls serve` for Django template files. The [getting started guide](../getting-started.md) covers installing the server and verifying the setup works.

If you get it working in your editor, documentation for other editors is sorely needed:

1. Create a new Markdown file in the [`docs/clients/`](https://github.com/joshuadavidthomas/django-language-server/tree/main/docs/clients) directory (e.g., `docs/clients/helix.md`)
2. Include step-by-step setup instructions, any required configuration snippets, and tips for troubleshooting

Your feedback and contributions will help make the setup process smoother for everyone! 🙌
