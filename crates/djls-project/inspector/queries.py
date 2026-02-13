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
    TEMPLATE_LIBRARIES = "template_libraries"


def initialize_django() -> tuple[bool, str | None]:
    import django
    from django.apps import apps

    try:
        if not os.environ.get("DJANGO_SETTINGS_MODULE"):
            return False, None

        if not apps.ready:
            django.setup()

        return True, None

    except KeyError as e:
        var_name = str(e).strip("'\"")
        return False, (
            f"Missing required environment variable: {var_name}. "
            f"Django settings failed to load because '{var_name}' is not set "
            f"in the editor's environment. To fix this, either add "
            f"'{var_name}' to a .env file in your project root, or configure "
            f"'env_file' in your djls settings to point to an env file that "
            f"defines it."
        )

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
class TemplateLibrarySymbol:
    kind: Literal["tag", "filter"]
    name: str
    load_name: str | None
    library_module: str
    module: str
    doc: str | None


@dataclass
class TemplateLibrariesQueryData:
    symbols: list[TemplateLibrarySymbol]
    libraries: dict[str, str]
    builtins: list[str]


def get_installed_template_libraries() -> TemplateLibrariesQueryData:
    import django
    from django.apps import apps
    from django.template.engine import Engine
    from django.template.library import import_library

    if not apps.ready:
        django.setup()

    symbols: list[TemplateLibrarySymbol] = []

    engine = Engine.get_default()

    builtin_modules: list[str] = []

    for builtin_module, library in zip(engine.builtins, engine.template_builtins):
        if builtin_module in builtin_modules:
            continue
        builtin_modules.append(builtin_module)

        if library.tags:
            for tag_name, tag_func in library.tags.items():
                symbols.append(
                    TemplateLibrarySymbol(
                        kind="tag",
                        name=tag_name,
                        load_name=None,
                        library_module=builtin_module,
                        module=tag_func.__module__,
                        doc=tag_func.__doc__,
                    )
                )

        if library.filters:
            for filter_name, filter_func in library.filters.items():
                symbols.append(
                    TemplateLibrarySymbol(
                        kind="filter",
                        name=filter_name,
                        load_name=None,
                        library_module=builtin_module,
                        module=filter_func.__module__,
                        doc=filter_func.__doc__,
                    )
                )

    lib_registry: dict[str, str] = {}

    for load_name, lib_module in engine.libraries.items():
        lib_registry[load_name] = lib_module

        try:
            library = import_library(lib_module)
        except Exception:
            continue

        if library and library.tags:
            for tag_name, tag_func in library.tags.items():
                symbols.append(
                    TemplateLibrarySymbol(
                        kind="tag",
                        name=tag_name,
                        load_name=load_name,
                        library_module=lib_module,
                        module=tag_func.__module__,
                        doc=tag_func.__doc__,
                    )
                )

        if library and library.filters:
            for filter_name, filter_func in library.filters.items():
                symbols.append(
                    TemplateLibrarySymbol(
                        kind="filter",
                        name=filter_name,
                        load_name=load_name,
                        library_module=lib_module,
                        module=filter_func.__module__,
                        doc=filter_func.__doc__,
                    )
                )

    return TemplateLibrariesQueryData(
        symbols=symbols,
        libraries=lib_registry,
        builtins=builtin_modules,
    )


QueryData = PythonEnvironmentQueryData | TemplateDirsQueryData | TemplateLibrariesQueryData
