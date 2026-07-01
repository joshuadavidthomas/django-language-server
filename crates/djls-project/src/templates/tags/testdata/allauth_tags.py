# Vendored unit-test fixture.
# Corpus: django-allauth/allauth/templatetags/allauth.py
# Keep snippets minimal: live corpus drift is covered by crates/djls-project/tests/corpus*.rs.

from django import template

register = template.Library()

def parse_tag(token, parser):
    bits = token.split_contents()
    tag_name = bits.pop(0)
    args = []
    kwargs = {}
    for bit in bits:
        # Is this a kwarg or an arg?
        match = kwarg_re.match(bit)
        kwarg_format = match and match.group(1)
        if kwarg_format:
            key, value = match.groups()
            kwargs[key] = FilterExpression(value, parser)
        else:
            args.append(FilterExpression(bit, parser))

    return (tag_name, args, kwargs)

@register.tag(name="element")
def do_element(parser, token):
    nodelist = parser.parse(("endelement",))
    tag_name, args, kwargs = parse_tag(token, parser)
    usage = f'{{% {tag_name} "element" argument=value %}} ... {{% end{tag_name} %}}'
    if len(args) > 1:
        raise template.TemplateSyntaxError(f"Usage: {usage}")

    parser.delete_first_token()
    return ElementNode(nodelist, args[0], kwargs)
