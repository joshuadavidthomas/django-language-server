from __future__ import annotations

from django import template

register = template.Library()


@register.tag("blocky")
def blocky(parser, token):
    # Should produce a block spec: start=blocky, middle=else, end=endblocky
    parser.parse(("else", "endblocky"))
    parser.delete_first_token()
    return template.Node()


@register.tag("dynblock")
def dynblock(parser, token):
    # Dynamic end tag spec: f"end{tag_name}"
    tag_name = token.split_contents()[0]
    parser.parse((f"end{tag_name}",))
    parser.delete_first_token()
    return template.Node()
