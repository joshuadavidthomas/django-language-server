# Installing the Django Language Server VS Code Extension

## Prerequisites

Before installing the extension, make sure you have the Django Language Server installed:

```bash
# Using uv (recommended)
uv tool install django-language-server

# Using pipx
pipx install django-language-server

# Or in your project environment
pip install django-language-server
```

## Installation Methods

### Method 1: Install from VSIX file

1. Open VS Code
2. Go to the Extensions view (Ctrl+Shift+X)
3. Click on the "..." menu in the top-right of the Extensions view
4. Select "Install from VSIX..."
5. Navigate to and select the `vscode-django-language-server-0.1.0.vsix` file
6. Restart VS Code if prompted

### Method 2: Install from VS Code Marketplace (Future)

Once the extension is published to the VS Code Marketplace:

1. Open VS Code
2. Go to the Extensions view (Ctrl+Shift+X)
3. Search for "Django Language Server"
4. Click "Install"

## Configuration

The extension provides the following settings that you can customize in your VS Code settings:

```json
{
  "djangoLanguageServer.command": "djls",
  "djangoLanguageServer.args": ["serve"],
  "djangoLanguageServer.trace.server": "off"
}
```

- `djangoLanguageServer.command`: Path to the Django Language Server executable
- `djangoLanguageServer.args`: Arguments to pass to the Django Language Server
- `djangoLanguageServer.trace.server`: Traces the communication between VS Code and the Django language server (options: "off", "messages", "verbose")

## Usage

Once installed, the extension will automatically activate when you open Django HTML templates (`.html`, `.djhtml`) or Python files in a Django project.

The language server will provide features like:
- Template tag autocompletion (when you type `{%`)

More features will be added as the Django Language Server project evolves.

## Troubleshooting

If you encounter issues:

1. Make sure the Django Language Server (`djls`) is installed and available in your PATH
2. Check the VS Code output panel (View > Output) and select "Django Language Server" from the dropdown
3. Try setting `djangoLanguageServer.trace.server` to "verbose" for more detailed logs
4. Ensure your Django project is properly configured and can be detected by the language server