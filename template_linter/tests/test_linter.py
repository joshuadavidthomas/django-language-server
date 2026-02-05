from __future__ import annotations

import pytest

from template_linter.template_syntax.parsing import parse_template_tags
from template_linter.template_syntax.parsing import parse_template_vars
from template_linter.validation.template import validate_template

TEST_CASES = [
    # Basic count checks
    ("{% autoescape on %}test{% endautoescape %}", True, "autoescape valid"),
    ("{% autoescape off %}test{% endautoescape %}", True, "autoescape valid off"),
    ("{% autoescape %}test{% endautoescape %}", False, "autoescape missing arg"),
    (
        "{% autoescape maybe %}test{% endautoescape %}",
        False,
        "autoescape invalid value",
    ),
    # Compound OR rules
    ("{% get_current_language as lang %}", True, "get_current_language valid"),
    ("{% get_current_language %}", False, "get_current_language missing 'as var'"),
    (
        "{% get_current_language into lang %}",
        False,
        "get_current_language wrong keyword",
    ),
    ("{% get_current_language as %}", False, "get_current_language missing var"),
    ("{% get_available_languages as langs %}", True, "get_available_languages valid"),
    ("{% get_available_languages %}", False, "get_available_languages missing args"),
    # cycle - various forms
    ("{% cycle 'a' 'b' 'c' %}", True, "cycle basic"),
    ("{% cycle %}", False, "cycle no args"),
    ("{% cycle 'a' 'b' as name %}", True, "cycle with as"),
    ("{% cycle 'a' 'b' 'c' as name silent %}", True, "cycle with silent"),
    # for tag - dynamic index pattern
    ("{% for x in items %}{% endfor %}", True, "for basic"),
    ("{% for x, y in items %}{% endfor %}", True, "for tuple unpack"),
    ("{% for x in items reversed %}{% endfor %}", True, "for reversed"),
    ("{% for x from items %}{% endfor %}", False, "for wrong keyword"),
    (
        "{% for x from items reversed %}{% endfor %}",
        False,
        "for wrong keyword reversed",
    ),
    # Block tags
    ("{% block content %}{% endblock %}", True, "block basic"),
    ("{% block %}{% endblock %}", False, "block missing name"),
    # firstof - boolean check pattern
    ("{% firstof var1 var2 %}", True, "firstof basic"),
    ("{% firstof var1 var2 var3 %}", True, "firstof multiple"),
    ("{% firstof %}", False, "firstof no args"),
    ("{% firstof var1 var2 as result %}", True, "firstof with as"),
    # with - token_kwargs pattern
    ("{% with foo=bar %}{% endwith %}", True, "with modern syntax"),
    ("{% with foo=bar baz=qux %}{% endwith %}", True, "with multiple kwargs"),
    ("{% with value as name %}{% endwith %}", True, "with legacy syntax"),
    ("{% with %}{% endwith %}", False, "with no args"),
    # widthratio - precondition inference
    ("{% widthratio val max width %}", True, "widthratio 3 args"),
    ("{% widthratio val max width as name %}", True, "widthratio with as"),
    ("{% widthratio %}", False, "widthratio no args"),
    # now - conditional slice of bits for 'as'
    ('{% now "Y" %}', True, "now basic"),
    ('{% now "Y" as current_year %}', True, "now with as"),
    ("{% now %}", False, "now missing format"),
    # url - 'as' suffix slices bits
    ('{% url "home" %}', True, "url basic"),
    ('{% url "home" as home_url %}', True, "url with as"),
    ("{% url %}", False, "url missing viewname"),
    # lorem - reverse parsing via pops
    ("{% lorem 3 p random extra %}", False, "lorem too many args"),
    # include option loop
    ('{% include "x.html" with foo=1 %}', True, "include with kwargs"),
    ('{% include "x.html" with %}', False, "include with missing kwargs"),
    ('{% include "x.html" with foo %}', False, "include with non-kwarg"),
    ('{% include "x.html" only %}', True, "include only"),
    ('{% include "x.html" badopt %}', False, "include unknown option"),
    # blocktranslate option loop
    (
        "{% blocktranslate trimmed %}x{% endblocktranslate %}",
        True,
        "blocktranslate trimmed",
    ),
    (
        "{% blocktranslate count %}x{% endblocktranslate %}",
        False,
        "blocktranslate count missing",
    ),
    (
        "{% blocktranslate count a=1 b=2 %}x{% endblocktranslate %}",
        False,
        "blocktranslate count too many",
    ),
    (
        "{% blocktranslate count a=1 %}x{% endblocktranslate %}",
        False,
        "blocktranslate count missing plural",
    ),
    (
        "{% blocktranslate count a=1 %}x{% plural %}y{% endblocktranslate %}",
        True,
        "blocktranslate count with plural",
    ),
    (
        "{% blocktranslate %}x{% plural %}y{% endblocktranslate %}",
        False,
        "blocktranslate plural without count",
    ),
    (
        '{% blocktranslate context "hint" %}x{% endblocktranslate %}',
        True,
        "blocktranslate context",
    ),
    (
        "{% blocktranslate asvar result %}x{% endblocktranslate %}",
        True,
        "blocktranslate asvar",
    ),
    (
        "{% blocktranslate asvar %}x{% endblocktranslate %}",
        False,
        "blocktranslate asvar missing",
    ),
    (
        "{% blocktranslate weirdopt %}x{% endblocktranslate %}",
        False,
        "blocktranslate unknown option",
    ),
    (
        "{% blocktranslate with view.full_name as full_name and view.url_name as url_name %}x{% endblocktranslate %}",
        True,
        "blocktranslate legacy with and",
    ),
    (
        "{% blocktranslate with %}x{% endblocktranslate %}",
        False,
        "blocktranslate with missing",
    ),
    (
        "{% blocktranslate with foo|upper as bar %}x{% endblocktranslate %}",
        True,
        "blocktranslate legacy with",
    ),
    (
        "{% blocktranslate context %}x{% endblocktranslate %}",
        False,
        "blocktranslate context missing",
    ),
    (
        "{% blocktranslate trimmed trimmed %}x{% endblocktranslate %}",
        False,
        "blocktranslate duplicate option",
    ),
    # Edge cases for legacy kwargs parsing
    (
        "{% blocktranslate with a as b and c %}x{% endblocktranslate %}",
        False,
        "blocktranslate legacy incomplete after and",
    ),
    (
        "{% blocktranslate with a as b extra %}x{% endblocktranslate %}",
        False,
        "blocktranslate legacy trailing token",
    ),
    (
        "{% blocktranslate with foo %}x{% endblocktranslate %}",
        False,
        "blocktranslate legacy no as",
    ),
    # get_flatpages (contrib.flatpages)
    ("{% get_flatpages %}", False, "get_flatpages missing args"),
    ("{% get_flatpages 5 as pages %}", True, "get_flatpages basic"),
    ("{% get_flatpages 5 as %}", False, "get_flatpages missing var"),
    # get_admin_log (contrib.admin)
    ("{% get_admin_log %}", False, "get_admin_log missing args"),
    ("{% get_admin_log 5 as log %}", True, "get_admin_log basic"),
    ("{% get_admin_log 5 to log %}", False, "get_admin_log wrong keyword"),
    ("{% get_admin_log x as log %}", False, "get_admin_log non-digit"),
    # add_preserved_filters (simple_tag via parse_bits)
    ("{% add_preserved_filters %}", False, "add_preserved_filters missing url"),
    ("{% add_preserved_filters url popup=True %}", True, "add_preserved_filters popup"),
    (
        '{% add_preserved_filters url to_field="pk" %}',
        True,
        "add_preserved_filters to_field",
    ),
    (
        "{% add_preserved_filters url badkw=1 %}",
        False,
        "add_preserved_filters unexpected kw",
    ),
    # partial / partialdef (match/case)
    ("{% partial %}", False, "partial missing name"),
    ("{% partial name extra %}", False, "partial too many args"),
    ("{% partial name %}", True, "partial ok"),
    ("{% partialdef %}{% endpartialdef %}", False, "partialdef missing name"),
    ("{% partialdef name inline %}{% endpartialdef %}", True, "partialdef inline ok"),
    ("{% partialdef name nope %}{% endpartialdef %}", False, "partialdef invalid arg"),
    (
        "{% partialdef name inline extra %}{% endpartialdef %}",
        False,
        "partialdef too many args",
    ),
    # filters - args_check and parsing
    ('{{ value|add:"1" }}', True, "filter add ok"),
    ("{{ value|add }}", False, "filter add missing arg"),
    ("{{ value|floatformat }}", True, "filter floatformat default arg"),
    ('{{ value|floatformat:"2" }}', True, "filter floatformat with arg"),
    ("{{ value|length }}", True, "filter length no arg"),
    ('{{ value|length:"x" }}', False, "filter length unexpected arg"),
    ('{{ value|add:"1"|length }}', True, "filter chain ok"),
    ('{{ value|default:"a|b" }}', True, "filter arg with pipe"),
    # filters in block tags
    ("{% if value|add %}x{% endif %}", False, "filter in if missing arg"),
    ('{% if value|add:"1" %}x{% endif %}', True, "filter in if ok"),
    ("{% include template|default %}", False, "filter in include missing arg"),
    # Additional tag coverage (basic valid forms)
    ("{% cache 300 frag %}x{% endcache %}", True, "cache basic"),
    ('{% extends "base.html" %}', True, "extends basic"),
    ("{% filter upper %}x{% endfilter %}", True, "filter tag basic"),
    ("{% get_current_language_bidi as bidi %}", True, "get_current_language_bidi"),
    ("{% get_current_timezone as tz %}", True, "get_current_timezone"),
    ('{% get_language_info for "en" as lang %}', True, "get_language_info"),
    (
        "{% get_language_info_list for LANGUAGES as langs %}",
        True,
        "get_language_info_list",
    ),
    ("{% get_media_prefix %}", True, "get_media_prefix"),
    ("{% get_static_prefix %}", True, "get_static_prefix"),
    ("{% ifchanged %}x{% endifchanged %}", True, "ifchanged basic"),
    ("{% ifchanged a b %}x{% endifchanged %}", True, "ifchanged args"),
    ("{% ifchanged %}x{% else %}y{% endifchanged %}", True, "ifchanged else"),
    ('{% language "de" %}x{% endlanguage %}', True, "language block"),
    ("{% language %}x{% endlanguage %}", False, "language missing arg"),
    ('{% language "de" extra %}x{% endlanguage %}', False, "language extra arg"),
    ("{% load static %}", True, "load static"),
    ("{% load i18n %}", True, "load i18n"),
    ("{% load i18n static %}", True, "load multiple libraries"),
    ("{% load static from i18n %}", True, "load from syntax (syntax only)"),
    ("{% localize off %}x{% endlocalize %}", True, "localize block"),
    ("{% localtime off %}x{% endlocaltime %}", True, "localtime block"),
    ("{% pagination cl %}", True, "pagination"),
    ("{% prepopulated_fields_js %}", True, "prepopulated_fields_js"),
    ("{% regroup people by gender as grouped %}", True, "regroup"),
    ("{% resetcycle %}", True, "resetcycle"),
    ("{% result_list cl %}", True, "result_list"),
    ("{% search_form cl %}", True, "search_form"),
    ("{% spaceless %}x{% endspaceless %}", True, "spaceless"),
    ('{% static "x.css" %}', True, "static"),
    ("{% submit_row %}", True, "submit_row"),
    ("{% templatetag openblock %}", True, "templatetag"),
    ('{% timezone "UTC" %}x{% endtimezone %}', True, "timezone"),
    ('{% trans "hello" %}', True, "trans"),
    ('{% translate "hello" %}', True, "translate"),
    ('{% translate "hello" noop %}', True, "translate noop"),
    ('{% translate "hello" as greeting %}', True, "translate as"),
    ('{% translate "hello" context "greeting" %}', True, "translate context"),
    ('{% translate "hello" noop as greeting %}', True, "translate noop as"),
    ('{% translate "hello" context %}', False, "translate context missing"),
    ('{% translate "hello" as %}', False, "translate as missing"),
    ('{% translate "hello" context as %}', False, "translate context invalid as"),
    ('{% translate "hello" context noop %}', False, "translate context invalid noop"),
    ('{% translate "hello" bogus %}', False, "translate unknown option"),
    ('{% translate "hello" noop noop %}', False, "translate duplicate option"),
    ("{% blocktrans %}x{% endblocktrans %}", True, "blocktrans"),
    ("{% csrf_token %}", True, "csrf_token"),
    ("{% debug %}", True, "debug"),
    ("{% comment %}{{ value|add }}{% endcomment %}", True, "comment"),
    ("{% verbatim %}{{ value|add }}{% endverbatim %}", True, "verbatim"),
    ("{% admin_actions %}", True, "admin_actions"),
    ("{% change_form_object_tools %}", True, "change_form_object_tools"),
    ("{% change_list_object_tools %}", True, "change_list_object_tools"),
    ("{% date_hierarchy cl %}", True, "date_hierarchy"),
    ("{% result_list cl %}", True, "result_list duplicate coverage"),
    ("{% search_form cl %}", True, "search_form duplicate coverage"),
    ("{% pagination cl %}", True, "pagination duplicate coverage"),
    ('{% static "x.css" as css_url %}', True, "static as var"),
]


@pytest.mark.parametrize("template,should_pass,description", TEST_CASES)
def test_validation_cases(
    template: str,
    should_pass: bool,
    description: str,
    rules,
    filters,
    opaque_blocks,
    structural_rules,
    block_specs,
):
    errors = validate_template(
        template,
        rules,
        filters,
        opaque_blocks,
        structural_rules=structural_rules,
        block_specs=block_specs,
    )
    assert (len(errors) == 0) == should_pass, f"{description}: {errors}"


def test_fallback_parsers_skip_comment_verbatim(opaque_blocks):
    fallback_template = (
        "{% comment %}{% url %}{% endcomment %}"
        "{{ value|add }}"
        "{% verbatim %}{{ value|add }}{% endverbatim %}"
    )

    fb_tags = parse_template_tags(
        fallback_template, force_fallback=True, opaque_blocks=opaque_blocks
    )
    fb_names = [t.name for t in fb_tags]
    fb_vars = parse_template_vars(
        fallback_template, force_fallback=True, opaque_blocks=opaque_blocks
    )

    assert "url" not in fb_names
    assert "comment" in fb_names and "endcomment" in fb_names
    assert "verbatim" in fb_names and "endverbatim" in fb_names
    assert len(fb_vars) == 1
    assert fb_vars[0][0].strip().startswith("value|add")


def test_lexer_parser_skips_comment_blocks(opaque_blocks):
    template = "{% comment %}{{ value|add }}{% endcomment %}{{ value|add }}"
    vars_ = parse_template_vars(template, opaque_blocks=opaque_blocks)
    assert len(vars_) == 1
    assert vars_[0][0].strip().startswith("value|add")


def test_unknown_tag_filter_reporting(rules, filters, opaque_blocks, block_specs):
    template = "{% does_not_exist %}{{ value|no_such_filter }}"
    errors = validate_template(
        template,
        rules,
        filters,
        opaque_blocks=opaque_blocks,
        report_unknown_tags=True,
        report_unknown_filters=True,
        block_specs=block_specs,
    )
    messages = [e.message for e in errors]
    assert any("Unknown tag 'does_not_exist'" in m for m in messages)
    assert any("Unknown filter 'no_such_filter'" in m for m in messages)
