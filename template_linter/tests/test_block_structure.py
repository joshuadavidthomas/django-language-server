from __future__ import annotations

import pytest

from template_linter.extraction.api import extract_from_file
from template_linter.extraction.structural import extract_block_specs_from_file
from template_linter.validation.template import validate_template


def test_extracts_block_specs_for_if_and_for(django_root, block_specs):
    # Smoke test: ensure we extract delimiter tags for common block tags.
    by_start = {}
    for spec in block_specs:
        for start in spec.start_tags:
            by_start[start] = spec

    assert "if" in by_start
    if_spec = by_start["if"]
    assert "endif" in if_spec.end_tags
    assert "else" in if_spec.middle_tags
    assert "elif" in if_spec.middle_tags

    assert "for" in by_start
    for_spec = by_start["for"]
    assert "endfor" in for_spec.end_tags
    assert "empty" in for_spec.middle_tags


@pytest.mark.parametrize(
    "template",
    [
        "{% else %}",
        "{% endif %}",
        "{% empty %}",
        "{% endfor %}",
    ],
)
def test_delimiters_outside_blocks_error(template, rules, opaque_blocks, block_specs):
    errors = validate_template(
        template,
        rules,
        opaque_blocks=opaque_blocks,
        block_specs=block_specs,
        report_unknown_tags=True,
    )
    assert errors
    assert any("Unexpected" in e.message for e in errors)


def test_mismatched_end_tag_errors(rules, opaque_blocks, block_specs):
    template = "{% if x %}{% endfor %}"
    errors = validate_template(
        template,
        rules,
        opaque_blocks=opaque_blocks,
        block_specs=block_specs,
        report_unknown_tags=True,
    )
    assert errors
    assert any("Mismatched" in e.message for e in errors)


def test_unclosed_block_errors(rules, opaque_blocks, block_specs):
    template = "{% if x %}hi"
    errors = validate_template(
        template,
        rules,
        opaque_blocks=opaque_blocks,
        block_specs=block_specs,
        report_unknown_tags=True,
    )
    assert errors
    assert any("Unclosed" in e.message for e in errors)


def test_duplicate_else_errors(rules, opaque_blocks, block_specs):
    template = "{% if x %}a{% else %}b{% else %}c{% endif %}"
    errors = validate_template(
        template,
        rules,
        opaque_blocks=opaque_blocks,
        block_specs=block_specs,
        report_unknown_tags=True,
    )
    assert errors
    assert any("Duplicate 'else'" in e.message for e in errors)


def test_elif_after_else_errors(rules, opaque_blocks, block_specs):
    template = "{% if x %}a{% else %}b{% elif y %}c{% endif %}"
    errors = validate_template(
        template,
        rules,
        opaque_blocks=opaque_blocks,
        block_specs=block_specs,
        report_unknown_tags=True,
    )
    assert errors
    assert any("after terminal delimiter" in e.message for e in errors)


def test_duplicate_empty_errors(rules, opaque_blocks, block_specs):
    template = "{% for x in y %}a{% empty %}b{% empty %}c{% endfor %}"
    errors = validate_template(
        template,
        rules,
        opaque_blocks=opaque_blocks,
        block_specs=block_specs,
        report_unknown_tags=True,
    )
    assert errors
    assert any("Duplicate 'empty'" in e.message for e in errors)


def test_plural_outside_blocktranslate_errors(rules, opaque_blocks, block_specs):
    template = "{% plural %}"
    errors = validate_template(
        template,
        rules,
        opaque_blocks=opaque_blocks,
        block_specs=block_specs,
        report_unknown_tags=True,
    )
    assert errors
    assert any("Unexpected 'plural'" in e.message for e in errors)


def test_endblocktranslate_outside_blocktranslate_errors(
    rules, opaque_blocks, block_specs
):
    template = "{% endblocktranslate %}"
    errors = validate_template(
        template,
        rules,
        opaque_blocks=opaque_blocks,
        block_specs=block_specs,
        report_unknown_tags=True,
    )
    assert errors
    assert any("Unexpected 'endblocktranslate'" in e.message for e in errors)


def test_manual_loop_end_tag_block_spec_detected(tmp_path):
    source = """
from django import template
from django.template.base import TokenType
from django.template import TemplateSyntaxError

register = template.Library()

@register.tag
def myblock(parser, token):
    bits = token.split_contents()
    while parser.tokens:
        tok = parser.next_token()
        if tok.token_type == TokenType.TEXT:
            continue
        break
    end_tag_name = "end%s" % bits[0]
    if tok.contents.strip() != end_tag_name:
        raise TemplateSyntaxError("bad end")
    return None
"""
    path = tmp_path / "custom_tags.py"
    path.write_text(source)

    rules = extract_from_file(path)
    specs = extract_block_specs_from_file(path)

    assert "myblock" in rules
    assert any("endmyblock" in spec.end_tags for spec in specs)

    ok = "{% myblock %}{% endmyblock %}"
    errors = validate_template(ok, rules, block_specs=specs, report_unknown_tags=True)
    assert errors == []

    errors = validate_template(
        "{% endmyblock %}", rules, block_specs=specs, report_unknown_tags=True
    )
    assert errors
    assert any("Unexpected 'endmyblock'" in e.message for e in errors)

    errors = validate_template(
        "{% myblock %}", rules, block_specs=specs, report_unknown_tags=True
    )
    assert errors
    assert any("Unclosed 'myblock'" in e.message for e in errors)


def test_class_registered_tag_block_spec_detected(tmp_path):
    source = """
from django import template

register = template.Library()

class Options:
    def __init__(self, *args, **kwargs):
        pass

class Parser:
    pass

class AddtoblockParser(Parser):
    def parse_blocks(self):
        name = "x"
        self.blocks = {}
        self.parser.parse(('endaddtoblock', 'endaddtoblock %s' % name))
        self.parser.delete_first_token()

class Tag:
    pass

class Addtoblock(Tag):
    options = Options(parser_class=AddtoblockParser)

register.tag('addtoblock', Addtoblock)
"""
    path = tmp_path / "sekizai_like.py"
    path.write_text(source)

    rules = extract_from_file(path)
    specs = extract_block_specs_from_file(path)

    assert "addtoblock" in rules
    assert any("endaddtoblock" in spec.end_tags for spec in specs)

    ok = "{% addtoblock x %}hi{% endaddtoblock %}"
    errors = validate_template(ok, rules, block_specs=specs, report_unknown_tags=True)
    assert errors == []

    errors = validate_template(
        "{% endaddtoblock %}", rules, block_specs=specs, report_unknown_tags=True
    )
    assert errors
    assert any("Unexpected 'endaddtoblock'" in e.message for e in errors)


def test_returned_node_init_parse_block_spec_detected(tmp_path):
    source = """
from django import template

register = template.Library()

class MyNode:
    def __init__(self, parser, token):
        self.nodelist = parser.parse(("endmytag",))
        parser.delete_first_token()

@register.tag
def mytag(parser, token):
    return MyNode(parser, token)
"""
    path = tmp_path / "node_like.py"
    path.write_text(source)

    rules = extract_from_file(path)
    specs = extract_block_specs_from_file(path)

    assert "mytag" in rules
    assert any("endmytag" in spec.end_tags for spec in specs)

    ok = "{% mytag %}hi{% endmytag %}"
    errors = validate_template(ok, rules, block_specs=specs, report_unknown_tags=True)
    assert errors == []


def test_register_simple_block_tag_decorator_block_spec_detected(tmp_path):
    source = """
from django import template

register = template.Library()

def register_simple_block_tag(library, *args, **kwargs):
    def dec(func):
        return func
    return dec

@register_simple_block_tag(register)
def dialog(content, *args, **kwargs):
    return content
"""
    path = tmp_path / "pretix_like.py"
    path.write_text(source)

    rules = extract_from_file(path)
    specs = extract_block_specs_from_file(path)

    assert "dialog" in rules
    assert any("enddialog" in spec.end_tags for spec in specs)

    ok = "{% dialog %}hi{% enddialog %}"
    errors = validate_template(ok, rules, block_specs=specs, report_unknown_tags=True)
    assert errors == []


def test_parse_until_suffix_values_are_canonicalized(tmp_path):
    source = """
from django import template

register = template.Library()

@register.tag
def x(parser, token):
    valid_end = ("endx", "endx foo")
    parser.parse(valid_end)
    return None
"""
    path = tmp_path / "custom_tags.py"
    path.write_text(source)
    specs = extract_block_specs_from_file(path)
    assert specs
    spec = specs[0]
    assert spec.end_tags == ("endx",)


def test_duplicate_plural_inside_blocktranslate_errors(
    rules, opaque_blocks, structural_rules, block_specs
):
    template = (
        "{% blocktranslate count a=1 %}x"
        "{% plural %}y"
        "{% plural %}z"
        "{% endblocktranslate %}"
    )
    errors = validate_template(
        template,
        rules,
        opaque_blocks=opaque_blocks,
        structural_rules=structural_rules,
        block_specs=block_specs,
        report_unknown_tags=True,
    )
    assert errors
    assert any("Duplicate 'plural'" in e.message for e in errors)


def test_endblock_name_mismatch_errors(rules, opaque_blocks, block_specs):
    template = "{% block foo %}x{% endblock bar %}"
    errors = validate_template(
        template,
        rules,
        opaque_blocks=opaque_blocks,
        block_specs=block_specs,
        report_unknown_tags=True,
    )
    assert errors
    assert any(
        "endblock" in e.message and "mismatch" in e.message.lower() for e in errors
    )
    assert not any("Unclosed 'block'" in e.message for e in errors)


def test_endblock_name_match_ok(rules, opaque_blocks, block_specs):
    template = "{% block foo %}x{% endblock foo %}"
    errors = validate_template(
        template,
        rules,
        opaque_blocks=opaque_blocks,
        block_specs=block_specs,
        report_unknown_tags=True,
    )
    assert errors == []


def test_endblock_extra_tokens_error(rules, opaque_blocks, block_specs):
    template = "{% block foo %}x{% endblock foo bar %}"
    errors = validate_template(
        template,
        rules,
        opaque_blocks=opaque_blocks,
        block_specs=block_specs,
        report_unknown_tags=True,
    )
    assert errors
    assert any(
        "too many" in e.message.lower() and "endblock" in e.message for e in errors
    )
    assert not any("Unclosed 'block'" in e.message for e in errors)


def test_partialdef_end_name_mismatch_errors(rules, opaque_blocks, block_specs):
    template = "{% partialdef foo %}x{% endpartialdef bar %}"
    errors = validate_template(
        template,
        rules,
        opaque_blocks=opaque_blocks,
        block_specs=block_specs,
        report_unknown_tags=True,
    )
    assert errors
    assert any(
        "endpartialdef" in e.message and "mismatch" in e.message.lower() for e in errors
    )
    assert not any("Unclosed 'partialdef'" in e.message for e in errors)


def test_partialdef_end_name_match_ok(rules, opaque_blocks, block_specs):
    template = "{% partialdef foo %}x{% endpartialdef foo %}"
    errors = validate_template(
        template,
        rules,
        opaque_blocks=opaque_blocks,
        block_specs=block_specs,
        report_unknown_tags=True,
    )
    assert errors == []


def test_partialdef_end_extra_tokens_error(rules, opaque_blocks, block_specs):
    template = "{% partialdef foo %}x{% endpartialdef foo bar %}"
    errors = validate_template(
        template,
        rules,
        opaque_blocks=opaque_blocks,
        block_specs=block_specs,
        report_unknown_tags=True,
    )
    assert errors
    assert any(
        "too many" in e.message.lower() and "endpartialdef" in e.message for e in errors
    )
    assert not any("Unclosed 'partialdef'" in e.message for e in errors)
