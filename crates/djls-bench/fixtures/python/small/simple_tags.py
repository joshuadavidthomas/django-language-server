from django import template

register = template.Library()


@register.simple_tag
def current_time(format_string):
    return format_string


@register.filter
def lower(value):
    return value.lower()


@register.filter(needs_autoescape=True)
def initial_letter_filter(text, autoescape=True):
    return text
