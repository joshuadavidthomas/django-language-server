"""
Template validation using extracted rules.

Key improvement over spikes 05-06: Properly handles compound OR rules
by inverting them to AND for validity checks.
"""

from __future__ import annotations

from ..extraction.filters import FilterSpec
from ..resolution.load import LibraryIndex
from ..resolution.load import resolve_load_tokens
from ..template_syntax.parsing import parse_template_tags  # noqa: F401
from ..template_syntax.parsing import parse_template_vars  # noqa: F401
from ..template_syntax.tokenization import TemplateToken  # noqa: F401
from ..template_syntax.tokenization import tokenize_template  # noqa: F401
from ..types import BlockTagSpec
from ..types import ConditionalInnerTagRule
from ..types import OpaqueBlockSpec
from ..types import TagValidation
from ..types import ValidationError
from ..types import simple_error as _simple_error
from .filters import validate_filters
from .if_expression import validate_if_expression_tag
from .structural import _opaque_end_tag_names
from .structural import _structure_tag_names
from .structural import _validate_block_structure
from .structural import _validate_structural_rules
from .tags import validate_tag

# NOTE: Rule evaluation and single-tag validation live in `rule_eval.py` and
# `tags.py`. This module focuses on template-wide orchestration.


def validate_template(
    template: str,
    rules: dict[str, TagValidation],
    filters: dict[str, FilterSpec] | None = None,
    opaque_blocks: dict[str, OpaqueBlockSpec] | None = None,
    report_unknown_tags: bool = False,
    report_unknown_filters: bool = False,
    tag_aliases: dict[str, str] | None = None,
    structural_rules: list[ConditionalInnerTagRule] | None = None,
    block_specs: list[BlockTagSpec] | None = None,
) -> list[ValidationError]:
    """
    Validate all tags in a template against extracted rules.

    Args:
        template: Template string to validate
        rules: Dict mapping tag names to their validation rules
        filters: Optional dict mapping filter names to FilterSpec

    Returns:
        List of validation errors found
    """
    tags = parse_template_tags(template, opaque_blocks=opaque_blocks)
    all_errors = []

    aliases = tag_aliases or {}
    structure_tags = _structure_tag_names(
        structural_rules=structural_rules,
        block_specs=block_specs,
    )
    opaque_end_tags = _opaque_end_tag_names(opaque_blocks)

    for tag in tags:
        all_errors.extend(validate_if_expression_tag(tag))

        rule_name = tag.name
        if rule_name not in rules:
            rule_name = aliases.get(rule_name, rule_name)
        if rule_name in rules:
            errors = validate_tag(tag, rules[rule_name])
            all_errors.extend(errors)
        elif (
            report_unknown_tags
            and tag.name not in structure_tags
            and tag.name not in opaque_end_tags
        ):
            all_errors.append(_simple_error(tag, f"Unknown tag '{tag.name}'"))

    if filters is not None:
        all_errors.extend(
            validate_filters(
                template,
                filters,
                opaque_blocks=opaque_blocks,
                report_unknown=report_unknown_filters,
            )
        )

    all_errors.extend(
        _validate_structural_rules(template, opaque_blocks, structural_rules)
    )

    all_errors.extend(_validate_block_structure(tags, block_specs))

    return all_errors


def validate_template_with_load_resolution(
    template: str,
    base_rules: dict[str, TagValidation],
    *,
    base_filters: dict[str, FilterSpec] | None = None,
    opaque_blocks: dict[str, OpaqueBlockSpec] | None = None,
    django_index: LibraryIndex | None = None,
    entry_index: LibraryIndex | None = None,
    report_unknown_tags: bool = False,
    report_unknown_filters: bool = False,
    tag_aliases: dict[str, str] | None = None,
    structural_rules: list[ConditionalInnerTagRule] | None = None,
    block_specs: list[BlockTagSpec] | None = None,
    report_unknown_libraries: bool = False,
) -> list[ValidationError]:
    """
    Validate a template while statically applying `{% load %}` directives.

    This updates the available tag/filter set as `load` tags are encountered,
    mirroring Django's "later loads override earlier" behavior.

    Notes:
    - Tag scoping is position-aware (only affects subsequent tags).
    - Filter validation uses the *final* filter set after all `{% load %}` tags,
      since variable tokenization doesn't preserve strict ordering here.
    """
    tags = parse_template_tags(template, opaque_blocks=opaque_blocks)
    all_errors: list[ValidationError] = []

    aliases = tag_aliases or {}
    opaque_end_tags = _opaque_end_tag_names(opaque_blocks)

    current_rules = dict(base_rules)
    current_filters = dict(base_filters) if base_filters is not None else None
    current_structural_rules = list(structural_rules or [])
    current_block_specs = list(block_specs or [])

    for tag in tags:
        if tag.name == "load":
            resolved = resolve_load_tokens(
                tag.tokens,
                django_index=django_index,
                entry_index=entry_index,
            )
            if report_unknown_libraries:
                for err in resolved.errors:
                    all_errors.append(_simple_error(tag, err.message))
            # Later loads override earlier ones (Django merges dicts with update()).
            current_rules.update(resolved.bundle.rules)
            if current_filters is not None:
                current_filters.update(resolved.bundle.filters)
            # Structural specs/rules also come from loaded libraries (e.g. block end tags
            # like `endcompress` from django-compressor).
            current_structural_rules.extend(resolved.bundle.structural_rules)
            current_block_specs.extend(resolved.bundle.block_specs)
            continue

        all_errors.extend(validate_if_expression_tag(tag))

        structure_tags = _structure_tag_names(
            structural_rules=current_structural_rules,
            block_specs=current_block_specs,
        )

        rule_name = tag.name
        if rule_name not in current_rules:
            rule_name = aliases.get(rule_name, rule_name)

        if rule_name in current_rules:
            all_errors.extend(validate_tag(tag, current_rules[rule_name]))
        elif (
            report_unknown_tags
            and tag.name not in structure_tags
            and tag.name not in opaque_end_tags
        ):
            all_errors.append(_simple_error(tag, f"Unknown tag '{tag.name}'"))

    if current_filters is not None:
        all_errors.extend(
            validate_filters(
                template,
                current_filters,
                opaque_blocks=opaque_blocks,
                report_unknown=report_unknown_filters,
            )
        )

    all_errors.extend(
        _validate_structural_rules(template, opaque_blocks, current_structural_rules)
    )

    all_errors.extend(_validate_block_structure(tags, current_block_specs))

    return all_errors
