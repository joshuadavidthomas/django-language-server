#!/bin/bash

# Initialize the VS Code extension project
echo "Initializing Django Language Server VS Code Extension..."

# Install dependencies
npm install

# Compile TypeScript
npm run compile

# Package the extension
npx vsce package

echo "Extension initialization complete!"
echo "VSIX package created at: $(pwd)/vscode-django-language-server-0.1.0.vsix"
echo ""
echo "To install the extension in VS Code:"
echo "1. Open VS Code"
echo "2. Go to Extensions view (Ctrl+Shift+X)"
echo "3. Click '...' menu in the top-right"
echo "4. Select 'Install from VSIX...'"
echo "5. Navigate to and select the VSIX file"
echo ""
echo "For more information, see INSTALL.md"