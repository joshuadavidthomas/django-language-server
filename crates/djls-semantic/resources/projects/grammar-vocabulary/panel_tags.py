from django import template
register = template.Library()
@register.tag(name='panel')
def panel(parser, token):
    nodelist = parser.parse(('elsepanel', 'endpanel'))
    parser.delete_first_token()
    return Node(nodelist)
