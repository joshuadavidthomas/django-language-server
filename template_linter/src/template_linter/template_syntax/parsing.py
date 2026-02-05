"""
Template parsing helpers.

These functions convert the canonical token stream (`tokenize_template`) into
the light-weight structures used by validators (`TemplateTag`, variable tokens).
"""

from __future__ import annotations

from ..types import OpaqueBlockSpec
from ..types import TemplateTag
from .tokenization import tokenize_template


def parse_template_tags(
    template: str,
    force_fallback: bool = False,
    opaque_blocks: dict[str, OpaqueBlockSpec] | None = None,
) -> list[TemplateTag]:
    """
    Parse template BLOCK tags from a template string, honoring opaque blocks.
    """
    tags: list[TemplateTag] = []
    for tok in tokenize_template(
        template, force_fallback=force_fallback, opaque_blocks=opaque_blocks
    ):
        if tok.kind != "block" or not tok.split or not tok.name:
            continue
        tags.append(
            TemplateTag(
                name=tok.name, tokens=tok.split, raw=tok.contents, line=tok.line
            )
        )
    return tags


def parse_template_vars(
    template: str,
    force_fallback: bool = False,
    opaque_blocks: dict[str, OpaqueBlockSpec] | None = None,
) -> list[tuple[str, int]]:
    """
    Extract variable token contents using the canonical tokenizer.

    Returns list of (contents, line).
    """
    vars_: list[tuple[str, int]] = []
    for tok in tokenize_template(
        template, force_fallback=force_fallback, opaque_blocks=opaque_blocks
    ):
        if tok.kind != "var":
            continue
        vars_.append((tok.contents, tok.line))
    return vars_
