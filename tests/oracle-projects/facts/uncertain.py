from __future__ import annotations

SECRET_KEY = "oracle-uncertain-builtins"
INSTALLED_APPS = []


def options():
    return {"builtins": ["checks"]}


TEMPLATES = [
    {
        "BACKEND": "django.template.backends.django.DjangoTemplates",
        "OPTIONS": options(),
    }
]
