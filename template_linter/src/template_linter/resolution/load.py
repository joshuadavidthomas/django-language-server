"""
Static `{% load %}` resolution.

This module intentionally avoids Django runtime imports. It resolves template
`{% load %}` directives against a statically-built index of `templatetags/*.py`
modules.
"""

from __future__ import annotations

import ast
from dataclasses import dataclass
from pathlib import Path

from .bundle import ExtractionBundle
from .bundle import empty_bundle
from .bundle import extract_bundle_from_file
from .bundle import merge_bundles
from .bundle import select_from_bundle
from .module_paths import resolve_module_to_path


@dataclass(frozen=True, slots=True)
class LibraryModule:
    """
    A single `templatetags/*.py` module discovered under a root.

    `name` is the library name used by `{% load <name> %}` (module basename).
    """

    name: str
    path: Path
    bundle: ExtractionBundle

    @property
    def tags(self) -> set[str]:
        return set(self.bundle.rules)

    @property
    def filters(self) -> set[str]:
        return set(self.bundle.filters)


@dataclass(frozen=True, slots=True)
class LibraryIndex:
    """
    Static mapping from library name -> candidate module(s).

    Multiple candidates can exist for the same name across a project with
    multiple apps. Without an installed-app order, resolution is ambiguous, so
    callers should treat such libraries conservatively.
    """

    libraries: dict[str, list[LibraryModule]]

    def candidates(self, name: str) -> list[LibraryModule]:
        return self.libraries.get(name, [])


@dataclass(frozen=True, slots=True)
class LoadError:
    message: str
    library: str | None = None


@dataclass(frozen=True, slots=True)
class LoadResolution:
    bundle: ExtractionBundle
    errors: list[LoadError]


def build_library_index(root: Path) -> LibraryIndex:
    """
    Build a LibraryIndex from `templatetags/**/*.py` modules under `root`.
    """
    libs: dict[str, list[LibraryModule]] = {}
    for path in sorted(root.rglob("templatetags/**/*.py")):
        # Ignore package boilerplate.
        if path.name == "__init__.py":
            continue
        name = path.stem
        rel = path.relative_to(root).with_suffix("")
        module = ".".join(rel.parts)
        bundle = _extract_bundle_for_library_module(
            module, path, extra_sys_path=[root]
        )
        mod = LibraryModule(name=name, path=path, bundle=bundle)
        libs.setdefault(name, []).append(mod)
    return LibraryIndex(libraries=libs)


def build_library_index_from_modules(
    libraries: dict[str, str],
    *,
    extra_sys_path: list[Path] | None = None,
) -> LibraryIndex:
    """
    Build a LibraryIndex from a mapping of `{% load %}` library name -> module path.

    This is intended to be fed by a runtime inspector (e.g. Django Engine.libraries)
    while still extracting validation rules statically from source files.
    """
    libs: dict[str, list[LibraryModule]] = {}
    for lib_name, module in libraries.items():
        path = resolve_module_to_path(module, extra_sys_path=extra_sys_path)
        if path is None or path.suffix != ".py":
            continue
        bundle = _extract_bundle_for_library_module(
            module, path, extra_sys_path=extra_sys_path
        )
        mod = LibraryModule(name=lib_name, path=path, bundle=bundle)
        libs.setdefault(lib_name, []).append(mod)

    return LibraryIndex(libraries=libs)


def _extract_bundle_for_library_module(
    module: str,
    path: Path,
    *,
    extra_sys_path: list[Path] | None,
) -> ExtractionBundle:
    """
    Extract a bundle for a template library module, including `import *` sources.

    Some template libraries (e.g. django-crispy-forms) implement a "meta library"
    by doing `from ...other_library import *` so that the exported `register`
    instance is effectively sourced from the imported module. To match Django's
    runtime behavior without importing/executing code, we statically merge direct
    star-import targets into the extracted bundle.
    """
    base = extract_bundle_from_file(path)

    try:
        tree = ast.parse(path.read_text(encoding="utf-8", errors="replace"))
    except SyntaxError:
        return base
    except OSError:
        return base

    imported = empty_bundle()
    for node in tree.body:
        if not isinstance(node, ast.ImportFrom):
            continue
        if not node.names or any(a.name != "*" for a in node.names):
            continue
        # Resolve relative imports against the current module path.
        if node.level and module:
            parts = module.split(".")
            # `from .x import *` has level=1, meaning "relative to package".
            prefix = parts[: max(0, len(parts) - node.level)]
            if node.module:
                target = ".".join(prefix + node.module.split("."))
            else:
                target = ".".join(prefix)
        else:
            target = node.module or ""
        if not target:
            continue
        target_path = resolve_module_to_path(target, extra_sys_path=extra_sys_path)
        if target_path is None or target_path.suffix != ".py":
            continue
        imported = merge_bundles(
            imported,
            extract_bundle_from_file(target_path),
            collision_policy="override",
        )

    if not imported.rules and not imported.filters and not imported.block_specs:
        return base
    return merge_bundles(imported, base, collision_policy="override")


def resolve_load_tokens(
    tokens: list[str],
    *,
    django_index: LibraryIndex | None = None,
    entry_index: LibraryIndex | None = None,
) -> LoadResolution:
    """
    Resolve a `{% load %}` tag's split tokens into an ExtractionBundle.

    Supports:
    - `{% load lib1 lib2 %}`
    - `{% load name1 name2 from lib %}` (selective imports)
    """
    if not tokens or tokens[0] != "load":
        return LoadResolution(bundle=empty_bundle(), errors=[])

    errors: list[LoadError] = []
    payload = tokens[1:]
    if not payload:
        return LoadResolution(bundle=empty_bundle(), errors=[])

    from_idx = None
    try:
        from_idx = payload.index("from")
    except ValueError:
        from_idx = None

    out = empty_bundle()

    def _candidates(name: str) -> list[LibraryModule]:
        candidates: list[LibraryModule] = []
        if django_index is not None:
            candidates.extend(django_index.candidates(name))
        if entry_index is not None:
            candidates.extend(entry_index.candidates(name))
        return candidates

    def _merge_candidates(cands: list[LibraryModule]) -> ExtractionBundle:
        # If multiple apps provide a same-named library, we don't know which one
        # Django would pick (depends on INSTALLED_APPS order). Be conservative:
        # union them but omit collisions.
        merged = empty_bundle()
        for mod in cands:
            merged = merge_bundles(merged, mod.bundle, collision_policy="omit")
        return merged

    if from_idx is not None:
        # `{% load name1 name2 from lib %}`
        names = payload[:from_idx]
        rest = payload[from_idx + 1 :]
        if not names or len(rest) != 1:
            return LoadResolution(bundle=empty_bundle(), errors=[])
        lib = rest[0]
        # Legacy Django compatibility: `{% load url from future %}`.
        #
        # The "future" library was removed long ago, but some third-party
        # templates still carry this pattern. In modern Django, `{% url %}` is
        # available without the library, so treat this as a no-op rather than
        # emitting an unknown-library error in strict corpus validation.
        if lib == "future" and set(names) <= {"url"}:
            return LoadResolution(bundle=empty_bundle(), errors=[])
        cands = _candidates(lib)
        if not cands:
            errors.append(
                LoadError(message=f"Unknown template library '{lib}'", library=lib)
            )
            return LoadResolution(bundle=empty_bundle(), errors=errors)

        wanted = set(names)
        # For each candidate module, include only requested exports.
        filtered = empty_bundle()
        missing = set(wanted)
        for mod in cands:
            tags = wanted & mod.tags
            filts = wanted & mod.filters
            if tags or filts:
                missing -= tags
                missing -= filts
                filtered = merge_bundles(
                    filtered,
                    select_from_bundle(
                        mod.bundle, tags=tags or set(), filters=filts or set()
                    ),
                    collision_policy="omit" if len(cands) > 1 else "override",
                )

        if missing:
            # Django would raise; treat as an error in strict modes.
            errors.append(
                LoadError(
                    message=f"Names not found in template library '{lib}': {sorted(missing)}",
                    library=lib,
                )
            )
        out = merge_bundles(out, filtered, collision_policy="override")
        return LoadResolution(bundle=out, errors=errors)

    # `{% load lib1 lib2 %}`
    for lib in payload:
        if lib == "future":
            continue
        cands = _candidates(lib)
        if not cands:
            errors.append(
                LoadError(message=f"Unknown template library '{lib}'", library=lib)
            )
            continue
        merged = _merge_candidates(cands)
        out = merge_bundles(out, merged, collision_policy="override")

    return LoadResolution(bundle=out, errors=errors)
