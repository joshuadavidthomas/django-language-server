#!/bin/bash

# Test the VS Code extension with the Django Language Server
echo "Testing Django Language Server VS Code Extension..."

# Check if Django Language Server is installed
if ! command -v djls &> /dev/null; then
    echo "Django Language Server (djls) not found. Installing..."
    pip install django-language-server
fi

# Create a simple Django project for testing
TEST_DIR="$(pwd)/test-django-project"
if [ ! -d "$TEST_DIR" ]; then
    echo "Creating test Django project..."
    mkdir -p "$TEST_DIR"
    cd "$TEST_DIR"
    
    # Check if Django is installed
    if ! python -c "import django" &> /dev/null; then
        echo "Django not found. Installing..."
        pip install django
    fi
    
    # Create a Django project
    django-admin startproject testproject .
    
    # Create a simple template
    mkdir -p templates
    cat > templates/test.html << EOF
<!DOCTYPE html>
<html>
<head>
    <title>Test Template</title>
</head>
<body>
    <h1>Test Template</h1>
    {% if test_var %}
        <p>{{ test_var }}</p>
    {% endif %}
    
    {% block content %}
    {% endblock %}
</body>
</html>
EOF
    
    # Update settings to include templates
    sed -i "s/'DIRS': \[\],/'DIRS': \[os.path.join(BASE_DIR, 'templates')\],/" testproject/settings.py
    sed -i "1s/^/import os\n/" testproject/settings.py
    
    echo "Test Django project created at $TEST_DIR"
fi

echo ""
echo "To test the extension:"
echo "1. Install the VSIX package in VS Code"
echo "2. Open the test Django project folder in VS Code"
echo "3. Open templates/test.html"
echo "4. Try typing '{% ' to see template tag autocompletion"
echo ""
echo "For more information, see INSTALL.md"