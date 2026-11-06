"""A source project using the pinned corpus Bird package, not synthetic inventory.

Apps and engine options follow django-bird v0.19.0 tests/conftest.py TEST_SETTINGS.
All libraries, registrations, block facts, and diagnostics come from real source.
"""

from __future__ import annotations

from pathlib import Path

SECRET_KEY = "bird-oracle-project"
INSTALLED_APPS = ["django_bird", "django.contrib.staticfiles"]
STATIC_URL = "/static/"
TEMPLATES = [
    {
        "BACKEND": "django.template.backends.django.DjangoTemplates",
        "DIRS": [Path(__file__).parent / "templates"],
        "OPTIONS": {
            "builtins": [
                "django.template.defaulttags",
                "django_bird.templatetags.django_bird",
            ],
            "loaders": [
                "django.template.loaders.filesystem.Loader",
                "django.template.loaders.app_directories.Loader",
            ],
        },
    }
]
