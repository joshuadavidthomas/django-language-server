from __future__ import annotations

import sys
from dataclasses import dataclass
from enum import Enum
from pathlib import Path
from typing import Literal


class Query(str, Enum):
    PYTHON_ENV = "python_env"
    TEMPLATETAGS = "templatetags"
    DJANGO_INIT = "django_init"


@dataclass
class PythonEnvironmentQueryData:
    sys_base_prefix: Path
    sys_executable: Path
    sys_path: list[Path]
    sys_platform: str
    sys_prefix: Path
    sys_version_info: tuple[
        int, int, int, Literal["alpha", "beta", "candidate", "final"], int
    ]


def get_python_environment_info():
    return PythonEnvironmentQueryData(
        sys_base_prefix=Path(sys.base_prefix),
        sys_executable=Path(sys.executable),
        sys_path=[Path(p) for p in sys.path],
        sys_platform=sys.platform,
        sys_prefix=Path(sys.prefix),
        sys_version_info=(
            sys.version_info.major,
            sys.version_info.minor,
            sys.version_info.micro,
            sys.version_info.releaselevel,
            sys.version_info.serial,
        ),
    )


@dataclass
class TemplateTagQueryData:
    templatetags: list[TemplateTag]


@dataclass
class TemplateTag:
    name: str
    module: str
    doc: str | None


def get_installed_templatetags() -> TemplateTagQueryData:
    import django
    from django.apps import apps
    from django.template.engine import Engine
    from django.template.library import import_library

    # Ensure Django is set up
    if not apps.ready:
        django.setup()

    templatetags: list[TemplateTag] = []

    engine = Engine.get_default()

    for library in engine.template_builtins:
        if library.tags:
            for tag_name, tag_func in library.tags.items():
                templatetags.append(
                    TemplateTag(
                        name=tag_name, module=tag_func.__module__, doc=tag_func.__doc__
                    )
                )

    for lib_module in engine.libraries.values():
        library = import_library(lib_module)
        if library and library.tags:
            for tag_name, tag_func in library.tags.items():
                templatetags.append(
                    TemplateTag(
                        name=tag_name, module=tag_func.__module__, doc=tag_func.__doc__
                    )
                )

    return TemplateTagQueryData(templatetags=templatetags)


def initialize_django() -> tuple[bool, str | None]:
    """Initialize Django and return (success, error_message)."""
    import os
    import django
    from django.apps import apps

    try:
        # Check if Django settings are configured
        if not os.environ.get("DJANGO_SETTINGS_MODULE"):
            # Try to find and set settings module
            import sys
            from pathlib import Path

            # Look for manage.py to determine project structure
            current_path = Path.cwd()
            manage_py = None

            # Search up to 3 levels for manage.py
            for _ in range(3):
                if (current_path / "manage.py").exists():
                    manage_py = current_path / "manage.py"
                    break
                if current_path.parent == current_path:
                    break
                current_path = current_path.parent

            if not manage_py:
                return (
                    False,
                    "Could not find manage.py or DJANGO_SETTINGS_MODULE not set",
                )

            # Add project directory to sys.path
            project_dir = manage_py.parent
            if str(project_dir) not in sys.path:
                sys.path.insert(0, str(project_dir))

            # Try to find settings module - look for common patterns
            # First check if there's a directory with the same name as the parent
            project_name = project_dir.name
            settings_candidates = [
                f"{project_name}.settings",  # e.g., myproject.settings
                "settings",  # Just settings.py in root
                "config.settings",  # Common pattern
                "project.settings",  # Another common pattern
            ]

            # Also check for any directory containing settings.py
            for item in project_dir.iterdir():
                if item.is_dir() and (item / "settings.py").exists():
                    candidate = f"{item.name}.settings"
                    if candidate not in settings_candidates:
                        settings_candidates.insert(
                            0, candidate
                        )  # Prioritize found settings

            for settings_candidate in settings_candidates:
                try:
                    __import__(settings_candidate)
                    os.environ["DJANGO_SETTINGS_MODULE"] = settings_candidate
                    break
                except ImportError:
                    continue

        # Set up Django
        if not apps.ready:
            django.setup()

        return True, None

    except Exception as e:
        return False, str(e)


QueryData = PythonEnvironmentQueryData | TemplateTagQueryData
