from __future__ import annotations

import json
import warnings
from pathlib import Path

from template_linter.template_syntax.parsing import parse_template_tags

from ._coverage_helpers import collect_filters_from_templates
from ._coverage_helpers import generate_filter_template
from ._coverage_helpers import generate_tag_template
from .test_inventory import _collect_registered_filters
from .test_inventory import _collect_registered_tags
from .test_linter import TEST_CASES


def test_case_coverage_inventory(
    django_root: Path, rules, filters, opaque_blocks, block_specs
):
    registered_tags = _collect_registered_tags(django_root)
    registered_filters = _collect_registered_filters(django_root)

    covered_tags: set[str] = set()
    covered_filters: set[str] = set()

    for template, _should_pass, _description in TEST_CASES:
        for tag in parse_template_tags(template, opaque_blocks=opaque_blocks):
            if tag.name in registered_tags:
                covered_tags.add(tag.name)
    covered_filters.update(
        collect_filters_from_templates(
            (tpl for tpl, _ok, _desc in TEST_CASES),
            opaque_blocks,
        )
    )

    generated_tag_templates = []
    for tag_name, validation in rules.items():
        generated = generate_tag_template(tag_name, validation, opaque_blocks)
        if generated:
            template, _kind = generated
            generated_tag_templates.append(template)
            covered_tags.add(tag_name)

    generated_filter_templates = []
    for name, spec in filters.items():
        generated = generate_filter_template(name, spec)
        if generated:
            template, _kind = generated
            generated_filter_templates.append(template)
            covered_filters.add(name)
    covered_filters.update(
        collect_filters_from_templates(generated_filter_templates, opaque_blocks)
    )

    missing_tag_cases = sorted(registered_tags - covered_tags)
    missing_filter_cases = sorted(registered_filters - covered_filters)

    report_path = Path(__file__).resolve().parents[1] / "reports" / "case_coverage.json"
    report_path.parent.mkdir(parents=True, exist_ok=True)
    report_path.write_text(
        json.dumps(
            {
                "registered_tags": len(registered_tags),
                "covered_tags": len(covered_tags),
                "missing_tag_cases": missing_tag_cases,
                "registered_filters": len(registered_filters),
                "covered_filters": len(covered_filters),
                "missing_filter_cases": missing_filter_cases,
            },
            indent=2,
            sort_keys=True,
        )
    )

    if missing_tag_cases:
        warnings.warn(
            f"Tags without test cases: {missing_tag_cases}",
            stacklevel=2,
        )
    if missing_filter_cases:
        warnings.warn(
            f"Filters without test cases: {missing_filter_cases}",
            stacklevel=2,
        )
