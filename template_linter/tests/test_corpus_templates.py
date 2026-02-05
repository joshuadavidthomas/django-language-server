from __future__ import annotations

from pathlib import Path

import pytest

from template_linter.resolution.bundle import ExtractionBundle
from template_linter.resolution.bundle import extract_bundle_from_django
from template_linter.resolution.bundle import extract_bundle_from_templatetags
from template_linter.resolution.bundle import merge_bundles
from template_linter.resolution.load import LibraryIndex
from template_linter.resolution.load import build_library_index
from template_linter.resolution.runtime_registry import build_runtime_environment
from template_linter.resolution.runtime_registry import load_runtime_registry
from template_linter.validation.template import validate_template
from template_linter.validation.template import validate_template_with_load_resolution


def _format_errors(errors, *, limit: int) -> str:
    lines: list[str] = []
    for e in errors[:limit]:
        tag = e.tag.raw or " ".join(e.tag.tokens)
        lines.append(f"line {e.tag.line}: {tag}: {e.message}")
    more = "" if len(errors) <= limit else f"\n... and {len(errors) - limit} more"
    return "\n".join(lines) + more


def _find_django_root_for_entry(entry: Path) -> Path | None:
    # For Django sdists, the package root is entry/django/
    candidate = entry / "django"
    if (candidate / "template" / "defaulttags.py").exists():
        return candidate
    return None


def _bundle_for_template(
    *,
    base: ExtractionBundle,
    entry: Path,
    corpus_django_version_rules: dict[str, ExtractionBundle],
    corpus_entry_extractions: dict[str, ExtractionBundle],
) -> ExtractionBundle:
    # Version-aware Django validation: if this template comes from a Django
    # sdist entry, extract rules from that Django source tree instead of using
    # the local checkout's rules.
    django_entry_root = _find_django_root_for_entry(entry)
    if django_entry_root is not None:
        cache_key = str(entry)
        cached = corpus_django_version_rules.get(cache_key)
        if cached is None:
            cached = extract_bundle_from_django(django_entry_root)
            corpus_django_version_rules[cache_key] = cached
        return cached

    # Entry-local extraction for third-party projects/frameworks (Wagtail, CMS,
    # NetBox, etc.). This is the core feasibility test: extract rules from the
    # project's own `templatetags/**/*.py` and validate its templates.
    cache_key = str(entry)
    entry_cached = corpus_entry_extractions.get(cache_key)
    if entry_cached is None:
        entry_cached = extract_bundle_from_templatetags(entry)
        corpus_entry_extractions[cache_key] = entry_cached

    # Without `{% load %}` resolution, collisions are ambiguous; omit them
    # rather than validating a tag/filter against a potentially wrong signature.
    return merge_bundles(base, entry_cached, collision_policy="omit")


@pytest.fixture(scope="session")
def corpus_django_library_index(django_root: Path) -> LibraryIndex:
    """
    Static LibraryIndex for the local Django checkout (for `{% load %}` resolution).
    """
    return build_library_index(django_root)


@pytest.fixture(scope="session")
def corpus_django_version_rules() -> dict[str, ExtractionBundle]:
    """
    Cache extracted rules per corpus Django version entry.

    Keyed by the corpus entry path string.
    """
    return {}


@pytest.fixture(scope="session")
def corpus_entry_extractions() -> dict[str, ExtractionBundle]:
    """Cache extracted rules per non-Django corpus entry (keyed by entry path)."""
    return {}


@pytest.fixture(scope="session")
def corpus_django_version_indexes() -> dict[str, LibraryIndex]:
    """Cache LibraryIndex per corpus Django version entry (keyed by entry path)."""
    return {}


@pytest.fixture(scope="session")
def corpus_entry_indexes() -> dict[str, LibraryIndex]:
    """Cache LibraryIndex per non-Django corpus entry (keyed by entry path)."""
    return {}


@pytest.fixture(scope="session")
def corpus_entry_runtime_envs() -> dict[str, tuple[LibraryIndex, ExtractionBundle]]:
    """
    Cache runtime-derived (LibraryIndex, builtins bundle) per entry.

    If an entry root contains `.runtime_registry.json`, strict corpus validation
    can model:
    - exact installed library mapping (so `{% load %}` collisions behave like Django)
    - project configured builtins (tags/filters available without `{% load %}`)
    """
    return {}


def _runtime_env_for_entry(
    entry: Path,
    *,
    cache: dict[str, tuple[LibraryIndex, ExtractionBundle]],
) -> tuple[LibraryIndex, ExtractionBundle] | None:
    registry_path = entry / ".runtime_registry.json"
    if not registry_path.exists():
        return None
    cache_key = str(entry)
    cached = cache.get(cache_key)
    if cached is not None:
        return cached
    registry = load_runtime_registry(registry_path)
    corpus_root = next((p for p in entry.parents if p.name == ".corpus"), None)
    extra_sys_path = [entry]
    if corpus_root is not None:
        extra_sys_path.extend(sorted((corpus_root / "packages").glob("*/*")))
        extra_sys_path.extend(sorted((corpus_root / "repos").glob("*/*")))
    env = build_runtime_environment(registry, extra_sys_path=extra_sys_path)
    cache[cache_key] = env
    return env


def test_corpus_template_validate_sample(
    django_bundle: ExtractionBundle,
    corpus_template_sample,
    corpus_django_version_rules: dict[str, ExtractionBundle],
    corpus_entry_extractions: dict[str, ExtractionBundle],
    corpus_django_library_index: LibraryIndex,
    corpus_django_version_indexes: dict[str, LibraryIndex],
    corpus_entry_indexes: dict[str, LibraryIndex],
    corpus_entry_runtime_envs: dict[str, tuple[LibraryIndex, ExtractionBundle]],
    request: pytest.FixtureRequest,
) -> None:
    """
    Validate one corpus template from the sampled set.

    Selection is controlled by pytest flags:
    - `--corpus-sample-per-entry` (default 50; use 0 to validate all)
    - `--corpus-include-test-templates`
    - `--corpus-include-doc-templates`
    """
    if corpus_template_sample is None:
        pytest.skip("No corpus templates found. Run `just corpus-sync`.")
    assert corpus_template_sample is not None
    entry = corpus_template_sample.entry
    template_path = corpus_template_sample.template

    bundle = _bundle_for_template(
        base=django_bundle,
        entry=entry,
        corpus_django_version_rules=corpus_django_version_rules,
        corpus_entry_extractions=corpus_entry_extractions,
    )

    text = template_path.read_text(encoding="utf-8", errors="replace")
    strict_unknown = bool(request.config.getoption("--corpus-strict-unknown"))

    if not strict_unknown:
        errors = validate_template(
            text,
            bundle.rules,
            filters=bundle.filters,
            opaque_blocks=bundle.opaque_blocks,
            report_unknown_tags=False,
            report_unknown_filters=False,
            structural_rules=bundle.structural_rules,
            block_specs=bundle.block_specs,
        )
    else:
        django_entry_root = _find_django_root_for_entry(entry)
        if django_entry_root is not None:
            # For Django sdists, use that version's template library index.
            cache_key = str(entry)
            idx = corpus_django_version_indexes.get(cache_key)
            if idx is None:
                idx = build_library_index(django_entry_root)
                corpus_django_version_indexes[cache_key] = idx
            django_index = idx
            entry_index = None
            base_rules = bundle.rules
            base_filters = bundle.filters
        else:
            # For non-Django entries: strict unknowns only enforce entry-local tags/filters
            # are available via `{% load %}`. Django built-ins are treated as available.
            runtime_env = _runtime_env_for_entry(entry, cache=corpus_entry_runtime_envs)
            if runtime_env is not None:
                runtime_index, builtins_bundle = runtime_env
                # Runtime registry provides entry/project-specific libraries and builtins.
                # Keep Django's full index available for `{% load %}` resolution of Django
                # and contrib libraries.
                django_index = corpus_django_library_index
                entry_index = runtime_index
                merged = merge_bundles(
                    django_bundle, builtins_bundle, collision_policy="override"
                )
                base_rules = merged.rules
                base_filters = merged.filters
            else:
                cache_key = str(entry)
                entry_index = corpus_entry_indexes.get(cache_key)
                if entry_index is None:
                    entry_index = build_library_index(entry)
                    corpus_entry_indexes[cache_key] = entry_index
                django_index = corpus_django_library_index
                base_rules = django_bundle.rules
                base_filters = django_bundle.filters

        errors = validate_template_with_load_resolution(
            text,
            base_rules,
            base_filters=base_filters,
            opaque_blocks=bundle.opaque_blocks,
            django_index=django_index,
            entry_index=entry_index,
            report_unknown_tags=True,
            report_unknown_filters=True,
            report_unknown_libraries=True,
            structural_rules=bundle.structural_rules,
            block_specs=bundle.block_specs,
        )

    assert not errors, (
        f"Template validation failures in {template_path}\n"
        + _format_errors(errors, limit=10)
    )


@pytest.mark.slow
def test_corpus_template_validate_full(
    django_bundle: ExtractionBundle,
    corpus_template_full,
    corpus_django_version_rules: dict[str, ExtractionBundle],
    corpus_entry_extractions: dict[str, ExtractionBundle],
    corpus_django_library_index: LibraryIndex,
    corpus_django_version_indexes: dict[str, LibraryIndex],
    corpus_entry_indexes: dict[str, LibraryIndex],
    corpus_entry_runtime_envs: dict[str, tuple[LibraryIndex, ExtractionBundle]],
    request: pytest.FixtureRequest,
) -> None:
    """
    Validate one corpus template from the full set.

    Enabled by passing `--corpus-templates-full`.
    """
    if corpus_template_full is None:
        pytest.skip("Pass --corpus-templates-full to run (and ensure corpus exists).")
    assert corpus_template_full is not None
    entry = corpus_template_full.entry
    template_path = corpus_template_full.template

    bundle = _bundle_for_template(
        base=django_bundle,
        entry=entry,
        corpus_django_version_rules=corpus_django_version_rules,
        corpus_entry_extractions=corpus_entry_extractions,
    )

    text = template_path.read_text(encoding="utf-8", errors="replace")
    strict_unknown = bool(request.config.getoption("--corpus-strict-unknown"))

    if not strict_unknown:
        errors = validate_template(
            text,
            bundle.rules,
            filters=bundle.filters,
            opaque_blocks=bundle.opaque_blocks,
            report_unknown_tags=False,
            report_unknown_filters=False,
            structural_rules=bundle.structural_rules,
            block_specs=bundle.block_specs,
        )
    else:
        django_entry_root = _find_django_root_for_entry(entry)
        if django_entry_root is not None:
            cache_key = str(entry)
            idx = corpus_django_version_indexes.get(cache_key)
            if idx is None:
                idx = build_library_index(django_entry_root)
                corpus_django_version_indexes[cache_key] = idx
            django_index = idx
            entry_index = None
            base_rules = bundle.rules
            base_filters = bundle.filters
        else:
            runtime_env = _runtime_env_for_entry(entry, cache=corpus_entry_runtime_envs)
            if runtime_env is not None:
                runtime_index, builtins_bundle = runtime_env
                django_index = runtime_index
                entry_index = None
                merged = merge_bundles(
                    django_bundle, builtins_bundle, collision_policy="override"
                )
                base_rules = merged.rules
                base_filters = merged.filters
            else:
                cache_key = str(entry)
                entry_index = corpus_entry_indexes.get(cache_key)
                if entry_index is None:
                    entry_index = build_library_index(entry)
                    corpus_entry_indexes[cache_key] = entry_index
                django_index = corpus_django_library_index
                base_rules = django_bundle.rules
                base_filters = django_bundle.filters

        errors = validate_template_with_load_resolution(
            text,
            base_rules,
            base_filters=base_filters,
            opaque_blocks=bundle.opaque_blocks,
            django_index=django_index,
            entry_index=entry_index,
            report_unknown_tags=True,
            report_unknown_filters=True,
            report_unknown_libraries=True,
            structural_rules=bundle.structural_rules,
            block_specs=bundle.block_specs,
        )

    assert not errors, (
        f"Template validation failures in {template_path}\n"
        + _format_errors(errors, limit=10)
    )
