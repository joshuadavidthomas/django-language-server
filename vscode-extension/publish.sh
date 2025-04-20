#!/bin/bash

# Publish the extension to the VS Code Marketplace
echo "Publishing Django Language Server VS Code Extension..."

# Check if vsce is installed
if ! command -v vsce &> /dev/null; then
    echo "vsce not found. Installing..."
    npm install -g @vscode/vsce
fi

# Check if a Personal Access Token is provided
if [ -z "$VSCE_PAT" ]; then
    echo "Error: No Personal Access Token provided."
    echo "Please set the VSCE_PAT environment variable with your Azure DevOps Personal Access Token."
    echo "You can create a token at: https://dev.azure.com/<your-organization>/_usersSettings/tokens"
    exit 1
fi

# Compile TypeScript
npm run compile

# Package the extension
vsce package

# Publish the extension
vsce publish

echo "Extension published to the VS Code Marketplace!"
echo "Note: You can also manually publish the extension by uploading the VSIX file to the VS Code Marketplace."