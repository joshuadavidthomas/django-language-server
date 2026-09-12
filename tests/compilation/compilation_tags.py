"""Tag registration and signature examples for template compilation."""

from __future__ import annotations

import functools
from functools import partial

from django import template

register = template.Library()


@register.simple_tag
def authored_required(value):
    return value


@register.simple_tag
def authored_keyword_only(*, required):
    return required


@register.simple_tag
def authored_default(one, two="default"):
    return f"{one}:{two}"


@register.simple_tag(takes_context=True)
def decorator_context(context, value):
    return value


def direct_context(context, value):
    return value


register.simple_tag(direct_context, takes_context=True)


def curried_context(context, value):
    return value


register.simple_tag(takes_context=True)(curried_context)


@register.simple_block_tag
def authored_panel(content, title):
    return f"{title}:{content}"


def helper_tag(parser, token):
    bits = token.split_contents()
    if len(bits) != 2:
        raise template.TemplateSyntaxError("'helper_tag' requires one argument")
    return template.Node()


register.tag("helper_tag", helper_tag)


class ClassTag(template.Node):
    def __init__(self, parser, token):
        bits = token.split_contents()
        if len(bits) != 2:
            raise template.TemplateSyntaxError("'class_tag' requires one argument")


register.tag("class_tag", ClassTag)


context_simple_tag = partial(register.simple_tag, takes_context=True)


@context_simple_tag
def stdlib_partial_context(context, value):
    return value


def logged(fn):
    @functools.wraps(fn)
    def inner(*args, **kwargs):
        return fn(*args, **kwargs)

    return inner


def stdlib_wrapped(value):
    return value


register.simple_tag(logged(stdlib_wrapped))
