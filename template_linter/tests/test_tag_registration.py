from __future__ import annotations

from pathlib import Path

from template_linter.extraction.api import extract_from_file


def test_extract_from_file_resolves_constant_tag_name(tmp_path: Path) -> None:
    path = tmp_path / "lib.py"
    path.write_text(
        """
from django import template

register = template.Library()

TAG_NAME = "escapescript"

def compile(parser, token):
    return ""

register.tag(TAG_NAME, compile)
""".lstrip(),
        encoding="utf-8",
    )
    rules = extract_from_file(path)
    assert "escapescript" in rules


def test_extract_from_file_resolves_class_constant_in_decorator(tmp_path: Path) -> None:
    path = tmp_path / "lib.py"
    path.write_text(
        """
from django import template

register = template.Library()

class Node:
    TAG_NAME = "escapescript"

@register.tag(Node.TAG_NAME)
def compile(parser, token):
    return ""
""".lstrip(),
        encoding="utf-8",
    )
    rules = extract_from_file(path)
    assert "escapescript" in rules


def test_extract_from_file_recognizes_register_simple_block_tag_wrapper(
    tmp_path: Path,
) -> None:
    path = tmp_path / "lib.py"
    path.write_text(
        """
from django import template

register = template.Library()

def register_simple_block_tag(library, func=None, takes_context=None, name=None, end_name=None):
    def dec(func):
        return func
    return dec

@register_simple_block_tag(register, name="dialog")
def dialog(content, html_id, title):
    return content
""".lstrip(),
        encoding="utf-8",
    )
    rules = extract_from_file(path)
    assert "dialog" in rules
