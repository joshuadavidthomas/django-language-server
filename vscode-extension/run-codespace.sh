#!/bin/bash

# Run the extension in a GitHub Codespace
echo "Running Django Language Server VS Code Extension in a GitHub Codespace..."

# Compile TypeScript
npm run compile

# Create a test project if it doesn't exist
TEST_DIR="$(pwd)/test-django-project"
if [ ! -d "$TEST_DIR" ]; then
    echo "Test project not found. Creating..."
    ./create-test-project.sh
fi

# Package the extension
npm run package

# Install the extension in VS Code
code --install-extension ./vscode-django-language-server-0.1.0.vsix

# Open the test project in VS Code
echo "Opening the test project in VS Code..."
code "$TEST_DIR"

echo "Codespace session started. You can now use the extension in VS Code."