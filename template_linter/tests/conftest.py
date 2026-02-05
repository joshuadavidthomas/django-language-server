from __future__ import annotations

import tomllib
from pathlib import Path

import pytest

from template_linter.extraction.api import extract_from_django
from template_linter.extraction.filters import extract_filters_from_django
from template_linter.extraction.opaque import extract_opaque_blocks_from_django
from template_linter.extraction.structural import extract_block_specs_from_django
from template_linter.extraction.structural import extract_structural_rules_from_django
from template_linter.resolution.compat import apply_legacy_unrestricted_tag_stubs
from template_linter.resolution.bundle import ExtractionBundle

ROOT = Path(__file__).resolve().parents[1]

_CORPUS_TEMPLATE_EXCLUDE_SUFFIXES: set[tuple[str, ...]] = {
    # Non-Django / front-end template fragments accidentally living under a
    # `templates/` directory (e.g. AngularJS templates under `static/**/templates`).
    #
    # These are not processed by Django's template engine and tend to produce
    # spurious unknown tag/filter errors in the corpus harness.
    ("geonode", "static", "geonode", "js", "templates", "cart.html"),
    # Known-invalid template snippets in upstream projects. These would raise
    # TemplateSyntaxError in Django itself and are not useful for extraction
    # parity testing.
    ("babybuddy", "templates", "error", "404.html"),
    ("src", "sentry", "templates", "sentry", "emails", "onboarding-continuation.html"),
}


class CorpusTemplateCase:
    __slots__ = ("entry", "template")

    def __init__(self, *, entry: Path, template: Path) -> None:
        self.entry = entry
        self.template = template


def pytest_addoption(parser: pytest.Parser) -> None:
    group = parser.getgroup("template-linter-corpus")
    group.addoption(
        "--corpus-sample-per-entry",
        action="store",
        type=int,
        default=50,
        help=(
            "How many templates to validate per corpus entry for the sampled "
            "corpus-template test. Use 0 to validate all templates (can be slow)."
        ),
    )
    group.addoption(
        "--corpus-templates-full",
        action="store_true",
        default=False,
        help="Enable the full corpus template validation test (very slow).",
    )
    group.addoption(
        "--corpus-include-test-templates",
        action="store_true",
        default=False,
        help="Include templates under tests/fixtures directories (often intentionally invalid).",
    )
    group.addoption(
        "--corpus-include-doc-templates",
        action="store_true",
        default=False,
        help="Include templates under docs/ directories (often snippet fragments).",
    )
    group.addoption(
        "--corpus-strict-unknown",
        action="store_true",
        default=False,
        help=(
            "Enable strict unknown tag/filter reporting for corpus templates, using "
            "static `{% load %}` resolution and scoping."
        ),
    )


def _load_corpus_manifest() -> dict:
    manifest_path = ROOT / "corpus" / "manifest.toml"
    return tomllib.loads(manifest_path.read_text(encoding="utf-8"))


def _iter_corpus_entries() -> list[Path]:
    corpus_root = ROOT / ".corpus"
    if not corpus_root.exists():
        return []

    manifest = _load_corpus_manifest()
    entries: list[Path] = []

    for pkg in manifest.get("package", []):
        p = corpus_root / "packages" / pkg["name"] / pkg["version"]
        if p.exists():
            entries.append(p)

    for repo in manifest.get("repo", []):
        p = corpus_root / "repos" / repo["name"] / repo["ref"]
        if p.exists():
            entries.append(p)

    return entries


def _safe_entry_name(entry: Path) -> str:
    parts = entry.parts
    if "packages" in parts:
        i = parts.index("packages")
        if i + 2 < len(parts):
            return f"pkg__{parts[i + 1]}__{parts[i + 2]}"
    if "repos" in parts:
        i = parts.index("repos")
        if i + 2 < len(parts):
            return f"repo__{parts[i + 1]}__{parts[i + 2]}"
    return entry.name.replace("/", "__")


def _iter_templates_under_entry(entry: Path) -> list[Path]:
    paths: list[Path] = []
    for p in entry.rglob("templates/**/*"):
        if not p.is_file():
            continue
        if p.suffix.lower() not in {".html", ".txt", ".xml"}:
            continue
        paths.append(p)
    return sorted(paths)


def _filter_templates(
    paths: list[Path],
    *,
    include_tests: bool,
    include_docs: bool,
) -> list[Path]:
    excluded_exact = {"test", "tests", "testing", "fixtures"}
    excluded_docs = {"doc", "docs", "documentation"}
    excluded_static = {"static"}
    out: list[Path] = []
    for p in paths:
        parts_lower = [part.lower() for part in p.parts]
        if any(part in excluded_static for part in parts_lower):
            continue
        if any(
            len(p.parts) >= len(suffix) and p.parts[-len(suffix) :] == suffix
            for suffix in _CORPUS_TEMPLATE_EXCLUDE_SUFFIXES
        ):
            continue
        if not include_tests:
            if any(part in excluded_exact for part in parts_lower):
                continue
            if any(part.startswith("test") for part in parts_lower):
                continue
        if not include_docs:
            if any(part in excluded_docs for part in parts_lower):
                continue
        out.append(p)
    return out


def _sample_evenly(paths: list[Path], n: int) -> list[Path]:
    if n <= 0 or len(paths) <= n:
        return paths
    step = len(paths) / n
    out: list[Path] = []
    i = 0.0
    while len(out) < n and int(i) < len(paths):
        out.append(paths[int(i)])
        i += step
    return out


def pytest_generate_tests(metafunc: pytest.Metafunc) -> None:
    """
    Parameterize corpus template validation so failures are isolated per file.

    - `corpus_template_sample` runs a bounded sample per entry (default 50).
    - `corpus_template_full` runs all templates but requires `--corpus-templates-full`.
    """
    entries = _iter_corpus_entries()

    def _make_params(*, full: bool) -> tuple[list[CorpusTemplateCase], list[str]]:
        if not entries:
            return [], []

        include_tests = bool(
            metafunc.config.getoption("--corpus-include-test-templates")
        )
        include_docs = bool(metafunc.config.getoption("--corpus-include-doc-templates"))
        sample_per_entry = int(metafunc.config.getoption("--corpus-sample-per-entry"))

        all_cases: list[CorpusTemplateCase] = []
        ids: list[str] = []
        for entry in entries:
            templates = _filter_templates(
                _iter_templates_under_entry(entry),
                include_tests=include_tests,
                include_docs=include_docs,
            )
            if not templates:
                continue
            selected = (
                templates if full else _sample_evenly(templates, sample_per_entry)
            )
            entry_id = _safe_entry_name(entry)
            for t in selected:
                all_cases.append(CorpusTemplateCase(entry=entry, template=t))
                rel = str(t.relative_to(entry))
                ids.append(f"{entry_id}::{rel}")
        return all_cases, ids

    if "corpus_template_sample" in metafunc.fixturenames:
        params, ids = _make_params(full=False)
        if not params:
            metafunc.parametrize(
                "corpus_template_sample",
                [
                    pytest.param(
                        None,
                        id="no-corpus",
                        marks=pytest.mark.skip(
                            reason="No corpus templates found. Run `just corpus-sync`."
                        ),
                    )
                ],
            )
        else:
            metafunc.parametrize("corpus_template_sample", params, ids=ids)

    if "corpus_template_full" in metafunc.fixturenames:
        if not metafunc.config.getoption("--corpus-templates-full"):
            metafunc.parametrize(
                "corpus_template_full",
                [
                    pytest.param(
                        None,
                        id="full-disabled",
                        marks=pytest.mark.skip(
                            reason="Pass --corpus-templates-full to run."
                        ),
                    )
                ],
            )
        else:
            params, ids = _make_params(full=True)
            if not params:
                metafunc.parametrize(
                    "corpus_template_full",
                    [
                        pytest.param(
                            None,
                            id="no-corpus",
                            marks=pytest.mark.skip(
                                reason="No corpus templates found. Run `just corpus-sync`."
                            ),
                        )
                    ],
                )
            else:
                metafunc.parametrize("corpus_template_full", params, ids=ids)


@pytest.fixture(scope="session")
def corpus_entries() -> list[Path]:
    entries = _iter_corpus_entries()
    if not entries:
        pytest.skip("Third-party corpus not present. Run `just corpus-sync`.")
    return entries


@pytest.fixture(scope="session")
def django_root() -> Path:
    root = ROOT.parent / "django"
    if not root.exists():
        pytest.skip(f"Django source not found at {root}")
    return root


@pytest.fixture(scope="session")
def rules(django_root: Path):
    return apply_legacy_unrestricted_tag_stubs(extract_from_django(django_root))


@pytest.fixture(scope="session")
def filters(django_root: Path):
    return extract_filters_from_django(django_root)


@pytest.fixture(scope="session")
def opaque_blocks(django_root: Path):
    return extract_opaque_blocks_from_django(django_root)


@pytest.fixture(scope="session")
def structural_rules(django_root: Path):
    return extract_structural_rules_from_django(django_root)


@pytest.fixture(scope="session")
def block_specs(django_root: Path):
    return extract_block_specs_from_django(django_root)


@pytest.fixture(scope="session")
def django_bundle(
    rules,
    filters,
    opaque_blocks,
    structural_rules,
    block_specs,
) -> ExtractionBundle:
    return ExtractionBundle(
        rules=rules,
        filters=filters,
        opaque_blocks=opaque_blocks,
        structural_rules=structural_rules,
        block_specs=block_specs,
    )
