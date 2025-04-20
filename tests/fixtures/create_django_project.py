#!/usr/bin/env python3
"""
Create a Django project for testing.
"""

from __future__ import annotations

import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path


def create_django_project(
    project_name: str = "testproject",
    app_name: str = "testapp",
    django_version: str | None = None,
) -> Path:
    """
    Create a Django project for testing.

    Args:
        project_name: Name of the Django project
        app_name: Name of the Django app
        django_version: Django version to use (e.g., "4.2", "5.0")

    Returns:
        Path to the created Django project
    """
    # Create a temporary directory
    temp_dir = tempfile.mkdtemp()
    project_dir = Path(temp_dir) / project_name

    # Install Django if a specific version is requested
    if django_version:
        subprocess.check_call(
            [
                sys.executable,
                "-m",
                "pip",
                "install",
                f"Django>={django_version},<{float(django_version) + 0.1}",
            ]
        )

    # Create Django project
    subprocess.check_call(
        [
            sys.executable,
            "-m",
            "django",
            "startproject",
            project_name,
            temp_dir,
        ]
    )

    # Create Django app
    os.chdir(temp_dir)
    subprocess.check_call(
        [
            sys.executable,
            "-m",
            "django",
            "startapp",
            app_name,
        ]
    )

    # Add app to INSTALLED_APPS
    settings_path = Path(temp_dir) / "settings.py"
    if not settings_path.exists():
        settings_path = Path(temp_dir) / project_name / "settings.py"

    with open(settings_path, "r") as f:
        settings_content = f.read()

    settings_content = settings_content.replace(
        "INSTALLED_APPS = [",
        f"INSTALLED_APPS = [\n    '{app_name}',",
    )

    with open(settings_path, "w") as f:
        f.write(settings_content)

    # Create templates directory
    templates_dir = Path(temp_dir) / app_name / "templates" / app_name
    templates_dir.mkdir(parents=True, exist_ok=True)

    # Create a sample template
    sample_template = templates_dir / "index.html"
    with open(sample_template, "w") as f:
        f.write(
            """{% extends "base.html" %}

{% block content %}
  <h1>{{ title }}</h1>
  
  <ul>
    {% for item in items %}
      <li>{{ item.name }} - {{ item.description }}</li>
    {% endfor %}
  </ul>
  
  {% if show_footer %}
    <footer>
      <p>© {% now "Y" %} Test Project</p>
    </footer>
  {% endif %}
{% endblock %}
"""
        )

    # Create a base template
    base_template_dir = Path(temp_dir) / "templates"
    base_template_dir.mkdir(parents=True, exist_ok=True)
    
    base_template = base_template_dir / "base.html"
    with open(base_template, "w") as f:
        f.write(
            """<!DOCTYPE html>
<html>
<head>
  <title>{% block title %}Test Project{% endblock %}</title>
  <style>
    body {
      font-family: Arial, sans-serif;
      margin: 0;
      padding: 20px;
    }
  </style>
</head>
<body>
  <nav>
    <a href="{% url 'home' %}">Home</a>
  </nav>
  
  <main>
    {% block content %}{% endblock %}
  </main>
</body>
</html>
"""
        )

    # Create a views.py file
    views_path = Path(temp_dir) / app_name / "views.py"
    with open(views_path, "w") as f:
        f.write(
            """from django.shortcuts import render

def home(request):
    context = {
        'title': 'Welcome to the Test Project',
        'items': [
            {'name': 'Item 1', 'description': 'Description 1'},
            {'name': 'Item 2', 'description': 'Description 2'},
            {'name': 'Item 3', 'description': 'Description 3'},
        ],
        'show_footer': True,
    }
    return render(request, f'{request.resolver_match.app_name}/index.html', context)
"""
        )

    # Create a urls.py file in the app
    app_urls_path = Path(temp_dir) / app_name / "urls.py"
    with open(app_urls_path, "w") as f:
        f.write(
            """from django.urls import path
from . import views

app_name = 'testapp'

urlpatterns = [
    path('', views.home, name='home'),
]
"""
        )

    # Update project urls.py
    project_urls_path = Path(temp_dir) / project_name / "urls.py"
    with open(project_urls_path, "r") as f:
        urls_content = f.read()

    urls_content = urls_content.replace(
        "from django.urls import path",
        "from django.urls import path, include",
    )
    urls_content = urls_content.replace(
        "urlpatterns = [",
        f"urlpatterns = [\n    path('', include('{app_name}.urls')),",
    )

    with open(project_urls_path, "w") as f:
        f.write(urls_content)

    # Update settings to include templates directory
    with open(settings_path, "r") as f:
        settings_content = f.read()

    if "'DIRS': []," in settings_content:
        settings_content = settings_content.replace(
            "'DIRS': [],",
            "'DIRS': [BASE_DIR / 'templates'],",
        )

    with open(settings_path, "w") as f:
        f.write(settings_content)

    return Path(temp_dir)


def cleanup_django_project(project_dir: Path) -> None:
    """
    Clean up a Django project created for testing.

    Args:
        project_dir: Path to the Django project
    """
    if project_dir.exists():
        shutil.rmtree(project_dir)


if __name__ == "__main__":
    # Example usage
    project_dir = create_django_project(django_version="5.0")
    print(f"Created Django project at: {project_dir}")
    # Don't clean up when run directly, for manual inspection