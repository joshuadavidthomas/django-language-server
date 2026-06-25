# Vendored unit-test fixture.
# Corpus: django-6.0/tests/template_tests/templatetags/testtags.py
# Keep snippets minimal: live corpus drift is covered by crates/djls-project/tests/corpus*.rs.

from django import template

register = template.Library()

@register.tag
def echo(parser, token):
    return EchoNode(token.contents.split()[1:])

register.tag("other_echo", echo)
