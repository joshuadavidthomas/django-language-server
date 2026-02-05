"""
Runtime inspector to discover installed `{% load %}` libraries.

This is an *optional* tool for aligning static validation with real project
environments, similar to django-language-server's inspector. It does not render
templates; it only queries Django's configured template Engine.

Output:
- JSON mapping `{library_name: module_path}` for Engine.libraries
- JSON list of builtin library module paths from Engine.builtins

This output can be used to build a LibraryIndex via
`template_linter.resolution.load.build_library_index_from_modules()`, while still doing
AST extraction from source.
"""

from __future__ import annotations

import argparse
import json
import os
from dataclasses import asdict
from dataclasses import dataclass


@dataclass(frozen=True, slots=True)
class InspectorOutput:
    libraries: dict[str, str]
    builtins: list[str]


def inspect_engine(engine) -> InspectorOutput:
    libs = dict(engine.libraries)
    # Preserve Engine ordering (later builtins override earlier ones).
    seen: set[str] = set()
    builtins: list[str] = []
    for mod in engine.builtins:
        if mod in seen:
            continue
        seen.add(mod)
        builtins.append(mod)
    return InspectorOutput(libraries=libs, builtins=builtins)


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Inspect a Django project's template Engine for libraries/builtins."
    )
    parser.add_argument(
        "--settings",
        required=True,
        help="DJANGO_SETTINGS_MODULE value to use for initializing Django.",
    )
    parser.add_argument(
        "--pythonpath",
        action="append",
        default=[],
        help="Extra path(s) to prepend to PYTHONPATH before importing Django.",
    )
    args = parser.parse_args()

    for p in reversed(args.pythonpath):
        os.environ["PYTHONPATH"] = f"{p}{os.pathsep}{os.environ.get('PYTHONPATH', '')}"

    os.environ["DJANGO_SETTINGS_MODULE"] = args.settings

    import django
    from django.apps import apps
    from django.template.engine import Engine

    if not apps.ready:
        django.setup()

    engine = Engine.get_default()

    out = inspect_engine(engine)
    print(json.dumps(asdict(out), indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
