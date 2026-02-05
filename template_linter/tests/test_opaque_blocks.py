from __future__ import annotations

from pathlib import Path

from template_linter.extraction.opaque import extract_opaque_blocks_from_django
from template_linter.extraction.opaque import extract_opaque_blocks_from_file


def test_django_opaque_blocks_detected(django_root: Path):
    opaque = extract_opaque_blocks_from_django(django_root)
    assert "comment" in opaque
    assert "endcomment" in opaque["comment"].end_tags
    assert "verbatim" in opaque
    assert "endverbatim" in opaque["verbatim"].end_tags
    assert opaque["verbatim"].match_suffix


def test_skip_past_custom_tag(tmp_path: Path):
    source = """
from django import template

register = template.Library()

@register.tag
def skip(parser, token):
    parser.skip_past("endskip")
    return None
"""
    path = tmp_path / "custom_tags.py"
    path.write_text(source)
    opaque = extract_opaque_blocks_from_file(path)
    assert "skip" in opaque
    assert opaque["skip"].end_tags == ["endskip"]


def test_manual_loop_custom_tag(tmp_path: Path):
    source = """
from django import template
from django.template.base import TokenType

register = template.Library()

@register.tag
def discard(parser, token):
    while parser.tokens:
        tok = parser.next_token()
        if tok.token_type == TokenType.BLOCK and tok.contents == "enddiscard":
            break
    return None
"""
    path = tmp_path / "custom_loop_tags.py"
    path.write_text(source)
    opaque = extract_opaque_blocks_from_file(path)
    assert "discard" in opaque
    assert opaque["discard"].end_tags == ["enddiscard"]


def test_loop_with_token_usage_is_not_opaque(tmp_path: Path):
    source = """
from django import template
from django.template.base import TokenType

register = template.Library()

@register.tag
def collect(parser, token):
    tokens = []
    while parser.tokens:
        tok = parser.next_token()
        if tok.token_type in (TokenType.TEXT, TokenType.VAR):
            tokens.append(tok)
            continue
        if tok.contents == "endcollect":
            break
        break
    return tokens
"""
    path = tmp_path / "custom_collect_tags.py"
    path.write_text(source)
    opaque = extract_opaque_blocks_from_file(path)
    assert "collect" not in opaque
