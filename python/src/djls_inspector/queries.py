from __future__ import annotations

import sys
from dataclasses import dataclass
from enum import Enum
from pathlib import Path
from typing import Literal


class Query(str, Enum):
    PYTHON_ENV = "python_env"
    TEMPLATETAGS = "templatetags"


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
    from django.template.engine import Engine
    from django.template.library import import_library

    # Ensure Django is set up
    if not django.apps.apps.ready:
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


QueryData = PythonEnvironmentQueryData | TemplateTagQueryData
