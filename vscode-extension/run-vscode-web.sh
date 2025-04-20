#!/bin/bash

# Run the extension in VS Code Web
echo "Running Django Language Server VS Code Extension in VS Code Web..."

# Compile TypeScript
npm run compile

# Check if vscode-test-web is installed
if ! command -v vscode-test-web &> /dev/null; then
    echo "vscode-test-web not found. Installing..."
    npm install -g @vscode/test-web
fi

# Create a test project if it doesn't exist
TEST_DIR="$(pwd)/test-django-project"
if [ ! -d "$TEST_DIR" ]; then
    echo "Test project not found. Creating..."
    ./create-test-project.sh
fi

# Package the extension
npm run package

# Start VS Code Web with the extension
echo "Starting VS Code Web with the extension..."
vscode-test-web --extensionDevelopmentPath="$(pwd)" --browserType=chromium --port=12000 --host=0.0.0.0 "$TEST_DIR"

echo "VS Code Web session ended."