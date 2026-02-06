from __future__ import annotations

from pathlib import Path


def _unique_existing_dirs(paths: list[Path]) -> list[Path]:
    seen: set[Path] = set()
    out: list[Path] = []
    for p in paths:
        try:
            rp = p.resolve()
        except OSError:
            continue
        if rp in seen:
            continue
        if not rp.exists() or not rp.is_dir():
            continue
        seen.add(rp)
        out.append(rp)
    return out


def default_module_search_roots(*, extra_sys_path: list[Path] | None = None) -> list[Path]:
    """
    Return filesystem roots to search for module source files.

    This intentionally avoids importing modules (and thus avoids executing any
    package `__init__.py`) by searching the filesystem directly.
    """
    roots: list[Path] = []
    if extra_sys_path:
        roots.extend(extra_sys_path)
    try:
        import sys

        for p in sys.path:
            if not isinstance(p, str) or not p:
                continue
            roots.append(Path(p))
    except Exception:
        # If sys.path isn't available for some reason, just use extra_sys_path.
        pass
    return _unique_existing_dirs(roots)


def resolve_module_to_path(
    module: str,
    *,
    extra_sys_path: list[Path] | None = None,
) -> Path | None:
    """
    Resolve a dotted module path to a filesystem path without importing it.

    Prefers `module.py`, falling back to `module/__init__.py`.
    """
    if not module:
        return None
    rel = Path(*module.split("."))
    roots = default_module_search_roots(extra_sys_path=extra_sys_path)
    for root in roots:
        candidate = (root / rel).with_suffix(".py")
        if candidate.exists() and candidate.is_file():
            return candidate
        pkg_init = root / rel / "__init__.py"
        if pkg_init.exists() and pkg_init.is_file():
            return pkg_init
    return None
