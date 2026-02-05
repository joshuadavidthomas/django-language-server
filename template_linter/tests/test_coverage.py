from __future__ import annotations

import pytest

from template_linter.validation.template import validate_template

KNOWN_TEST_CASES = [
    # (template, should_error, description)
    ("{% autoescape %}test{% endautoescape %}", True, "autoescape missing arg"),
    ("{% autoescape maybe %}test{% endautoescape %}", True, "autoescape invalid value"),
    ("{% cycle %}", True, "cycle no args"),
    ("{% for x from items %}{% endfor %}", True, "for wrong keyword"),
    ("{% block %}{% endblock %}", True, "block missing name"),
    ("{% get_current_language %}", True, "get_current_language missing args"),
    (
        "{% get_current_language into lang %}",
        True,
        "get_current_language wrong keyword",
    ),
    ("{% get_available_languages %}", True, "get_available_languages missing args"),
    ("{% lorem 5 z %}", True, "lorem invalid method"),
    ("{% with %}{% endwith %}", True, "with no args"),
    ("{% include %}", True, "include no args"),
    ("{% url %}", True, "url no args"),
    ("{% widthratio %}", True, "widthratio no args"),
    ("{% regroup %}", True, "regroup no args"),
    ("{% firstof %}", True, "firstof no args"),
    # Valid cases (should NOT error)
    ("{% autoescape on %}test{% endautoescape %}", False, "autoescape valid"),
    ("{% cycle 'a' 'b' %}", False, "cycle valid"),
    ("{% for x in items %}{% endfor %}", False, "for valid"),
    ("{% block content %}{% endblock %}", False, "block valid"),
    ("{% get_current_language as lang %}", False, "get_current_language valid"),
]


@pytest.mark.parametrize("template,should_error,description", KNOWN_TEST_CASES)
def test_known_cases(
    template: str,
    should_error: bool,
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
    assert (len(errors) > 0) == should_error, f"{description}: {errors}"
