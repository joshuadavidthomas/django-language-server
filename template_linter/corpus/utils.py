from __future__ import annotations

import re
from pathlib import Path

_ARCHIVE_SUFFIXES = (
    ".tar.gz",
    ".tar.bz2",
    ".tar.xz",
    ".tgz",
    ".zip",
    ".whl",
)


def to_pip_requirement(name: str, version: str) -> str:
    """
    Convert a manifest version string into a pip requirement.

    Supported forms:
    - "6.0"  -> "Name==6.0" (exact)
    - "6.0.2" -> "Name==6.0.2" (pinned)
    - "6.0.*" -> "Name==6.0.*" (explicit floating patch)
    """
    v = version.strip()
    if "*" in v:
        return f"{name}=={v}"
    return f"{name}=={v}"


def strip_archive_suffix(path: Path) -> str:
    """
    Return the filename with common sdist suffixes removed.

    Example:
    - django-6.0.2.tar.gz -> django-6.0.2
    """
    s = path.name
    for suf in _ARCHIVE_SUFFIXES:
        if s.endswith(suf):
            return s[: -len(suf)]
    # Fall back to a single suffix.
    return path.stem


def infer_sdist_version(name: str, archive: Path) -> str | None:
    """
    Infer the version from an sdist or wheel filename for a given project name.

    This is best-effort and intentionally avoids importing packaging.
    """
    base = strip_archive_suffix(archive)
    norm_base = base.lower().replace("-", "_")
    norm_name = name.lower().replace("-", "_")

    # Wheel filenames are:
    #   {distribution}-{version}(-{build tag})?-{python tag}-{abi tag}-{platform tag}.whl
    # Split on '-' and take the second component.
    if archive.suffix == ".whl":
        parts = base.split("-")
        if len(parts) >= 2:
            dist = parts[0].lower().replace("-", "_")
            if dist == norm_name:
                return parts[1]

    m = re.match(rf"^{re.escape(norm_name)}[-_](?P<version>.+)$", norm_base)
    if not m:
        return None
    return m.group("version")
