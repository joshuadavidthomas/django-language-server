from pathlib import Path

BASE_DIR = Path(__file__).resolve().parent

INSTALLED_APPS = ["blog"]
TEMPLATES = [
    {
        "BACKEND": "django.template.backends.django.DjangoTemplates",
        "DIRS": [],
        "APP_DIRS": True,
    }
]
