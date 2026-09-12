from django import template

register = template.Library()


@register.simple_tag
def known_tag():
    pass
