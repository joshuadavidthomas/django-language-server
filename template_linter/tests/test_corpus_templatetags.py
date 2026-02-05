from __future__ import annotations

from pathlib import Path

import pytest

from template_linter.extraction.api import extract_from_file
from template_linter.extraction.filters import extract_filters_from_file
from template_linter.extraction.structural import extract_block_specs_from_file
from template_linter.extraction.structural import extract_structural_rules_from_file


def test_corpus_extraction_smoke() -> None:
    """
    Smoke-test third-party corpus extraction.

    This test is skipped unless `template_linter/.corpus/` exists. The corpus is
    intentionally gitignored and assembled locally (see `template_linter/corpus/`).
    """
    corpus_root = Path(__file__).resolve().parents[1] / ".corpus"
    if not corpus_root.exists():
        pytest.skip("Third-party corpus not present. Run `just corpus-sync`.")

    roots = []
    for child in ("packages", "repos"):
        p = corpus_root / child
        if p.exists():
            roots.append(p)

    py_files: list[Path] = []
    for root in roots:
        py_files.extend(root.rglob("templatetags/**/*.py"))
    py_files = sorted(py_files)
    if not py_files:
        pytest.skip("Third-party corpus has no templatetags files.")

    # Keep this as a smoke test: we only assert we can parse and extract without
    # crashing across real-world code.
    for path in py_files:
        extract_from_file(path)
        extract_filters_from_file(path)
        extract_structural_rules_from_file(path)
        extract_block_specs_from_file(path)
