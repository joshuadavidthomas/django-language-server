"""Subset of django.template.defaulttags for extraction testing.

Note: Uses 'args' instead of 'bits' in autoescape to verify dynamic variable detection.
"""
from django import template
from django.template.base import TemplateSyntaxError

register = template.Library()


@register.tag
def autoescape(parser, token):
    # Using 'args' instead of 'bits' to test variable detection
    args = token.split_contents()
    if len(args) != 2:
        raise TemplateSyntaxError("'autoescape' tag requires exactly one argument.")
    arg = args[1]
    if arg not in ("on", "off"):
        raise TemplateSyntaxError("'autoescape' argument should be 'on' or 'off'")
    nodelist = parser.parse(("endautoescape",))
    parser.delete_first_token()
    return AutoEscapeControlNode((arg == "on"), nodelist)


@register.tag("if")
def do_if(parser, token):
    nodelist = parser.parse(("elif", "else", "endif"))
    tok = parser.next_token()
    if tok.contents == "else":
        nodelist = parser.parse(("endif",))
    return IfNode(nodelist)


@register.tag("for")
def do_for(parser, token):
    parts = token.split_contents()  # Using 'parts' to test variable detection
    if len(parts) < 4:
        raise TemplateSyntaxError("'for' statements should have at least four words")
    if parts[2] != "in":
        raise TemplateSyntaxError("'for' statements should use 'for x in y'")

    nodelist_loop = parser.parse(("empty", "endfor"))
    token = parser.next_token()
    if token.contents == "empty":
        nodelist_empty = parser.parse(("endfor",))
    else:
        nodelist_empty = None
    return ForNode(nodelist_loop, nodelist_empty)


@register.simple_tag
def now(format_string):
    return datetime.now().strftime(format_string)


@register.filter
def title(value):
    return value.title()


@register.filter
def default(value, arg=""):
    return value or arg


@register.filter
def truncatewords(value, arg):
    words = value.split()
    return " ".join(words[:int(arg)])
