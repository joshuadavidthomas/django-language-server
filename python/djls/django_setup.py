from __future__ import annotations

import json

from django.conf import settings
from django.template.engine import Engine


def get_django_setup_info():
    """
    Get Django setup information including installed apps and template tags.
    Returns dict with 'apps' and 'tags' keys.
    """
    return {
        "apps": list(settings.INSTALLED_APPS),
        "tags": [
            {
                "name": tag_name,
                "library": module_name.split(".")[-1],
                "doc": tag_func.__doc__ if hasattr(tag_func, "__doc__") else None,
            }
            for module_name, library in (
                [("", lib) for lib in Engine.get_default().template_builtins]
                + sorted(Engine.get_default().template_libraries.items())
            )
            for tag_name, tag_func in library.tags.items()
        ],
    }


if __name__ == "__main__":
    import django

    django.setup()
    print(json.dumps(get_django_setup_info()))
