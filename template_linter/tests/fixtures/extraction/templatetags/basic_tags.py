from __future__ import annotations

from django import template
from django.template import TemplateSyntaxError

register = template.Library()


@register.simple_tag
def hello(name):
    return f"hello {name}"


@register.filter
def trim(value):
    return str(value).strip()


@register.tag("strict_two_args")
def strict_two_args(parser, token):
    bits = token.split_contents()
    if len(bits) != 3:
        raise TemplateSyntaxError("strict_two_args requires exactly two arguments")
    return template.Node()
