from __future__ import annotations

from template_linter.template_syntax.parsing import parse_template_tags
from template_linter.types import BlockTagSpec
from template_linter.validation.structural import _validate_block_structure


def test_implicit_block_prevents_else_misattribution() -> None:
    # `ifequal`/`endifequal` is a legacy Django block tag that appears in third-party
    # templates. When it's not present in extracted Django block specs, we still
    # need structural validation to avoid treating its `{% else %}` as belonging
    # to an outer `{% if %}` block.
    template = (
        "{% if x %}"
        "{% ifequal a b %}a{% else %}b{% endifequal %}"
        "{% else %}c{% endif %}"
    )
    tags = parse_template_tags(template)
    specs = [
        BlockTagSpec(
            start_tags=("if",),
            end_tags=("endif",),
            middle_tags=("else",),
            repeatable_middle_tags=(),
            terminal_middle_tags=("else",),
            end_suffix_from_start_index=None,
        )
    ]
    errors = _validate_block_structure(tags, specs)
    assert errors == []
