from __future__ import annotations

from django import template

register = template.Library()


@register.simple_tag
def djls_greeting(name):
    return f"hello {name}"


@register.filter
def djls_shout(value):
    return str(value).upper()
