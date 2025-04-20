#!/bin/bash

# Run the extension in VS Code Portable
echo "Running Django Language Server VS Code Extension in VS Code Portable..."

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

# Check if VS Code Portable directory exists
PORTABLE_DIR="$(pwd)/vscode-portable"
if [ ! -d "$PORTABLE_DIR" ]; then
    echo "VS Code Portable not found. Creating..."
    mkdir -p "$PORTABLE_DIR"
    
    # Download VS Code
    echo "Downloading VS Code..."
    if [[ "$OSTYPE" == "linux-gnu"* ]]; then
        wget -O "$PORTABLE_DIR/vscode.tar.gz" "https://code.visualstudio.com/sha/download?build=stable&os=linux-x64"
        tar -xzf "$PORTABLE_DIR/vscode.tar.gz" -C "$PORTABLE_DIR" --strip-components=1
        rm "$PORTABLE_DIR/vscode.tar.gz"
    elif [[ "$OSTYPE" == "darwin"* ]]; then
        wget -O "$PORTABLE_DIR/vscode.zip" "https://code.visualstudio.com/sha/download?build=stable&os=darwin-universal"
        unzip "$PORTABLE_DIR/vscode.zip" -d "$PORTABLE_DIR"
        rm "$PORTABLE_DIR/vscode.zip"
    else
        echo "Unsupported OS: $OSTYPE"
        exit 1
    fi
fi

# Create data directory if it doesn't exist
mkdir -p "$PORTABLE_DIR/data/extensions"

# Copy the extension to the portable extensions directory
cp ./vscode-django-language-server-0.1.0.vsix "$PORTABLE_DIR/data/extensions/"

# Start VS Code Portable with the test project
echo "Starting VS Code Portable with the test project..."
if [[ "$OSTYPE" == "linux-gnu"* ]]; then
    "$PORTABLE_DIR/code" --user-data-dir="$PORTABLE_DIR/data" "$TEST_DIR"
elif [[ "$OSTYPE" == "darwin"* ]]; then
    "$PORTABLE_DIR/Visual Studio Code.app/Contents/MacOS/Electron" --user-data-dir="$PORTABLE_DIR/data" "$TEST_DIR"
fi

echo "VS Code Portable session ended."