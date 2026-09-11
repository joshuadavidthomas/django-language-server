from django import template

register = template.Library()


def other_tag(context):
    pass


register.simple_tag(takes_context=True)(globals()["other_tag"])
