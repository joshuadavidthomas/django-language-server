#!/bin/bash

# Create a test Django project for testing the extension
echo "Creating a test Django project..."

# Check if Django is installed
if ! python -c "import django" &> /dev/null; then
    echo "Django not found. Installing..."
    pip install django
fi

# Create a test directory
TEST_DIR="$(pwd)/test-django-project"
mkdir -p "$TEST_DIR"
cd "$TEST_DIR"

# Create a Django project
django-admin startproject testproject .

# Create a Django app
python manage.py startapp testapp

# Create templates directory
mkdir -p templates/testapp

# Create a sample template
cat > templates/testapp/index.html << EOF
<!DOCTYPE html>
<html>
<head>
    <title>{% block title %}Django Test{% endblock %}</title>
</head>
<body>
    <header>
        <h1>Django Test Project</h1>
    </header>
    
    <main>
        {% block content %}
        <p>Welcome to the test project!</p>
        
        {% if user.is_authenticated %}
            <p>Hello, {{ user.username }}!</p>
        {% else %}
            <p>Please log in.</p>
        {% endif %}
        
        <ul>
            {% for item in items %}
                <li>{{ item.name }} - {{ item.description }}</li>
            {% empty %}
                <li>No items available.</li>
            {% endfor %}
        </ul>
        {% endblock %}
    </main>
    
    <footer>
        {% now "Y" %} Django Test Project
    </footer>
</body>
</html>
EOF

# Create a base template
cat > templates/base.html << EOF
<!DOCTYPE html>
<html>
<head>
    <title>{% block title %}Django Test{% endblock %}</title>
    {% block extra_head %}{% endblock %}
</head>
<body>
    <header>
        <h1>{% block header %}Django Test Project{% endblock %}</h1>
        <nav>
            <ul>
                <li><a href="{% url 'home' %}">Home</a></li>
                <li><a href="{% url 'about' %}">About</a></li>
                <li><a href="{% url 'contact' %}">Contact</a></li>
            </ul>
        </nav>
    </header>
    
    <main>
        {% block content %}{% endblock %}
    </main>
    
    <footer>
        {% now "Y" %} Django Test Project
        {% block footer_extra %}{% endblock %}
    </footer>
</body>
</html>
EOF

# Create a child template
cat > templates/testapp/child.html << EOF
{% extends "base.html" %}

{% block title %}Child Page{% endblock %}

{% block header %}Child Page Header{% endblock %}

{% block content %}
    <h2>Child Page Content</h2>
    <p>This is content from the child template.</p>
    
    {% include "testapp/snippet.html" with variable="Included content" %}
    
    {% with name="Test User" %}
        <p>Hello, {{ name }}!</p>
    {% endwith %}
{% endblock %}

{% block footer_extra %}
    <p>Additional footer content</p>
{% endblock %}
EOF

# Create a snippet template
cat > templates/testapp/snippet.html << EOF
<div class="snippet">
    <h3>Reusable Snippet</h3>
    <p>{{ variable|default:"Default content" }}</p>
</div>
EOF

# Update settings.py to include templates directory
sed -i "s/'DIRS': \[\],/'DIRS': \[os.path.join(BASE_DIR, 'templates')\],/" testproject/settings.py
sed -i "1s/^/import os\n/" testproject/settings.py

# Add testapp to INSTALLED_APPS
sed -i "s/'django.contrib.staticfiles',/'django.contrib.staticfiles',\n    'testapp',/" testproject/settings.py

# Create a simple view
cat > testapp/views.py << EOF
from django.shortcuts import render

def index(request):
    items = [
        {'name': 'Item 1', 'description': 'Description 1'},
        {'name': 'Item 2', 'description': 'Description 2'},
        {'name': 'Item 3', 'description': 'Description 3'},
    ]
    return render(request, 'testapp/index.html', {'items': items})

def about(request):
    return render(request, 'testapp/child.html')

def contact(request):
    return render(request, 'base.html')
EOF

# Create URLs
cat > testapp/urls.py << EOF
from django.urls import path
from . import views

urlpatterns = [
    path('', views.index, name='home'),
    path('about/', views.about, name='about'),
    path('contact/', views.contact, name='contact'),
]
EOF

# Update project URLs
cat > testproject/urls.py << EOF
from django.contrib import admin
from django.urls import path, include

urlpatterns = [
    path('admin/', admin.site.urls),
    path('', include('testapp.urls')),
]
EOF

echo "Test Django project created at $TEST_DIR"
echo ""
echo "To test the extension:"
echo "1. Install the VSIX package in VS Code"
echo "2. Open the test Django project folder in VS Code"
echo "3. Open any of the template files in the templates directory"
echo "4. Try typing '{% ' to see template tag autocompletion"