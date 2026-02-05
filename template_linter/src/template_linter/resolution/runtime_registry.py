"""
Runtime registry integration (optional).

This module lets callers supply a registry of template libraries/builtins that
was discovered at runtime (e.g. via a Django Engine inspector) while still
extracting validation rules statically from source files.

This keeps the core validator "static": we never render templates or execute
tag/filter code. We only resolve module -> source file paths and parse them.
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path

from .bundle import ExtractionBundle
from .bundle import empty_bundle
from .bundle import extract_bundle_from_file
from .bundle import merge_bundles
from .load import LibraryIndex
from .load import build_library_index_from_modules
from .module_paths import resolve_module_to_path


@dataclass(frozen=True, slots=True)
class RuntimeRegistry:
    """
    A runtime-discovered registry of `{% load %}` libraries and builtins.

    This mirrors the `template_linter/corpus/inspect_runtime.py` JSON output:
    - libraries: mapping of `{% load %}` name -> module path
    - builtins: ordered list of module paths that are always loaded (later wins)
    """

    libraries: dict[str, str]
    builtins: list[str]


def load_runtime_registry(path: Path) -> RuntimeRegistry:
    data = json.loads(path.read_text(encoding="utf-8"))
    libs = data.get("libraries", {})
    builtins = data.get("builtins", [])
    if not isinstance(libs, dict) or not isinstance(builtins, list):
        raise ValueError(f"Invalid runtime registry JSON: {path}")
    libs_out: dict[str, str] = {}
    for k, v in libs.items():
        if isinstance(k, str) and isinstance(v, str):
            libs_out[k] = v
    builtins_out = [m for m in builtins if isinstance(m, str)]
    return RuntimeRegistry(libraries=libs_out, builtins=builtins_out)


def build_runtime_environment(
    registry: RuntimeRegistry,
    *,
    extra_sys_path: list[Path] | None = None,
) -> tuple[LibraryIndex, ExtractionBundle]:
    """
    Build a LibraryIndex and builtins bundle from a runtime registry.

    - The LibraryIndex is used for `{% load %}` resolution.
    - The builtins bundle is merged into the base rule/filter set (later wins),
      matching Django's builtin ordering semantics.
    """
    idx = build_library_index_from_modules(
        registry.libraries, extra_sys_path=extra_sys_path
    )
    builtins_bundle = _build_builtins_bundle(
        registry.builtins, extra_sys_path=extra_sys_path
    )
    return idx, builtins_bundle


def _build_builtins_bundle(
    module_paths: list[str],
    *,
    extra_sys_path: list[Path] | None = None,
) -> ExtractionBundle:
    out = empty_bundle()
    for module in module_paths:
        path = resolve_module_to_path(module, extra_sys_path=extra_sys_path)
        if path is None or path.suffix != ".py":
            continue
        out = merge_bundles(
            out,
            extract_bundle_from_file(path),
            collision_policy="override",
        )
    return out
