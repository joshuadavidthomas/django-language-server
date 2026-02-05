from __future__ import annotations

from pathlib import Path

from template_linter.resolution.load import build_library_index
from template_linter.types import TagValidation
from template_linter.validation.template import validate_template_with_load_resolution


def _write(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def _base_rules() -> dict[str, TagValidation]:
    # Ensure `{% load %}` itself isn't flagged as unknown in strict mode.
    return {"load": TagValidation(tag_name="load", unrestricted=True)}


def test_load_makes_tag_available(tmp_path: Path) -> None:
    root = tmp_path / "proj"
    _write(
        root / "app" / "templatetags" / "lib.py",
        """
from django import template

register = template.Library()

@register.tag()
def t(parser, token):
    bits = token.split_contents()
    if len(bits) != 2:
        raise template.TemplateSyntaxError("bad")
    return ""
""".lstrip(),
    )
    idx = build_library_index(root)
    tpl = "{% load lib %}{% t x %}"
    errors = validate_template_with_load_resolution(
        tpl,
        _base_rules(),
        django_index=None,
        entry_index=idx,
        report_unknown_tags=True,
        report_unknown_filters=True,
        report_unknown_libraries=True,
    )
    assert not errors


def test_load_is_position_aware(tmp_path: Path) -> None:
    root = tmp_path / "proj"
    _write(
        root / "app" / "templatetags" / "lib.py",
        """
from django import template

register = template.Library()

@register.tag()
def t(parser, token):
    return ""
""".lstrip(),
    )
    idx = build_library_index(root)
    tpl = "{% t x %}{% load lib %}"
    errors = validate_template_with_load_resolution(
        tpl,
        _base_rules(),
        django_index=None,
        entry_index=idx,
        report_unknown_tags=True,
        report_unknown_libraries=True,
    )
    assert any("Unknown tag 't'" in e.message for e in errors)


def test_load_order_overrides(tmp_path: Path) -> None:
    root = tmp_path / "proj"
    _write(
        root / "a" / "templatetags" / "lib1.py",
        """
from django import template

register = template.Library()

@register.tag()
def t(parser, token):
    bits = token.split_contents()
    if len(bits) != 2:
        raise template.TemplateSyntaxError("bad")
    return ""
""".lstrip(),
    )
    _write(
        root / "b" / "templatetags" / "lib2.py",
        """
from django import template

register = template.Library()

@register.tag()
def t(parser, token):
    bits = token.split_contents()
    if len(bits) != 3:
        raise template.TemplateSyntaxError("bad")
    return ""
""".lstrip(),
    )
    idx = build_library_index(root)

    # Last loaded wins -> expects 3 tokens -> passes.
    tpl = "{% load lib1 lib2 %}{% t a b %}"
    errors = validate_template_with_load_resolution(
        tpl,
        _base_rules(),
        django_index=None,
        entry_index=idx,
        report_unknown_tags=True,
        report_unknown_libraries=True,
    )
    assert not errors

    # Reverse order -> expects 2 tokens -> errors.
    tpl2 = "{% load lib2 lib1 %}{% t a b %}"
    errors2 = validate_template_with_load_resolution(
        tpl2,
        _base_rules(),
        django_index=None,
        entry_index=idx,
        report_unknown_tags=True,
        report_unknown_libraries=True,
    )
    assert any("Expected 2 tokens" in e.message for e in errors2)


def test_load_from_is_selective(tmp_path: Path) -> None:
    root = tmp_path / "proj"
    _write(
        root / "app" / "templatetags" / "lib.py",
        """
from django import template

register = template.Library()

@register.tag()
def t(parser, token):
    return ""

@register.tag()
def u(parser, token):
    return ""
""".lstrip(),
    )
    idx = build_library_index(root)
    tpl = "{% load t from lib %}{% t x %}{% u x %}"
    errors = validate_template_with_load_resolution(
        tpl,
        _base_rules(),
        django_index=None,
        entry_index=idx,
        report_unknown_tags=True,
        report_unknown_libraries=True,
    )
    assert any("Unknown tag 'u'" in e.message for e in errors)


def test_unknown_library_reports_error(tmp_path: Path) -> None:
    idx = build_library_index(tmp_path / "proj")
    tpl = "{% load does_not_exist %}"
    errors = validate_template_with_load_resolution(
        tpl,
        _base_rules(),
        django_index=None,
        entry_index=idx,
        report_unknown_libraries=True,
    )
    assert any("Unknown template library 'does_not_exist'" in e.message for e in errors)


def test_register_simple_tag_name_kw_is_recognized(tmp_path: Path) -> None:
    root = tmp_path / "proj"
    _write(
        root / "app" / "templatetags" / "lib.py",
        """
from django import template

register = template.Library()

def underlying(context):
    return ""

register.simple_tag(underlying, name="alias_tag")
""".lstrip(),
    )
    idx = build_library_index(root)
    tpl = "{% load lib %}{% alias_tag %}"
    errors = validate_template_with_load_resolution(
        tpl,
        _base_rules(),
        django_index=None,
        entry_index=idx,
        report_unknown_tags=True,
        report_unknown_libraries=True,
    )
    assert not errors


def test_load_url_from_future_is_noop(tmp_path: Path) -> None:
    # Support legacy `{% load url from future %}` by treating it as a no-op.
    tpl = "{% load url from future %}{% url 'x' %}"
    rules = {
        "load": TagValidation(tag_name="load", unrestricted=True),
        "url": TagValidation(tag_name="url", unrestricted=True),
    }
    errors = validate_template_with_load_resolution(
        tpl,
        rules,
        django_index=None,
        entry_index=build_library_index(tmp_path / "proj"),
        report_unknown_tags=True,
        report_unknown_libraries=True,
    )
    assert not errors


def test_load_module_with_star_import_provides_imported_filters(tmp_path: Path) -> None:
    root = tmp_path / "proj"
    _write(
        root / "app" / "templatetags" / "lib_filters.py",
        """
from django import template

register = template.Library()

@register.filter(name="f")
def f(val):
    return val
""".lstrip(),
    )
    _write(
        root / "app" / "templatetags" / "lib_tags.py",
        """
from django import template

register = template.Library()

from app.templatetags.lib_filters import *  # noqa: F401,F403
""".lstrip(),
    )
    idx = build_library_index(root)
    tpl = "{% load lib_tags %}{{ x|f }}"
    errors = validate_template_with_load_resolution(
        tpl,
        _base_rules(),
        base_filters={},
        django_index=None,
        entry_index=idx,
        report_unknown_filters=True,
        report_unknown_libraries=True,
    )
    assert not errors


def test_load_library_can_define_block_end_tags(tmp_path: Path) -> None:
    root = tmp_path / "proj"
    _write(
        root / "app" / "templatetags" / "lib.py",
        """
from django import template

register = template.Library()

@register.tag()
def compress(parser, token):
    # Delimiter tags like `endcompress` aren't registered; Django's parser stops
    # on them via parser.parse((...,)).
    parser.parse(("endcompress",))
    return ""
""".lstrip(),
    )
    idx = build_library_index(root)
    tpl = "{% load lib %}{% compress %}x{% endcompress %}"
    errors = validate_template_with_load_resolution(
        tpl,
        _base_rules(),
        base_filters={},
        django_index=None,
        entry_index=idx,
        report_unknown_tags=True,
        report_unknown_libraries=True,
    )
    assert not errors
