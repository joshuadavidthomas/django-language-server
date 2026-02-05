from __future__ import annotations

from pathlib import Path

import pytest

from template_linter.validation.template import validate_template


def _iter_django_shipped_templates(django_root: Path) -> list[Path]:
    """
    Collect Django's own shipped templates (mostly contrib/admin).

    This is an integration-style corpus test: it helps catch false positives
    and tokenization gaps that don't show up in per-tag unit cases.
    """
    candidates: list[Path] = []
    for base in (django_root / "contrib", django_root / "forms"):
        if base.exists():
            candidates.extend(
                [
                    p
                    for p in base.rglob("templates/**/*")
                    if p.is_file() and p.suffix in {".html", ".txt"}
                ]
            )
    return sorted(candidates)


def test_django_shipped_templates_validate(
    django_root: Path,
    rules,
    filters,
    opaque_blocks,
    structural_rules,
    block_specs,
):
    paths = _iter_django_shipped_templates(django_root)
    if not paths:
        pytest.skip("No Django shipped templates found to validate.")

    failures: dict[Path, list[str]] = {}
    for path in paths:
        template = path.read_text(encoding="utf-8")
        errors = validate_template(
            template,
            rules,
            filters=filters,
            opaque_blocks=opaque_blocks,
            structural_rules=structural_rules,
            block_specs=block_specs,
            report_unknown_tags=True,
            report_unknown_filters=True,
        )
        if errors:
            failures[path] = [str(e) for e in errors]

    assert not failures, f"Validation errors in shipped templates: {failures}"
