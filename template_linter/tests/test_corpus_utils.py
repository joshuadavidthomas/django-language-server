from __future__ import annotations

from pathlib import Path

from corpus.utils import infer_sdist_version
from corpus.utils import strip_archive_suffix
from corpus.utils import to_pip_requirement


def test_to_pip_requirement_pins_patch_versions() -> None:
    assert to_pip_requirement("Django", "6.0.2") == "Django==6.0.2"


def test_to_pip_requirement_pins_minor_versions_exactly() -> None:
    assert to_pip_requirement("Django", "6.0") == "Django==6.0"
    assert to_pip_requirement("wagtail", "7.2") == "wagtail==7.2"


def test_to_pip_requirement_allows_explicit_wildcard() -> None:
    assert to_pip_requirement("Django", "6.0.*") == "Django==6.0.*"


def test_strip_archive_suffix_tar_gz() -> None:
    assert strip_archive_suffix(Path("django-6.0.2.tar.gz")) == "django-6.0.2"

def test_strip_archive_suffix_whl() -> None:
    assert (
        strip_archive_suffix(Path("django_debug_toolbar-4.4.6-py3-none-any.whl"))
        == "django_debug_toolbar-4.4.6-py3-none-any"
    )


def test_infer_sdist_version_best_effort() -> None:
    assert infer_sdist_version("Django", Path("django-6.0.2.tar.gz")) == "6.0.2"
    assert (
        infer_sdist_version(
            "django-debug-toolbar", Path("django_debug_toolbar-4.4.6.tar.gz")
        )
        == "4.4.6"
    )
    assert (
        infer_sdist_version(
            "django-debug-toolbar",
            Path("django_debug_toolbar-4.4.6-py3-none-any.whl"),
        )
        == "4.4.6"
    )
