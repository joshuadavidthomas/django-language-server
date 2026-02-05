"""
Shared file discovery helpers for Django template tag/filter sources.
"""

from __future__ import annotations

from collections.abc import Iterable
from pathlib import Path


def iter_templatetag_files(django_root: Path) -> Iterable[Path]:
    yield from (django_root / "templatetags").rglob("*.py")
    yield from (django_root / "contrib").rglob("templatetags/*.py")


def iter_tag_files(django_root: Path) -> list[Path]:
    files: list[Path] = [
        django_root / "template" / "defaulttags.py",
        django_root / "template" / "loader_tags.py",
    ]
    files.extend(iter_templatetag_files(django_root))
    return [path for path in files if path.exists()]


def iter_filter_files(django_root: Path) -> list[Path]:
    files: list[Path] = [
        django_root / "template" / "defaultfilters.py",
    ]
    files.extend(iter_templatetag_files(django_root))
    return [path for path in files if path.exists()]
