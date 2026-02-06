from __future__ import annotations

from pathlib import Path

from template_linter.resolution.runtime_registry import RuntimeRegistry
from template_linter.resolution.runtime_registry import build_runtime_environment


def test_build_runtime_environment_resolves_modules_from_extra_sys_path(
    tmp_path: Path,
) -> None:
    pkg = tmp_path / "myapp"
    (pkg / "templatetags").mkdir(parents=True)
    (pkg / "__init__.py").write_text("", encoding="utf-8")
    (pkg / "templatetags" / "__init__.py").write_text("", encoding="utf-8")
    (pkg / "templatetags" / "foo.py").write_text(
        "from django import template\n"
        "register = template.Library()\n"
        "@register.simple_tag\n"
        "def hi():\n"
        "    return 'x'\n",
        encoding="utf-8",
    )

    registry = RuntimeRegistry(
        libraries={"foo": "myapp.templatetags.foo"},
        builtins=[],
    )
    idx, builtins = build_runtime_environment(registry, extra_sys_path=[tmp_path])

    assert not builtins.rules
    assert idx.candidates("foo")
