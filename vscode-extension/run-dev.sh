#!/bin/bash

# Run the extension in development mode
echo "Running Django Language Server VS Code Extension in development mode..."

# Compile TypeScript
npm run compile

# Check if code-server is installed
if ! command -v code-server &> /dev/null; then
    echo "code-server not found. Please install it to test the extension in development mode."
    echo "You can install it with: npm install -g code-server"
    exit 1
fi

# Create a test project if it doesn't exist
TEST_DIR="$(pwd)/test-django-project"
if [ ! -d "$TEST_DIR" ]; then
    echo "Test project not found. Creating..."
    ./create-test-project.sh
fi

# Start code-server with the extension
echo "Starting code-server with the extension..."
code-server --install-extension ./vscode-django-language-server-0.1.0.vsix
code-server "$TEST_DIR"

echo "Development session ended."