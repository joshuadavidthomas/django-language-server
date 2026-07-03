from __future__ import annotations

from django import template

register = template.Library()


@register.simple_tag
def ambiguous_greeting(name):
    return f"alpha {name}"


@register.filter
def ambiguous_shout(value):
    return str(value).upper()
