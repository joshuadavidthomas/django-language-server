from __future__ import annotations

from pathlib import Path

from template_linter.resolution.load import build_library_index_from_modules
from template_linter.resolution.runtime_registry import RuntimeRegistry
from template_linter.resolution.runtime_registry import build_runtime_environment


def test_build_library_index_from_modules_does_not_import_packages(tmp_path: Path) -> None:
    # If module resolution imports `badpkg`, this will raise.
    pkg = tmp_path / "badpkg"
    (pkg / "templatetags").mkdir(parents=True)
    (pkg / "__init__.py").write_text(
        "raise RuntimeError('imported badpkg')\n", encoding="utf-8"
    )
    (pkg / "templatetags" / "__init__.py").write_text("", encoding="utf-8")
    (pkg / "templatetags" / "badlib.py").write_text(
        "from django import template\nregister = template.Library()\n",
        encoding="utf-8",
    )

    idx = build_library_index_from_modules(
        {"badlib": "badpkg.templatetags.badlib"}, extra_sys_path=[tmp_path]
    )
    cands = idx.candidates("badlib")
    assert len(cands) == 1
    assert cands[0].path.name == "badlib.py"


def test_build_runtime_environment_does_not_import_packages(tmp_path: Path) -> None:
    pkg = tmp_path / "badpkg"
    pkg.mkdir()
    (pkg / "__init__.py").write_text(
        "raise RuntimeError('imported badpkg')\n", encoding="utf-8"
    )
    (pkg / "builtins.py").write_text(
        "from django import template\nregister = template.Library()\n",
        encoding="utf-8",
    )

    registry = RuntimeRegistry(libraries={}, builtins=["badpkg.builtins"])
    idx, builtins_bundle = build_runtime_environment(
        registry, extra_sys_path=[tmp_path]
    )
    assert idx.libraries == {}
    assert builtins_bundle.rules == {}

