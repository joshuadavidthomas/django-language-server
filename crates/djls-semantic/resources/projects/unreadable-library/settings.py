from pathlib import Path

BASE_DIR = Path(__file__).resolve().parent

INSTALLED_APPS = []
TEMPLATES = [
    {
        "BACKEND": "django.template.backends.django.DjangoTemplates",
        "DIRS": [BASE_DIR / "templates"],
        "APP_DIRS": False,
        "OPTIONS": {
            "libraries": {
                "known": "known_tags",
                "open": "open_tags",
            },
        },
    }
]
