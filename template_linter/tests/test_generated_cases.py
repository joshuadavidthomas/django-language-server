from __future__ import annotations

import json
import warnings
from pathlib import Path

from template_linter.template_syntax.parsing import parse_template_tags
from template_linter.validation.filters import validate_filters
from template_linter.validation.template import validate_template

from ._coverage_helpers import generate_filter_template
from ._coverage_helpers import generate_tag_template
from .test_linter import TEST_CASES


def test_generated_tag_cases(
    rules, filters, opaque_blocks, structural_rules, block_specs
):
    manual_tags = set()
    for template, _ok, _desc in TEST_CASES:
        for tag in parse_template_tags(template, opaque_blocks=opaque_blocks):
            manual_tags.add(tag.name)

    generated = {}
    failed = {}
    skipped = {}

    for tag_name, validation in rules.items():
        if tag_name in manual_tags:
            continue
        generated_case = generate_tag_template(tag_name, validation, opaque_blocks)
        if not generated_case:
            skipped[tag_name] = "no_generator"
            continue
        template, kind = generated_case
        errors = validate_template(
            template,
            rules,
            filters,
            opaque_blocks,
            structural_rules=structural_rules,
            block_specs=block_specs,
        )
        if errors:
            failed[tag_name] = {
                "kind": kind,
                "template": template,
                "error": errors[0].message,
            }
        else:
            generated[tag_name] = {"kind": kind, "template": template}

    report_path = (
        Path(__file__).resolve().parents[1] / "reports" / "generated_tag_cases.json"
    )
    report_path.parent.mkdir(parents=True, exist_ok=True)
    report_path.write_text(
        json.dumps(
            {
                "generated": generated,
                "failed": failed,
                "skipped": skipped,
            },
            indent=2,
            sort_keys=True,
        )
    )

    if failed:
        warnings.warn(
            f"Generated tag cases failing validation: {sorted(failed.keys())}",
            stacklevel=2,
        )


def test_generated_filter_cases(filters, opaque_blocks):
    generated = {}
    failed = {}
    skipped = {}

    for name, spec in filters.items():
        generated_case = generate_filter_template(name, spec)
        if not generated_case:
            skipped[name] = "requires_multiple_args"
            continue
        template, kind = generated_case
        errors = validate_filters(template, filters, opaque_blocks=opaque_blocks)
        if errors:
            failed[name] = {
                "kind": kind,
                "template": template,
                "error": errors[0].message,
            }
        else:
            generated[name] = {"kind": kind, "template": template}

    report_path = (
        Path(__file__).resolve().parents[1] / "reports" / "generated_filter_cases.json"
    )
    report_path.parent.mkdir(parents=True, exist_ok=True)
    report_path.write_text(
        json.dumps(
            {
                "generated": generated,
                "failed": failed,
                "skipped": skipped,
            },
            indent=2,
            sort_keys=True,
        )
    )

    if failed:
        warnings.warn(
            f"Generated filter cases failing validation: {sorted(failed.keys())}",
            stacklevel=2,
        )
