
from django import template

register = template.Library()

@register.tag("mystery")
def do_mystery(parser, token):
    options = {"name": "mystery"}
    nodelist = parser.parse((f"end{options['name']}",))
    return MysteryNode(nodelist)
