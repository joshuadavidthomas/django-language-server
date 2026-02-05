from __future__ import annotations

from pathlib import Path

from template_linter.resolution.bundle import extract_bundle_from_django


def test_django_bundle_includes_legacy_ifequal_stubs(django_root: Path) -> None:
    bundle = extract_bundle_from_django(django_root)
    for name in ("ifequal", "ifnotequal", "endifequal", "endifnotequal"):
        assert name in bundle.rules
        assert bundle.rules[name].unrestricted is True

