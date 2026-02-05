from __future__ import annotations

from django import template

register = template.Library()


def _impl(a, b=1, *, c=None):
    return (a, b, c)


# Name override passed explicitly (common in real-world libs).
register.simple_tag(_impl, name="alias_simple_tag")
register.filter(_impl, name="alias_filter")  # type: ignore


class Handler:
    @staticmethod
    def handle(parser, token):
        return template.Node()


# Callable is an attribute expression, not a bare Name.
register.tag("classy", Handler.handle)
