
from django import template

register = template.Library()

@register.tag("mystery")
def do_mystery(parser, token):
    tag_name, *rest = token.split_contents()
    nodelist = parser.parse((f"end{tag_name}",))
    return MysteryNode(nodelist)
