from __future__ import annotations

import json
from dataclasses import asdict
from pathlib import Path

import pytest

from template_linter.template_syntax.tokenization import tokenize_template


def _golden_dir() -> Path:
    return Path(__file__).resolve().parent / "goldens" / "tokenization"


def _fixtures_dir() -> Path:
    return Path(__file__).resolve().parent / "fixtures" / "tokenization"


def _load_expected(path: Path) -> list[dict]:
    return json.loads(path.read_text(encoding="utf-8"))


def _dump_actual(template_text: str, *, opaque_blocks) -> list[dict]:
    toks = tokenize_template(template_text, opaque_blocks=opaque_blocks)
    return [asdict(t) for t in toks]


@pytest.mark.parametrize(
    "name",
    [
        "basic",
        "quotes",
        "comment",
        "verbatim_named",
    ],
)
def test_tokenization_golden(name: str, opaque_blocks) -> None:
    template_path = _fixtures_dir() / f"{name}.html"
    expected_path = _golden_dir() / f"{name}.json"

    template_text = template_path.read_text(encoding="utf-8")
    actual = _dump_actual(template_text, opaque_blocks=opaque_blocks)

    if not expected_path.exists():
        pytest.fail(
            "Missing golden file.\n"
            f"Add {expected_path} with:\n"
            + json.dumps(actual, indent=2, sort_keys=True)
            + "\n"
        )

    expected = _load_expected(expected_path)
    assert actual == expected
