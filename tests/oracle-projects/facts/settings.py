from __future__ import annotations

SECRET_KEY = "oracle-evaluation"
INSTALLED_APPS = []
TEMPLATES = [
    {
        "BACKEND": "django.template.backends.django.DjangoTemplates",
        "OPTIONS": {"libraries": {"checks": "checks"}},
    }
]
