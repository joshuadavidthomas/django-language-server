from __future__ import annotations

from django.conf import settings
from django.template.engine import Engine

from corpus.inspect_runtime import inspect_engine


def test_inspect_engine_matches_default_engine() -> None:
    if not settings.configured:
        settings.configure(
            TEMPLATES=[
                {
                    "BACKEND": "django.template.backends.django.DjangoTemplates",
                    "APP_DIRS": True,
                    "OPTIONS": {},
                }
            ]
        )

    import django

    django.setup()

    engine = Engine.get_default()
    out = inspect_engine(engine)

    assert out.builtins == engine.builtins

    # A couple of stable defaults (these are Django's standard loadable libs).
    assert out.libraries["static"] == "django.templatetags.static"
    assert out.libraries["i18n"] == "django.templatetags.i18n"
