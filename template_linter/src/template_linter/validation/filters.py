"""
Filter validation (static).

Validates:
- known filter argument counts
- unknown filters (optional)

This module is separate from tag validation to reduce coupling and improve
portability.
"""

from __future__ import annotations

from ..extraction.filters import FilterSpec
from ..template_syntax.filter_syntax import _extract_filter_exprs_from_token
from ..template_syntax.filter_syntax import _parse_filter_chain
from ..template_syntax.parsing import parse_template_tags
from ..template_syntax.parsing import parse_template_vars
from ..types import OpaqueBlockSpec
from ..types import TemplateTag
from ..types import ValidationError
from ..types import simple_error


def _validate_filter_chain(
    expr: str,
    line: int,
    filters: dict[str, FilterSpec],
) -> list[ValidationError]:
    errors: list[ValidationError] = []
    for name, has_arg in _parse_filter_chain(expr):
        spec = filters.get(name)
        if not spec:
            continue
        if spec.unrestricted:
            continue
        provided = 1 if has_arg else 0
        plen = provided + 1
        alen = spec.pos_args
        dlen = spec.defaults
        if plen < (alen - dlen) or plen > alen:
            message = f"{name} requires {alen - dlen} arguments, {plen} provided"
            tag = TemplateTag(
                name=f"filter:{name}",
                tokens=[name],
                raw=expr,
                line=line,
            )
            errors.append(simple_error(tag, message))
    return errors


def validate_filters(
    template: str,
    filters: dict[str, FilterSpec],
    opaque_blocks: dict[str, OpaqueBlockSpec] | None = None,
    report_unknown: bool = False,
) -> list[ValidationError]:
    errors: list[ValidationError] = []
    for contents, line in parse_template_vars(template, opaque_blocks=opaque_blocks):
        errors.extend(_validate_filter_chain(contents, line, filters))
        if report_unknown:
            for name, _has_arg in _parse_filter_chain(contents):
                if name not in filters:
                    tag = TemplateTag(
                        name=f"filter:{name}",
                        tokens=[name],
                        raw=contents,
                        line=line,
                    )
                    errors.append(simple_error(tag, f"Unknown filter '{name}'"))

    # Validate filters used in block tag arguments (e.g., {% if x|add %})
    for tag in parse_template_tags(template, opaque_blocks=opaque_blocks):
        for token in tag.tokens[1:]:
            for expr in _extract_filter_exprs_from_token(token):
                errors.extend(_validate_filter_chain(expr, tag.line, filters))
                if report_unknown:
                    for name, _has_arg in _parse_filter_chain(expr):
                        if name not in filters:
                            unknown_tag = TemplateTag(
                                name=f"filter:{name}",
                                tokens=[name],
                                raw=expr,
                                line=tag.line,
                            )
                            errors.append(
                                simple_error(unknown_tag, f"Unknown filter '{name}'")
                            )
    return errors
