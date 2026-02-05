from __future__ import annotations

from pathlib import Path

from template_linter.extraction.filters import extract_filters_from_file


def test_extract_filters_register_filter_decorator_call(tmp_path: Path) -> None:
    path = tmp_path / "lib.py"
    path.write_text(
        """
from django import template

register = template.Library()

def date_relative(value):
    return value

register.filter(is_safe=False)(date_relative)
""".lstrip(),
        encoding="utf-8",
    )
    filters = extract_filters_from_file(path)
    assert "date_relative" in filters


def test_extract_filters_creates_stub_for_unresolved_function(tmp_path: Path) -> None:
    path = tmp_path / "lib.py"
    path.write_text(
        """
from django import template

register = template.Library()

register.filter()(external_filter)
""".lstrip(),
        encoding="utf-8",
    )
    filters = extract_filters_from_file(path)
    assert "external_filter" in filters
    assert filters["external_filter"].unrestricted is True
