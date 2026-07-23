# Vendored unit-test fixture.
# Corpus: django-6.1/tests/template_tests/templatetags/custom.py
# Keep snippets minimal: live corpus drift is covered by crates/djls-project/tests/corpus*.rs.

from django import template

register = template.Library()

@register.simple_block_tag
def div(content, id="test"):
    return format_html("<div id='{}'>{}</div>", id, content)

@register.simple_tag
def no_params():
    """Expected no_params __doc__"""
    return "no_params - Expected result"

@register.simple_tag(takes_context=True)
def no_params_with_context(context):
    """Expected no_params_with_context __doc__"""
    return (
        "no_params_with_context - Expected result (context value: %s)"
        % context["value"]
    )

@register.simple_tag
def one_param(arg):
    """Expected one_param __doc__"""
    return "one_param - Expected result: %s" % arg

@register.simple_tag
def simple_one_default(one, two="hi"):
    """Expected simple_one_default __doc__"""
    return "simple_one_default - Expected result: %s, %s" % (one, two)

@register.simple_tag
def simple_two_params(one, two):
    """Expected simple_two_params __doc__"""
    return "simple_two_params - Expected result: %s, %s" % (one, two)
