from django import template

register = template.Library()


@register.simple_tag
def post_title():
    return 'Post'
