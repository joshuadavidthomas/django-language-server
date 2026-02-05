from __future__ import annotations

import zipfile
import json
from pathlib import Path

from corpus.sync import _extract_archive
from corpus.sync import _find_distribution_archive
from corpus.sync import _GLOBAL_LIBRARY_PROVIDERS_CACHE
from corpus.sync import _discover_template_builtins
from corpus.sync import _write_runtime_registry


def test_find_distribution_archive_prefers_newest(tmp_path: Path) -> None:
    # This helper should consider both sdists and wheels.
    older = tmp_path / "django_debug_toolbar-4.4.5-py3-none-any.whl"
    older.write_bytes(b"not-a-real-wheel")
    newer = tmp_path / "django_debug_toolbar-4.4.6-py3-none-any.whl"
    newer.write_bytes(b"not-a-real-wheel")

    # Make sure mtimes are distinct and ordered.
    older.touch()
    newer.touch()

    found = _find_distribution_archive(tmp_path, "django-debug-toolbar")
    assert found == newer


def test_extract_archive_supports_whl(tmp_path: Path) -> None:
    archive = tmp_path / "demo-1.0.0-py3-none-any.whl"
    dest = tmp_path / "out"
    dest.mkdir()

    with zipfile.ZipFile(archive, "w") as zf:
        zf.writestr("pkg/__init__.py", "")
        zf.writestr("pkg/templates/example.html", "hi")

    _extract_archive(archive, dest)

    assert (dest / "pkg" / "__init__.py").exists()
    assert (dest / "pkg" / "templates" / "example.html").read_text(encoding="utf-8") == "hi"


def test_write_runtime_registry_resolves_cross_entry_loads(tmp_path: Path) -> None:
    corpus_root = tmp_path / ".corpus"
    provider = corpus_root / "packages" / "provider" / "1"
    consumer = corpus_root / "packages" / "consumer" / "1"

    (provider / "app" / "templatetags").mkdir(parents=True)
    (provider / "app" / "templatetags" / "__init__.py").write_text("", encoding="utf-8")
    (provider / "app" / "templatetags" / "dep.py").write_text(
        "from django import template\nregister = template.Library()\n",
        encoding="utf-8",
    )

    (consumer / "templates").mkdir(parents=True)
    (consumer / "templates" / "x.html").write_text(
        "{% load dep %}\n", encoding="utf-8"
    )

    _GLOBAL_LIBRARY_PROVIDERS_CACHE.pop(corpus_root, None)
    _write_runtime_registry(consumer)

    data = json.loads((consumer / ".runtime_registry.json").read_text(encoding="utf-8"))
    assert data["libraries"]["dep"] == "app.templatetags.dep"


def test_write_runtime_registry_includes_templatetags_builtins(tmp_path: Path) -> None:
    root = tmp_path / ".corpus" / "repos" / "proj" / "1"
    builtins_dir = root / "app" / "templatetags" / "builtins"
    builtins_dir.mkdir(parents=True)
    (root / "app" / "templatetags" / "__init__.py").write_text("", encoding="utf-8")
    (builtins_dir / "__init__.py").write_text("", encoding="utf-8")
    (builtins_dir / "tags.py").write_text(
        "from django import template\nregister = template.Library()\n", encoding="utf-8"
    )
    (builtins_dir / "filters.py").write_text(
        "from django import template\nregister = template.Library()\n", encoding="utf-8"
    )

    _write_runtime_registry(root)
    data = json.loads((root / ".runtime_registry.json").read_text(encoding="utf-8"))

    assert "app.templatetags.builtins.tags" in data["builtins"]
    assert "app.templatetags.builtins.filters" in data["builtins"]


def test_discover_template_builtins_from_ast(tmp_path: Path) -> None:
    root = tmp_path / "entry"
    root.mkdir()
    (root / "settings.py").write_text(
        'TEMPLATES = [{"OPTIONS": {"builtins": ["x.y", "a.b"]}}]\n',
        encoding="utf-8",
    )
    assert _discover_template_builtins(root) == ["x.y", "a.b"]
