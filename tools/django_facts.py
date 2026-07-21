from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path
from typing import Any


def main() -> None:
    args = parse_args()
    project = args.project.resolve()
    settings_module = args.settings or read_settings_module(project)

    sys.path.insert(0, str(project))
    os.environ["DJANGO_SETTINGS_MODULE"] = settings_module

    import django

    site_packages = Path(django.__file__).resolve().parents[1]
    django.setup()

    facts = {
        "template_dirs": collect_template_dirs(project, site_packages),
        "template_library_catalog": collect_template_library_catalog(),
    }
    json.dump(facts, sys.stdout, indent=2, sort_keys=True)
    sys.stdout.write("\n")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Generate normalized Django facts for the e2e fixture project."
    )
    parser.add_argument("--project", type=Path, required=True)
    parser.add_argument("--settings")
    return parser.parse_args()


def read_settings_module(project: Path) -> str:
    config = project / "djls.toml"
    for line in config.read_text(encoding="utf-8").splitlines():
        line = line.split("#", 1)[0].strip()
        if not line.startswith("django_settings_module"):
            continue
        _, raw_value = line.split("=", 1)
        return raw_value.strip().strip('"').strip("'")
    raise RuntimeError(f"{config} does not define django_settings_module")


def collect_template_dirs(project: Path, site_packages: Path) -> list[str]:
    from django.apps import apps
    from django.conf import settings

    dirs = []
    for engine in settings.TEMPLATES:
        if "django" not in engine["BACKEND"].lower():
            continue

        dirs.extend(Path(path) for path in engine.get("DIRS", []))

        if engine.get("APP_DIRS", False):
            for app_config in apps.get_app_configs():
                template_dir = Path(app_config.path) / "templates"
                if template_dir.exists():
                    dirs.append(template_dir)

    return [normalize_path(path, project, site_packages) for path in dirs]


def collect_template_library_catalog() -> dict[str, Any]:
    from django.template.engine import Engine
    from django.template.library import import_library

    engine = Engine.get_default()
    builtins = []
    symbols = []

    for builtin_module, library in zip(engine.builtins, engine.template_builtins):
        if builtin_module in builtins:
            continue
        builtins.append(builtin_module)
        symbols.extend(symbol_rows(library, builtin_module, None))

    libraries = dict(sorted(engine.libraries.items()))
    for load_name, library_module in libraries.items():
        try:
            library = import_library(library_module)
        except Exception as exc:
            raise RuntimeError(
                f"failed to import Django template library {library_module}"
            ) from exc
        symbols.extend(symbol_rows(library, library_module, load_name))

    symbols.sort(
        key=lambda symbol: (
            symbol["library_module"],
            symbol["load_name"] or "",
            symbol["kind"],
            symbol["name"],
            symbol["module"],
        )
    )
    return {
        "builtins": builtins,
        "libraries": libraries,
        "symbols": symbols,
    }


def symbol_rows(library: Any, library_module: str, load_name: str | None) -> list[dict[str, str | None]]:
    rows = []
    for name, tag_func in sorted(library.tags.items()):
        rows.append(
            {
                "kind": "tag",
                "name": name,
                "load_name": load_name,
                "library_module": library_module,
                "module": tag_func.__module__,
            }
        )
    for name, filter_func in sorted(library.filters.items()):
        rows.append(
            {
                "kind": "filter",
                "name": name,
                "load_name": load_name,
                "library_module": library_module,
                "module": filter_func.__module__,
            }
        )
    return rows


def normalize_path(path: Path, project: Path, site_packages: Path) -> str:
    path = path.resolve()
    if path == project or path.is_relative_to(project):
        return placeholder_path("${PROJECT}", path.relative_to(project))
    if path == site_packages or path.is_relative_to(site_packages):
        return placeholder_path("${SITE_PACKAGES}", path.relative_to(site_packages))
    return path.as_posix()


def placeholder_path(prefix: str, relative: Path) -> str:
    relative_path = relative.as_posix()
    if relative_path == ".":
        return prefix
    return f"{prefix}/{relative_path}"


if __name__ == "__main__":
    main()
