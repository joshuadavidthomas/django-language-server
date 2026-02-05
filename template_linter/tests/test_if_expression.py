from __future__ import annotations

import pytest
from django.template import TemplateSyntaxError
from django.template.defaulttags import TemplateIfParser

from template_linter.template_syntax.if_expression import validate_if_expression


class _DummyFilterExpr:
    def __init__(self, text: str) -> None:
        self.text = text

    def resolve(self, context, *, ignore_failures: bool = True):  # pragma: no cover
        return None


class _DummyTemplateParser:
    def compile_filter(self, value: str) -> _DummyFilterExpr:
        return _DummyFilterExpr(value)


def _django_validate(tokens: list[str]) -> str | None:
    try:
        TemplateIfParser(_DummyTemplateParser(), tokens).parse()
    except TemplateSyntaxError as e:
        return str(e)
    return None


@pytest.mark.parametrize(
    "tokens",
    [
        ["x"],
        ["not", "x"],
        ["x", "and", "y"],
        ["x", "or", "y"],
        ["x", "and", "not", "y"],
        ["x", "==", "y"],
        ["x", "!=", "y"],
        ["x", ">", "y"],
        ["x", ">=", "y"],
        ["x", "<", "y"],
        ["x", "<=", "y"],
        ["x", "in", "y"],
        ["x", "not", "in", "y"],
        ["x", "is", "y"],
        ["x", "is", "not", "y"],
        ["x|length", ">=", "5"],
    ],
)
def test_validate_if_expression_valid(tokens: list[str]) -> None:
    assert validate_if_expression(tokens) is None
    assert validate_if_expression(tokens) == _django_validate(tokens)


@pytest.mark.parametrize(
    "tokens",
    [
        [],
        ["and", "x"],
        ["x", "=="],
        ["x", "in"],
        ["not"],
        ["x", "y"],
        ["x", "and", "or", "y"],
        ["x", "not", "in"],
        ["x", "is", "not"],
    ],
)
def test_validate_if_expression_invalid(tokens: list[str]) -> None:
    assert validate_if_expression(tokens) == _django_validate(tokens)
