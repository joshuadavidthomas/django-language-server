"""Tag registration and signature examples for template compilation."""

from __future__ import annotations

import functools
from functools import partial

from django import template
from django.template.base import token_kwargs

register = template.Library()


@register.tag
def assignment_modern(parser, token):
    bits = token.split_contents()[1:]
    assignments = token_kwargs(bits, parser)
    if len(assignments) != 1:
        raise template.TemplateSyntaxError(
            "'assignment_modern' expected exactly one unique assignment"
        )
    if bits:
        raise template.TemplateSyntaxError("'assignment_modern' received trailing input")
    return template.Node()


@register.tag
def assignment_legacy(parser, token):
    bits = token.split_contents()[1:]
    assignments = token_kwargs(bits, parser, support_legacy=True)
    if not assignments:
        raise template.TemplateSyntaxError(
            "'assignment_legacy' expected at least one assignment"
        )
    if bits:
        raise template.TemplateSyntaxError("'assignment_legacy' received trailing input")
    return template.Node()


@register.simple_tag
def authored_required(value):
    return value


@register.simple_tag
def authored_empty():
    return ""


@register.simple_tag
def authored_keyword_only(*, required):
    return required


@register.simple_tag
def authored_default(one, two="default"):
    return f"{one}:{two}"


@register.simple_tag
def authored_positional_only(value, /):
    return value


@register.simple_tag
def authored_varargs(*values):
    return values


@register.simple_tag
def authored_kwargs(one, **options):
    return one, options


@register.simple_tag(name=None)
def none_named_simple(value):
    return value


@register.simple_tag(name="")
def empty_named_simple(value):
    return value


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


@register.simple_block_tag(
    takes_context=True,
    name="context_panel",
    end_name="close_context_panel",
)
def context_panel_impl(context, content, title):
    return f"{title}:{content}"


@register.simple_block_tag(name="default_panel", end_name=None)
def default_panel_impl(content, title):
    return f"{title}:{content}"


def curried_inclusion(value):
    return {"value": value}


register.inclusion_tag("included.html", name="curried_inclusion")(curried_inclusion)


@register.inclusion_tag("included.html", name="")
def empty_named_inclusion(value):
    return {"value": value}


def ignored_inclusion(value):
    return {"ignored": value}


@register.inclusion_tag(
    "included.html",
    func=ignored_inclusion,
    name="inclusion_func_is_ignored",
)
def inclusion_func_is_ignored(value):
    return {"value": value}


@register.simple_block_tag(name="")
def empty_named_panel(content, title):
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


def optional_pop(parser, token):
    bits = token.split_contents()
    if bits[-1] == "tail":
        bits.pop()
    if bits[0] == "optional_pop":
        bits.pop(0)
    if bits:
        raise template.TemplateSyntaxError("'optional_pop' accepts only a tail marker")
    return template.Node()


register.tag("optional_pop", optional_pop)


def truthiness_guard(parser, token):
    bits = token.split_contents()[1:]
    if not bits:
        raise template.TemplateSyntaxError(
            "'truthiness_guard' requires at least one argument"
        )
    return template.Node()


register.tag("truthiness_guard", truthiness_guard)


def early_return_guard(parser, token):
    bits = token.split_contents()
    if len(bits) != 2:
        raise template.TemplateSyntaxError("'early_return_guard' requires one argument")
    if bits[1] == "early":
        return template.Node()
    return template.Node()


register.tag("early_return_guard", early_return_guard)


def common_branch_guard(parser, token):
    bits = token.split_contents()
    if token.lineno:
        if len(bits) != 2:
            raise template.TemplateSyntaxError("'common_branch_guard' requires one argument")
    else:
        if len(bits) != 2:
            raise template.TemplateSyntaxError("'common_branch_guard' requires one argument")
    return template.Node()


register.tag("common_branch_guard", common_branch_guard)


def finally_guard(parser, token):
    bits = token.split_contents()
    try:
        return template.Node()
    finally:
        if len(bits) != 2:
            raise template.TemplateSyntaxError("'finally_guard' requires one argument")


register.tag("finally_guard", finally_guard)


def pre_try_finally(parser, token):
    bits = token.split_contents()
    if len(bits) == 1:
        return template.Node()
    try:
        pass
    finally:
        if len(bits) != 2:
            raise template.TemplateSyntaxError("'pre_try_finally' requires zero or one argument")
    return template.Node()


register.tag("pre_try_finally", pre_try_finally)


def finally_suppresses_raise(parser, token):
    bits = token.split_contents()
    parser.parse(("endfinally_suppresses_raise",))
    parser.delete_first_token()
    try:
        if len(bits) != 2:
            raise template.TemplateSyntaxError("wrong count")
    finally:
        return template.Node()


register.tag("finally_suppresses_raise", finally_suppresses_raise)


def terminal_forms(parser, token):
    bits = token.split_contents()
    if len(bits) == 2:
        return template.Node()
    else:
        raise template.TemplateSyntaxError("'terminal_forms' requires one argument")
    if len(bits) != 3:
        raise template.TemplateSyntaxError("unreachable")


register.tag("terminal_forms", terminal_forms)


def authored_loop(parser, token):
    bits = token.split_contents()
    if len(bits) < 4:
        raise template.TemplateSyntaxError("authored_loop needs four words")
    has_reversed_tail = bits[-1] == "reversed"
    separator = -3 if has_reversed_tail else -2
    if bits[separator] != "in":
        raise template.TemplateSyntaxError("authored_loop expects 'variables in sequence'")
    parser.parse(("endauthored_loop",))
    parser.delete_first_token()
    return template.Node()


register.tag("authored_loop", authored_loop)


def conjunction_guard(parser, token):
    bits = token.split_contents()
    if len(bits) > 3 and bits[2] != "as":
        raise template.TemplateSyntaxError("bad syntax")
    parser.parse(("endconjunction_guard",))
    parser.delete_first_token()
    return template.Node()


register.tag("conjunction_guard", conjunction_guard)


def fail_between_pops():
    raise RuntimeError("controlled failure")


def exception_between_pops(parser, token):
    bits = token.split_contents()
    try:
        bits.pop()
        fail_between_pops()
        bits.pop()
    except RuntimeError:
        pass
    if len(bits) != 1:
        raise template.TemplateSyntaxError("'exception_between_pops' requires one argument")
    parser.parse(("endexception_between_pops",))
    parser.delete_first_token()
    return template.Node()


register.tag("exception_between_pops", exception_between_pops)


def restored_exception_state(parser, token):
    bits = token.split_contents()
    try:
        bits.pop()
        fail_between_pops()
        bits = token.split_contents()
    except RuntimeError:
        pass
    if len(bits) != 1:
        raise template.TemplateSyntaxError(
            "'restored_exception_state' expected the failure after one pop"
        )
    parser.parse(("endrestored_exception_state",))
    parser.delete_first_token()
    return template.Node()


register.tag("restored_exception_state", restored_exception_state)


def unhandled_exception_finally(parser, token):
    bits = token.split_contents()
    try:
        bits.pop()
        fail_between_pops()
        bits = token.split_contents()
    finally:
        if len(bits) != 1:
            raise template.TemplateSyntaxError(
                "'unhandled_exception_finally' expected the failure after one pop"
            )
        parser.parse(("endunhandled_exception_finally",))
        parser.delete_first_token()
        return template.Node()


register.tag("unhandled_exception_finally", unhandled_exception_finally)


@register.tag
def stylesheet(parser, token):
    try:
        tag_name, name = token.split_contents()
    except ValueError:
        message = "%r requires exactly one argument"
        raise template.TemplateSyntaxError(message % token.split_contents()[0])
    return template.Node()


@register.tag
def javascript(parser, token):
    try:
        tag_name, name = token.split_contents()
    except ValueError:
        message = "%r requires exactly one argument"
        raise template.TemplateSyntaxError(message % token.split_contents()[0])
    return template.Node()


@register.tag
def mutated_choice(parser, token):
    bits = token.split_contents()
    choices = ["old"]
    choices.append("new")
    if len(bits) != 2 or bits[1] not in choices:
        raise template.TemplateSyntaxError("mutated_choice argument is invalid")
    return template.Node()


OUTPUT_FILE = "file"
OUTPUT_INLINE = "inline"
OUTPUT_PRELOAD = "preload"
OUTPUT_MODES = (OUTPUT_FILE, OUTPUT_INLINE, OUTPUT_PRELOAD)


@register.tag
def compress(parser, token):
    parser.parse(("endcompress",))
    parser.delete_first_token()
    args = token.split_contents()
    if not len(args) in (2, 3, 4):
        raise template.TemplateSyntaxError("compress expects one to three arguments")
    if len(args) >= 3:
        if args[2] not in OUTPUT_MODES:
            raise template.TemplateSyntaxError("compress mode is invalid")
    return template.Node()


class DjangoTemplateTagNode(template.Node):
    mapping = {
        "openblock": "{%",
        "closeblock": "%}",
        "openvariable": "{{",
        "closevariable": "}}",
        "openbrace": "{",
        "closebrace": "}",
        "opencomment": "{#",
        "closecomment": "#}",
    }


@register.tag
def templatetag(parser, token):
    bits = token.contents.split()
    if len(bits) != 2:
        raise template.TemplateSyntaxError("templatetag expects one argument")
    tag = bits[1]
    if tag not in DjangoTemplateTagNode.mapping:
        raise template.TemplateSyntaxError("templatetag argument is invalid")
    return DjangoTemplateTagNode()
