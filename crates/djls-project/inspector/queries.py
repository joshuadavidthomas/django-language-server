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
    TEMPLATETAGS = "templatetags"  # Legacy (M1-M3 compat)
    TEMPLATE_INVENTORY = "template_inventory"  # Unified query (M4+)



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
    # Top-level registry structures (preserved from Django engine)
    libraries: dict[str, str]  # load_name → module_path mapping
    builtins: list[str]  # ordered builtin module paths
    # Tag inventory
    templatetags: list[TemplateTag]


@dataclass
class TemplateTag:
    name: str
    provenance: dict  # {"library": {"load_name": str, "module": str}} | {"builtin": {"module": str}}
    defining_module: str
    doc: str | None


@dataclass
class TemplateFilter:
    """A template filter with provenance information."""

    name: str
    provenance: dict  # {"library": {"load_name": str, "module": str}} | {"builtin": {"module": str}}
    defining_module: str
    doc: str | None


@dataclass
class TemplateInventoryQueryData:
    """Unified template inventory: tags + filters + registry in one snapshot."""

    libraries: dict[str, str]  # load_name → module_path
    builtins: list[str]  # ordered builtin module paths
    templatetags: list[TemplateTag]
    templatefilters: list[TemplateFilter]


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

    # Preserve top-level registry structures
    # engine.libraries: {load_name: module_path} - the authoritative mapping
    libraries = dict(engine.libraries)
    # engine.builtins: ordered list of builtin module paths
    builtins = list(engine.builtins)

    # Collect builtins with Builtin provenance
    # Use zip to pair module paths (engine.builtins) with Library objects (engine.template_builtins)
    # Guard: these should always be the same length, but check to avoid silent data loss
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

    # Collect libraries with Library provenance, preserving load-name
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


def get_template_inventory() -> TemplateInventoryQueryData:
    """Get unified template inventory (tags + filters) in a single query.

    This is the preferred query for M4+. Returns everything needed for
    tag/filter validation and completions in one IPC round trip.
    """
    import django
    from django.apps import apps
    from django.template.engine import Engine
    from django.template.library import import_library

    if not apps.ready:
        django.setup()

    engine = Engine.get_default()
    templatetags: list[TemplateTag] = []
    templatefilters: list[TemplateFilter] = []

    # Preserve registry structures
    libraries = dict(engine.libraries)
    builtins = list(engine.builtins)

    # Sanity check
    if len(engine.builtins) != len(engine.template_builtins):
        raise RuntimeError(
            f"engine.builtins ({len(engine.builtins)}) and "
            f"engine.template_builtins ({len(engine.template_builtins)}) length mismatch"
        )

    # Collect builtins (both tags AND filters)
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

    # Collect library tags AND filters
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
    | TemplateInventoryQueryData  # NEW: unified inventory
)
