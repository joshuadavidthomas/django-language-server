from __future__ import annotations

import tomllib
from pathlib import Path

import pytest

from template_linter.resolution.bundle import merge_bundles
from template_linter.resolution.load import build_library_index
from template_linter.resolution.runtime_registry import build_runtime_environment
from template_linter.resolution.runtime_registry import load_runtime_registry
from template_linter.validation.template import validate_template_with_load_resolution


def _corpus_entry_for_repo(*, root: Path, name: str) -> Path | None:
    manifest_path = root / "corpus" / "manifest.toml"
    manifest = tomllib.loads(manifest_path.read_text(encoding="utf-8"))
    for repo in manifest.get("repo", []):
        if repo.get("name") == name:
            ref = repo.get("ref")
            if not ref:
                return None
            return root / ".corpus" / "repos" / name / ref
    return None


def _extra_sys_path_for_entry(entry: Path) -> list[Path]:
    corpus_root = next((p for p in entry.parents if p.name == ".corpus"), None)
    extra = [entry]
    if corpus_root is not None:
        extra.extend(sorted((corpus_root / "packages").glob("*/*")))
        extra.extend(sorted((corpus_root / "repos").glob("*/*")))
    return extra


def test_known_invalid_sentry_onboarding_template_fails_strict(
    django_bundle,
    django_root: Path,
) -> None:
    """
    Some upstream repos include templates that Django itself would reject.

    We exclude these from the main corpus harness, but keep an explicit test so
    we don't silently "paper over" real syntax errors.
    """
    root = Path(__file__).resolve().parents[1]
    entry = _corpus_entry_for_repo(root=root, name="getsentry-sentry")
    if entry is None or not entry.exists():
        pytest.skip("Sentry corpus entry not present. Run `just corpus-sync`.")

    template_path = (
        entry
        / "src"
        / "sentry"
        / "templates"
        / "sentry"
        / "emails"
        / "onboarding-continuation.html"
    )
    if not template_path.exists():
        pytest.skip("Pinned Sentry template not found in corpus entry.")

    registry_path = entry / ".runtime_registry.json"
    if not registry_path.exists():
        pytest.skip("Runtime registry not present. Run `just corpus-sync`.")

    registry = load_runtime_registry(registry_path)
    runtime_index, builtins_bundle = build_runtime_environment(
        registry, extra_sys_path=_extra_sys_path_for_entry(entry)
    )

    bundle = merge_bundles(django_bundle, builtins_bundle, collision_policy="override")
    text = template_path.read_text(encoding="utf-8", errors="replace")

    errors = validate_template_with_load_resolution(
        text,
        bundle.rules,
        base_filters=bundle.filters,
        opaque_blocks=bundle.opaque_blocks,
        django_index=build_library_index(django_root),
        entry_index=runtime_index,
        report_unknown_tags=True,
        report_unknown_filters=True,
        report_unknown_libraries=True,
        structural_rules=bundle.structural_rules,
        block_specs=bundle.block_specs,
    )

    assert any(
        e.tag.name == "onboarding_link" and "Unknown tag 'onboarding_link'" in e.message
        for e in errors
    ), "Expected the template to fail due to `{% onboarding_link %}` being an unknown tag."
