#!/bin/bash

# Run the extension in debug mode
echo "Running Django Language Server VS Code Extension in debug mode..."

# Compile TypeScript
npm run compile

# Check if code is installed
if ! command -v code &> /dev/null; then
    echo "VS Code CLI not found. Please make sure VS Code is installed and the CLI is in your PATH."
    echo "See: https://code.visualstudio.com/docs/editor/command-line"
    exit 1
fi

# Create a test project if it doesn't exist
TEST_DIR="$(pwd)/test-django-project"
if [ ! -d "$TEST_DIR" ]; then
    echo "Test project not found. Creating..."
    ./create-test-project.sh
fi

# Start VS Code with the extension in debug mode
echo "Starting VS Code with the extension in debug mode..."
code --extensionDevelopmentPath="$(pwd)" "$TEST_DIR"

echo "Debug session ended."