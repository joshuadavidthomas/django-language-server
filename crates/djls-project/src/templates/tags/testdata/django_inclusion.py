# Vendored unit-test fixture.
# Corpus: django-6.0/tests/template_tests/templatetags/inclusion.py
# Keep snippets minimal: live corpus drift is covered by crates/djls-project/tests/corpus*.rs.

from django import template

register = template.Library()

@register.inclusion_tag("inclusion.html")
def inclusion_no_params():
    """Expected inclusion_no_params __doc__"""
    return {"result": "inclusion_no_params - Expected result"}

@register.inclusion_tag("inclusion.html", takes_context=True)
def inclusion_no_params_with_context(context):
    """Expected inclusion_no_params_with_context __doc__"""
    return {
        "result": (
            "inclusion_no_params_with_context - Expected result (context value: %s)"
        )
        % context["value"]
    }

@register.inclusion_tag("inclusion.html")
def inclusion_one_default(one, two="hi"):
    """Expected inclusion_one_default __doc__"""
    return {"result": "inclusion_one_default - Expected result: %s, %s" % (one, two)}

@register.inclusion_tag("inclusion.html")
def inclusion_one_param(arg):
    """Expected inclusion_one_param __doc__"""
    return {"result": "inclusion_one_param - Expected result: %s" % arg}
