from __future__ import annotations

from pathlib import Path

SECRET_KEY = "oracle-evaluation"
INSTALLED_APPS = []
TEMPLATES = [
    {
        "BACKEND": "django.template.backends.django.DjangoTemplates",
        "DIRS": [Path(__file__).parent / "templates"],
        "OPTIONS": {"loaders": ["custom.Loader"]},
    }
]
