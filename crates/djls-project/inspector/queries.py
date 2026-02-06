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
    TEMPLATE_INVENTORY = "template_inventory"


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
class TemplateTag:
    name: str
    provenance: dict  # {"library": {"load_name": str, "module": str}} | {"builtin": {"module": str}}
    defining_module: str
    doc: str | None


@dataclass
class TemplateFilter:
    name: str
    provenance: dict  # {"library": {"load_name": str, "module": str}} | {"builtin": {"module": str}}
    defining_module: str
    doc: str | None


@dataclass
class TemplateTagQueryData:
    libraries: dict[str, str]  # load_name -> module_path mapping
    builtins: list[str]  # ordered builtin module paths
    templatetags: list[TemplateTag]


def get_installed_templatetags() -> TemplateTagQueryData:
    import django
    from django.apps import apps
    from django.template.engine import Engine
    from django.template.library import import_library

    if not apps.ready:
        django.setup()

    engine = Engine.get_default()
    templatetags: list[TemplateTag] = []

    libraries = dict(engine.libraries)
    builtins = list(engine.builtins)

    if len(engine.builtins) != len(engine.template_builtins):
        raise RuntimeError(
            f"engine.builtins ({len(engine.builtins)}) and "
            f"engine.template_builtins ({len(engine.template_builtins)}) length mismatch"
        )

    for builtin_module, library in zip(engine.builtins, engine.template_builtins):
        if library.tags:
            for tag_name, tag_func in library.tags.items():
                templatetags.append(
                    TemplateTag(
                        name=tag_name,
                        provenance={"builtin": {"module": builtin_module}},
                        defining_module=tag_func.__module__,
                        doc=tag_func.__doc__,
                    )
                )

    for load_name, lib_module in engine.libraries.items():
        library = import_library(lib_module)
        if library and library.tags:
            for tag_name, tag_func in library.tags.items():
                templatetags.append(
                    TemplateTag(
                        name=tag_name,
                        provenance={"library": {"load_name": load_name, "module": lib_module}},
                        defining_module=tag_func.__module__,
                        doc=tag_func.__doc__,
                    )
                )

    return TemplateTagQueryData(
        libraries=libraries,
        builtins=builtins,
        templatetags=templatetags,
    )


@dataclass
class TemplateInventoryQueryData:
    libraries: dict[str, str]
    builtins: list[str]
    templatetags: list[TemplateTag]
    templatefilters: list[TemplateFilter]


def get_template_inventory() -> TemplateInventoryQueryData:
    import django
    from django.apps import apps
    from django.template.engine import Engine
    from django.template.library import import_library

    if not apps.ready:
        django.setup()

    engine = Engine.get_default()
    templatetags: list[TemplateTag] = []
    templatefilters: list[TemplateFilter] = []

    libraries = dict(engine.libraries)
    builtins = list(engine.builtins)

    if len(engine.builtins) != len(engine.template_builtins):
        raise RuntimeError(
            f"engine.builtins ({len(engine.builtins)}) and "
            f"engine.template_builtins ({len(engine.template_builtins)}) length mismatch"
        )

    for builtin_module, library in zip(engine.builtins, engine.template_builtins):
        if library.tags:
            for tag_name, tag_func in library.tags.items():
                templatetags.append(
                    TemplateTag(
                        name=tag_name,
                        provenance={"builtin": {"module": builtin_module}},
                        defining_module=tag_func.__module__,
                        doc=tag_func.__doc__,
                    )
                )
        if library.filters:
            for filter_name, filter_func in library.filters.items():
                templatefilters.append(
                    TemplateFilter(
                        name=filter_name,
                        provenance={"builtin": {"module": builtin_module}},
                        defining_module=filter_func.__module__,
                        doc=filter_func.__doc__,
                    )
                )

    for load_name, lib_module in engine.libraries.items():
        library = import_library(lib_module)
        if library:
            if library.tags:
                for tag_name, tag_func in library.tags.items():
                    templatetags.append(
                        TemplateTag(
                            name=tag_name,
                            provenance={"library": {"load_name": load_name, "module": lib_module}},
                            defining_module=tag_func.__module__,
                            doc=tag_func.__doc__,
                        )
                    )
            if library.filters:
                for filter_name, filter_func in library.filters.items():
                    templatefilters.append(
                        TemplateFilter(
                            name=filter_name,
                            provenance={"library": {"load_name": load_name, "module": lib_module}},
                            defining_module=filter_func.__module__,
                            doc=filter_func.__doc__,
                        )
                    )

    return TemplateInventoryQueryData(
        libraries=libraries,
        builtins=builtins,
        templatetags=templatetags,
        templatefilters=templatefilters,
    )


QueryData = (
    PythonEnvironmentQueryData
    | TemplateDirsQueryData
    | TemplateTagQueryData
    | TemplateInventoryQueryData
)
