from __future__ import annotations

import os
import sys
from dataclasses import dataclass
from enum import Enum
from pathlib import Path
from typing import Literal


class Query(str, Enum):
    DJANGO_INIT = "django_init"
    PYTHON_ENV = "python_env"
    TEMPLATE_DIRS = "template_dirs"
    TEMPLATETAGS = "templatetags"


def initialize_django() -> tuple[bool, str | None]:
    import django
    from django.apps import apps

    try:
        if not os.environ.get("DJANGO_SETTINGS_MODULE"):
            return False, None

        if not apps.ready:
            django.setup()

        return True, None

    except Exception as e:
        return False, str(e)


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
class TemplateDirsQueryData:
    dirs: list[Path]


def get_template_dirs() -> TemplateDirsQueryData:
    from django.apps import apps
    from django.conf import settings

    dirs = []

    for engine in settings.TEMPLATES:
        if "django" not in engine["BACKEND"].lower():
            continue

        dirs.extend(engine.get("DIRS", []))

        if engine.get("APP_DIRS", False):
            for app_config in apps.get_app_configs():
                template_dir = Path(app_config.path) / "templates"
                if template_dir.exists():
                    dirs.append(template_dir)

    return TemplateDirsQueryData(dirs)


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


QueryData = PythonEnvironmentQueryData | TemplateDirsQueryData | TemplateTagQueryData
