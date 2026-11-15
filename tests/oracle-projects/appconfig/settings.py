from __future__ import annotations

SECRET_KEY = "oracle-evaluation"
INSTALLED_APPS = ["nsapp.apps.Custom"]
TEMPLATES = [
    {
        "BACKEND": "django.template.backends.django.DjangoTemplates",
        "APP_DIRS": True,
    }
]
