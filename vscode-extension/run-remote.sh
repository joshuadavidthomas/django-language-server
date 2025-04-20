#!/bin/bash

# Run the extension in a remote VS Code Server
echo "Running Django Language Server VS Code Extension in a remote VS Code Server..."

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

# Check if code-server is installed
if ! command -v code-server &> /dev/null; then
    echo "code-server not found. Installing..."
    curl -fsSL https://code-server.dev/install.sh | sh
fi

# Install the extension in code-server
code-server --install-extension ./vscode-django-language-server-0.1.0.vsix

# Start code-server with the test project
echo "Starting code-server with the test project..."
code-server --auth none --port 12000 --host 0.0.0.0 "$TEST_DIR"

echo "Remote VS Code Server session ended."