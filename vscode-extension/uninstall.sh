#!/bin/bash

# Uninstall the extension
echo "Uninstalling Django Language Server VS Code Extension..."

# Check if code is installed
if ! command -v code &> /dev/null; then
    echo "VS Code CLI not found. Please make sure VS Code is installed and the CLI is in your PATH."
    echo "See: https://code.visualstudio.com/docs/editor/command-line"
    exit 1
fi

# Uninstall the extension
code --uninstall-extension django-language-server.vscode-django-language-server

echo "Extension uninstalled!"
echo "Please restart VS Code to complete the uninstallation."