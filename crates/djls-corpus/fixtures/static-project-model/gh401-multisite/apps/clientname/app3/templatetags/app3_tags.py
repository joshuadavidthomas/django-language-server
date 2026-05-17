from django import template

register = template.Library()


@register.simple_tag
def app3_name():
    return "app3"
