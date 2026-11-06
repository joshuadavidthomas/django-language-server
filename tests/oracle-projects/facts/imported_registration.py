from __future__ import annotations

from checks import register


@register.simple_tag
def imported_greeting():
    return "hello"
