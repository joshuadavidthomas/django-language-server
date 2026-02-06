from __future__ import annotations

from ..types import TagValidation

LEGACY_UNRESTRICTED_TAGS: tuple[str, ...] = (
    "ifequal",
    "ifnotequal",
    "endifequal",
    "endifnotequal",
)


def apply_legacy_unrestricted_tag_stubs(
    rules: dict[str, TagValidation],
) -> dict[str, TagValidation]:
    """
    Add a small set of legacy Django tag stubs to avoid corpus noise.

    These tags were removed from modern Django but still appear in third-party
    templates. We treat them as known/unrestricted for static validation.
    """
    for name in LEGACY_UNRESTRICTED_TAGS:
        rules.setdefault(name, TagValidation(tag_name=name, unrestricted=True))
    return rules
