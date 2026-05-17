from django import template

register = template.Library()


@register.simple_tag
def app2_name():
    return "app2"
