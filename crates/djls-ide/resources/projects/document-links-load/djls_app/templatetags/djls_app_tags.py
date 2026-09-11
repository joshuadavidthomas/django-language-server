from django import template

register = template.Library()


@register.simple_tag
def djls_greeting():
    pass
