from __future__ import annotations

import json
from dataclasses import asdict
from pathlib import Path
from typing import Any

import pytest

from template_linter.resolution.bundle import ExtractionBundle
from template_linter.resolution.bundle import extract_bundle_from_file


def _golden_dir() -> Path:
    return Path(__file__).resolve().parent / "goldens" / "extraction"


def _fixtures_dir() -> Path:
    return Path(__file__).resolve().parent / "fixtures" / "extraction" / "templatetags"


def bundle_to_golden_dict(bundle: ExtractionBundle) -> dict[str, Any]:
    """
    Convert an ExtractionBundle into a stable, JSON-serializable dict.

    This intentionally records *just enough* structure to be useful for parity,
    while keeping the schema easy to reproduce in Rust.
    """

    def _sort_list(xs: list[str]) -> list[str]:
        return sorted(set(xs))

    rules: dict[str, Any] = {}
    for name, tv in sorted(bundle.rules.items(), key=lambda kv: kv[0]):
        rules[name] = {
            "unrestricted": bool(tv.unrestricted),
            "has_parse_bits_spec": tv.parse_bits_spec is not None,
            "rule_count": len(tv.rules),
            "valid_options": sorted(set(tv.valid_options)),
            "rejects_unknown_options": bool(tv.rejects_unknown_options),
            "no_duplicate_options": bool(tv.no_duplicate_options),
            # file_path is intentionally omitted (not stable across machines).
        }

    filters: dict[str, Any] = {}
    for name, spec in sorted(bundle.filters.items(), key=lambda kv: kv[0]):
        # FilterSpec includes file_path which is unstable.
        filters[name] = {
            "pos_args": int(spec.pos_args),
            "defaults": int(spec.defaults),
        }

    opaque: dict[str, Any] = {}
    for name, spec in sorted(bundle.opaque_blocks.items(), key=lambda kv: kv[0]):
        opaque[name] = {
            "end_tags": _sort_list(list(spec.end_tags)),
            "match_suffix": bool(spec.match_suffix),
            "kind": str(spec.kind or ""),
        }

    block_specs: list[dict[str, Any]] = []
    for spec in bundle.block_specs:
        d = asdict(spec)
        # Canonicalize list-like fields for deterministic dumps.
        d["start_tags"] = sorted(set(d.get("start_tags") or []))
        d["end_tags"] = sorted(set(d.get("end_tags") or []))
        d["middle_tags"] = sorted(set(d.get("middle_tags") or []))
        d["repeatable_middle_tags"] = sorted(set(d.get("repeatable_middle_tags") or []))
        d["terminal_middle_tags"] = sorted(set(d.get("terminal_middle_tags") or []))
        block_specs.append(d)
    block_specs.sort(
        key=lambda d: (
            tuple(d.get("start_tags") or []),
            tuple(d.get("end_tags") or []),
            tuple(d.get("middle_tags") or []),
        )
    )

    structural_rules: list[dict[str, Any]] = []
    for rule in bundle.structural_rules:
        d = asdict(rule)
        d["start_tags"] = sorted(set(d.get("start_tags") or []))
        d["end_tags"] = sorted(set(d.get("end_tags") or []))
        structural_rules.append(d)
    structural_rules.sort(
        key=lambda d: (
            tuple(d.get("start_tags") or []),
            tuple(d.get("end_tags") or []),
            d.get("inner_tag") or "",
        )
    )

    return {
        "rules": rules,
        "filters": filters,
        "opaque_blocks": opaque,
        "block_specs": block_specs,
        "structural_rules": structural_rules,
    }


@pytest.mark.parametrize(
    "module_name",
    [
        "basic_tags",
        "opaque_tags",
        "block_tags",
        "registration_variants",
    ],
)
def test_extraction_golden(module_name: str) -> None:
    path = _fixtures_dir() / f"{module_name}.py"
    expected_path = _golden_dir() / f"{module_name}.json"

    bundle = extract_bundle_from_file(path)
    actual = bundle_to_golden_dict(bundle)

    if not expected_path.exists():
        pytest.fail(
            "Missing golden file.\n"
            f"Add {expected_path} with:\n"
            + json.dumps(actual, indent=2, sort_keys=True)
            + "\n"
        )

    expected = json.loads(expected_path.read_text(encoding="utf-8"))
    assert actual == expected
