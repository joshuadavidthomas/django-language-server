#!/bin/bash

# Run the extension in a web browser
echo "Running Django Language Server VS Code Extension in a web browser..."

# Compile TypeScript
npm run compile

# Check if code-server is installed
if ! command -v code-server &> /dev/null; then
    echo "code-server not found. Installing..."
    npm install -g code-server
fi

# Create a test project if it doesn't exist
TEST_DIR="$(pwd)/test-django-project"
if [ ! -d "$TEST_DIR" ]; then
    echo "Test project not found. Creating..."
    ./create-test-project.sh
fi

# Package the extension
npm run package

# Install the extension in code-server
code-server --install-extension ./vscode-django-language-server-0.1.0.vsix

# Start code-server with the test project
echo "Starting code-server with the test project..."
code-server --bind-addr 0.0.0.0:12000 --auth none "$TEST_DIR"

echo "Web session ended."