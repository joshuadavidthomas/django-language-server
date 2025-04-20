#!/bin/bash

# Generate documentation
echo "Generating documentation for Django Language Server VS Code Extension..."

# Check if typedoc is installed
if ! command -v typedoc &> /dev/null; then
    echo "typedoc not found. Installing..."
    npm install --save-dev typedoc
fi

# Create docs directory
mkdir -p docs

# Generate documentation
echo "Generating API documentation..."
npx typedoc --out docs/api src/extension.ts

# Create documentation index
echo "Creating documentation index..."
cat > docs/index.md << EOF
# Django Language Server VS Code Extension

This is the documentation for the Django Language Server VS Code Extension.

## Contents

- [API Documentation](api/index.html)
- [Installation Guide](../INSTALL.md)
- [Contributing Guide](../CONTRIBUTING.md)
- [Changelog](../CHANGELOG.md)
- [License](../LICENSE)

## Features

- Template tag autocompletion
- Syntax highlighting for Django templates
- Error checking and diagnostics
- More features coming soon as the Django Language Server evolves!

## Requirements

Before using this extension, you need to install the Django Language Server:

\`\`\`bash
# Using uv (recommended)
uv tool install django-language-server

# Using pipx
pipx install django-language-server

# Or in your project environment
pip install django-language-server
\`\`\`

## Extension Settings

This extension contributes the following settings:

* \`djangoLanguageServer.command\`: Path to the Django Language Server executable (default: "djls")
* \`djangoLanguageServer.args\`: Arguments to pass to the Django Language Server (default: ["serve"])
* \`djangoLanguageServer.trace.server\`: Traces the communication between VS Code and the Django language server (options: "off", "messages", "verbose")
EOF

echo "Documentation generated in the docs directory!"