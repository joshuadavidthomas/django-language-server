#!/bin/bash

# Clean the project
echo "Cleaning Django Language Server VS Code Extension project..."

# Remove node_modules
if [ -d "node_modules" ]; then
    echo "Removing node_modules..."
    rm -rf node_modules
fi

# Remove out directory
if [ -d "out" ]; then
    echo "Removing out directory..."
    rm -rf out
fi

# Remove VSIX files
if ls *.vsix 1> /dev/null 2>&1; then
    echo "Removing VSIX files..."
    rm *.vsix
fi

# Remove test project
if [ -d "test-django-project" ]; then
    echo "Removing test Django project..."
    rm -rf test-django-project
fi

echo "Project cleaned!"