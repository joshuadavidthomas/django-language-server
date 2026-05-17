from django import template

register = template.Library()


@register.simple_tag
def app1_name():
    return "app1"
