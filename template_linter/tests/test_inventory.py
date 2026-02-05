from __future__ import annotations

import ast
import json
import warnings
from pathlib import Path

from template_linter.extraction.files import iter_filter_files
from template_linter.extraction.files import iter_tag_files
from template_linter.extraction.registry import collect_registered_filters
from template_linter.extraction.registry import collect_registered_tags


def _collect_registered_tags(django_root: Path) -> set[str]:
    tags: set[str] = set()
    for path in iter_tag_files(django_root):
        source = path.read_text()
        tree = ast.parse(source)
        tags.update(collect_registered_tags(tree))
    return tags


def _collect_registered_filters(django_root: Path) -> set[str]:
    filters: set[str] = set()
    for path in iter_filter_files(django_root):
        source = path.read_text()
        tree = ast.parse(source)
        filters.update(collect_registered_filters(tree))
    return filters


def _has_validation(tag_validation, opaque_blocks) -> bool:
    if tag_validation.tag_name in opaque_blocks:
        return True
    return bool(tag_validation.rules) or (
        tag_validation.parse_bits_spec is not None
        or bool(tag_validation.valid_options)
        or bool(tag_validation.option_constraints)
        or tag_validation.no_duplicate_options
        or tag_validation.rejects_unknown_options
        or getattr(tag_validation, "unrestricted", False)
    )


def test_tag_inventory_complete(django_root: Path, rules, opaque_blocks):
    expected = _collect_registered_tags(django_root)
    extracted = set(rules.keys())
    missing = sorted(expected - extracted)
    assert not missing, f"Missing tag extractions: {missing}"

    zero_rules = sorted(
        name for name, val in rules.items() if not _has_validation(val, opaque_blocks)
    )
    if zero_rules:
        warnings.warn(
            f"Tags without validation rules: {zero_rules}",
            stacklevel=2,
        )

    _write_inventory_report(missing_tags=missing, zero_rule_tags=zero_rules)


def test_filter_inventory_complete(django_root: Path, filters):
    expected = _collect_registered_filters(django_root)
    extracted = set(filters.keys())
    missing = sorted(expected - extracted)
    assert not missing, f"Missing filter extractions: {missing}"
    _write_inventory_report(missing_filters=missing)


def _write_inventory_report(
    missing_tags: list[str] | None = None,
    zero_rule_tags: list[str] | None = None,
    missing_filters: list[str] | None = None,
) -> None:
    report_path = Path(__file__).resolve().parents[1] / "reports" / "inventory.json"
    report_path.parent.mkdir(parents=True, exist_ok=True)
    data = {
        "missing_tags": missing_tags or [],
        "zero_rule_tags": zero_rule_tags or [],
        "missing_filters": missing_filters or [],
    }
    report_path.write_text(json.dumps(data, indent=2, sort_keys=True))
