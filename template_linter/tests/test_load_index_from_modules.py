from __future__ import annotations

from pathlib import Path

from template_linter.resolution.load import build_library_index_from_modules


def test_build_library_index_from_modules(tmp_path: Path) -> None:
    pkg = tmp_path / "pkg"
    (pkg / "templatetags").mkdir(parents=True)
    (pkg / "__init__.py").write_text("", encoding="utf-8")
    (pkg / "templatetags" / "__init__.py").write_text("", encoding="utf-8")
    (pkg / "templatetags" / "lib.py").write_text(
        """
from django import template

register = template.Library()

@register.tag()
def hello(parser, token):
    return ""
""".lstrip(),
        encoding="utf-8",
    )

    idx = build_library_index_from_modules(
        {"lib": "pkg.templatetags.lib"},
        extra_sys_path=[tmp_path],
    )
    candidates = idx.candidates("lib")
    assert len(candidates) == 1
    assert "hello" in candidates[0].bundle.rules
