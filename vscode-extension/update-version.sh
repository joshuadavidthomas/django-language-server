#!/bin/bash

# Update the extension version
echo "Updating Django Language Server VS Code Extension version..."

# Check if a version is provided
if [ -z "$1" ]; then
    echo "Error: No version provided."
    echo "Usage: ./update-version.sh <version>"
    echo "Example: ./update-version.sh 0.2.0"
    exit 1
fi

VERSION=$1

# Update package.json
sed -i "s/\"version\": \"[0-9]*\.[0-9]*\.[0-9]*\"/\"version\": \"$VERSION\"/" package.json

# Update CHANGELOG.md
DATE=$(date +%Y-%m-%d)
sed -i "s/## \[[0-9]*\.[0-9]*\.[0-9]*\].*/## [$VERSION] - $DATE/" CHANGELOG.md

echo "Version updated to $VERSION!"
echo "Don't forget to update the CHANGELOG.md with the changes for this version."