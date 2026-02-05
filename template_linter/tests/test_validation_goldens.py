from __future__ import annotations

import json
from pathlib import Path

import pytest

from template_linter.resolution.bundle import extract_bundle_from_django
from template_linter.resolution.load import build_library_index
from template_linter.validation.template import validate_template_with_load_resolution


def _fixtures_dir() -> Path:
    return Path(__file__).resolve().parent / "fixtures" / "validation"


def _golden_dir() -> Path:
    return Path(__file__).resolve().parent / "goldens" / "validation"


def _format_error(e) -> dict:
    tag = e.tag
    return {
        "line": int(tag.line),
        "name": str(tag.name),
        "raw": str(tag.raw),
        "message": str(e.message),
    }


@pytest.mark.parametrize(
    "name",
    [
        "unknown_tag",
        "unknown_filter",
        "filter_arg_count",
        "block_structure",
        "if_expression",
        "load_scoping_unknown",
    ],
)
def test_validation_golden(name: str, django_root: Path) -> None:
    """
    End-to-end validation goldens.

    This pins the observable diagnostics list for port parity.
    """
    template_path = _fixtures_dir() / f"{name}.html"
    expected_path = _golden_dir() / f"{name}.json"

    template_text = template_path.read_text(encoding="utf-8")

    bundle = extract_bundle_from_django(django_root)
    django_index = build_library_index(django_root)

    errors = validate_template_with_load_resolution(
        template_text,
        bundle.rules,
        base_filters=bundle.filters,
        opaque_blocks=bundle.opaque_blocks,
        django_index=django_index,
        entry_index=None,
        report_unknown_tags=True,
        report_unknown_filters=True,
        report_unknown_libraries=True,
        structural_rules=bundle.structural_rules,
        block_specs=bundle.block_specs,
    )

    actual = [_format_error(e) for e in errors]

    if not expected_path.exists():
        pytest.fail(
            "Missing golden file.\n"
            f"Add {expected_path} with:\n"
            + json.dumps(actual, indent=2, sort_keys=True)
            + "\n"
        )

    expected = json.loads(expected_path.read_text(encoding="utf-8"))
    assert actual == expected
