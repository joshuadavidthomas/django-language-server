#!/bin/bash

# Run the extension in VS Code Insiders
echo "Running Django Language Server VS Code Extension in VS Code Insiders..."

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

# Check if VS Code Insiders is installed
if ! command -v code-insiders &> /dev/null; then
    echo "VS Code Insiders not found. Please install it first."
    exit 1
fi

# Install the extension in VS Code Insiders
code-insiders --install-extension ./vscode-django-language-server-0.1.0.vsix

# Open the test project in VS Code Insiders
echo "Opening the test project in VS Code Insiders..."
code-insiders "$TEST_DIR"

echo "VS Code Insiders session started. You can now use the extension in VS Code Insiders."