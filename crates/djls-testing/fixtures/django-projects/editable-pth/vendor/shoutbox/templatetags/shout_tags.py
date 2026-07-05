from django import template

register = template.Library()


@register.simple_tag
def shout():
    return 'shout'
