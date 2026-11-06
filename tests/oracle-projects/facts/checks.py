from __future__ import annotations

from django import template
from django.template import TemplateSyntaxError
from django.template.base import TextNode

register = template.Library()


@register.filter
def self_filter(self, value):
    return value


@register.filter
def impossible():
    return ""


@register.tag
def specialized(parser, token):
    if token.split_contents()[1:] != ["unique-syntax"]:
        raise TemplateSyntaxError("expected unique-syntax")
    parser.parse(("endspecialized",))
    parser.delete_first_token()
    return TextNode("")


@register.tag
def catches_unclosed(parser, token):
    try:
        parser.parse(("endcatch",))
    except TemplateSyntaxError:
        return TextNode("fallback")
    parser.delete_first_token()
    return TextNode("")


# This module imports our register and extends it, as InvenTree's generic library does.
import imported_registration  # noqa: E402, F401
