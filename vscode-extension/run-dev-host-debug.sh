#!/bin/bash

# Run the extension in VS Code Extension Development Host with debugging
echo "Running Django Language Server VS Code Extension in Extension Development Host with debugging..."

# Compile TypeScript
npm run compile

# Create a test project if it doesn't exist
TEST_DIR="$(pwd)/test-django-project"
if [ ! -d "$TEST_DIR" ]; then
    echo "Test project not found. Creating..."
    ./create-test-project.sh
fi

# Check if VS Code is installed
if ! command -v code &> /dev/null; then
    echo "VS Code not found. Please install it first."
    exit 1
fi

# Start VS Code Extension Development Host with the test project and debugging
echo "Starting VS Code Extension Development Host with the test project and debugging..."
code --extensionDevelopmentPath="$(pwd)" --inspect-extensions=9229 "$TEST_DIR"

echo "VS Code Extension Development Host session started with debugging. You can now use the extension in VS Code and debug it."