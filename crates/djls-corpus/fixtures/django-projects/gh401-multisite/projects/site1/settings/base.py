from pathlib import Path

PROJECT_DIR = Path(__file__).resolve().parents[1]

INSTALLED_APPS = [
    "django.contrib.auth",
    "django.contrib.contenttypes",
    "clientname.app1",
    "clientname.app2",
]

TEMPLATES = [
    {
        "BACKEND": "django.template.backends.django.DjangoTemplates",
        "DIRS": [PROJECT_DIR / "templates"],
        "APP_DIRS": True,
    }
]
