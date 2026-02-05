from __future__ import annotations

from django import template

register = template.Library()


@register.tag("opaque_block")
def opaque_block(parser, token):
    # This pattern should be detected as opaque: skip past a literal end tag.
    parser.skip_past("endopaque_block")
    return template.Node()


@register.tag("opaque_suffix")
def opaque_suffix(parser, token):
    # Opaque with a suffix should be detectable when `skip_past` uses a
    # constant string with a suffix component.
    parser.skip_past("endopaque_suffix foo")
    return template.Node()
