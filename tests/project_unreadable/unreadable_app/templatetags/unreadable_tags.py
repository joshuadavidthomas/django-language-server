from __future__ import annotations

from django import template

register = template.Library()


def hidden_tag(context):
    return context


register.simple_tag(takes_context=True)(globals()["hidden_tag"])
