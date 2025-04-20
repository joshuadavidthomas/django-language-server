# Django Language Server for VS Code

This extension integrates the [Django Language Server](https://github.com/joshuadavidthomas/django-language-server) into Visual Studio Code, providing enhanced language features for Django templates and Python files.

![Django Language Server in action](https://raw.githubusercontent.com/joshuadavidthomas/django-language-server/main/docs/images/demo.gif)

## Features

- Template tag autocompletion
- Syntax highlighting for Django templates
- Error checking and diagnostics
- More features coming soon as the Django Language Server evolves!

## Requirements

Before using this extension, you need to install the Django Language Server:

```bash
# Using uv (recommended)
uv tool install django-language-server

# Using pipx
pipx install django-language-server

# Or in your project environment
pip install django-language-server
```

## Extension Settings

This extension contributes the following settings:

* `djangoLanguageServer.command`: Path to the Django Language Server executable (default: "djls")
* `djangoLanguageServer.args`: Arguments to pass to the Django Language Server (default: ["serve"])
* `djangoLanguageServer.trace.server`: Traces the communication between VS Code and the Django language server (options: "off", "messages", "verbose")

## Usage

Once installed, the extension will automatically activate when you open Django HTML templates (`.html`, `.djhtml`) or Python files in a Django project.

The language server provides features like:
- Template tag autocompletion (when you type `{%`)
- More features will be added as the Django Language Server project evolves

## Troubleshooting

If you encounter issues:

1. Make sure the Django Language Server (`djls`) is installed and available in your PATH
2. Check the VS Code output panel (View > Output) and select "Django Language Server" from the dropdown
3. Try setting `djangoLanguageServer.trace.server` to "verbose" for more detailed logs
4. Ensure your Django project is properly configured and can be detected by the language server

## Development

1. Clone this repository
2. Run `npm install` to install dependencies
3. Run `npm run compile` to compile TypeScript
4. Run `npm run package` to create a VSIX package
5. Install the extension in VS Code using the "Install from VSIX" command

### Development Scripts

This extension comes with several scripts to help with development:

- `npm run compile`: Compile TypeScript
- `npm run watch`: Watch for changes and compile TypeScript
- `npm run lint`: Lint the code
- `npm run package`: Package the extension as a VSIX file
- `npm run publish`: Publish the extension to VS Code Marketplace
- `npm run clean`: Clean the project
- `npm run test-project`: Create a test Django project
- `npm run install-local`: Install the extension locally
- `npm run uninstall`: Uninstall the extension
- `npm run update-version`: Update the extension version
- `npm run debug`: Run the extension in debug mode
- `npm run setup-tests`: Set up and run tests
- `npm run docs`: Generate documentation
- `npm run web`: Run the extension in a web browser
- `npm run vscode-web`: Run the extension in VS Code Web
- `npm run remote`: Run the extension in a remote VS Code Server
- `npm run docker`: Run the extension in a Docker container
- `npm run codespace`: Run the extension in a GitHub Codespace
- `npm run insiders`: Run the extension in VS Code Insiders
- `npm run portable`: Run the extension in VS Code Portable
- `npm run dev-host`: Run the extension in VS Code Extension Development Host
- `npm run dev-host-debug`: Run the extension in VS Code Extension Development Host with debugging

### Development Environments

You can develop this extension in different environments:

- **Local**: Use `npm run debug` to run the extension in debug mode, `npm run dev-host` for Extension Development Host, `npm run dev-host-debug` for Extension Development Host with debugging, `npm run insiders` for VS Code Insiders, or `npm run portable` for VS Code Portable
- **Web**: Use `npm run web` to run the extension in a web browser, `npm run vscode-web` for VS Code Web, or `npm run remote` for a remote VS Code Server
- **Docker**: Use `npm run docker` to run the extension in a Docker container
- **GitHub Codespace**: Use the provided `.devcontainer/devcontainer.json` to develop in a GitHub Codespace or run `npm run codespace`

## Contributing

Contributions are welcome! See [CONTRIBUTING.md](CONTRIBUTING.md) for details.

## License

This extension is licensed under the [Apache License 2.0](LICENSE).

## Release Notes

### 0.1.0

Initial release of the Django Language Server extension:
- Integration with Django Language Server
- Support for Django HTML templates and Python files
- Template tag autocompletion