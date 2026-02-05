from __future__ import annotations

from pathlib import Path

from tests.conftest import _filter_templates


def test_corpus_template_filter_excludes_static_and_known_bad_suffixes() -> None:
    paths = [
        Path("/x/repo/templates/ok.html"),
        Path("/x/geonode/static/geonode/js/templates/cart.html"),
        Path("/x/babybuddy/templates/error/404.html"),
        Path("/x/src/sentry/templates/sentry/emails/onboarding-continuation.html"),
    ]

    filtered = _filter_templates(paths, include_tests=False, include_docs=False)
    assert filtered == [Path("/x/repo/templates/ok.html")]

