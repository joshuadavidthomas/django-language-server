#!/bin/bash

# Install the extension locally
echo "Installing Django Language Server VS Code Extension locally..."

# Check if code is installed
if ! command -v code &> /dev/null; then
    echo "VS Code CLI not found. Please make sure VS Code is installed and the CLI is in your PATH."
    echo "See: https://code.visualstudio.com/docs/editor/command-line"
    exit 1
fi

# Compile TypeScript
npm run compile

# Package the extension
vsce package

# Install the extension
code --install-extension vscode-django-language-server-0.1.0.vsix

echo "Extension installed locally!"
echo "Please restart VS Code to activate the extension."