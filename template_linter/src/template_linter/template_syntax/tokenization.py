"""
Template tokenization.

This module is intentionally "port-critical": it defines the canonical template
token stream used by parsing/validation and golden tests. Keeping tokenization
in one place reduces drift and simplifies the Rust parity story.
"""

from __future__ import annotations

import re
from dataclasses import dataclass
from typing import Literal

from django.template.base import Lexer
from django.template.base import TokenType

from ..types import OpaqueBlockSpec
from ..types import resolve_opaque_blocks


@dataclass(frozen=True, slots=True)
class TemplateToken:
    """
    Canonical template token representation used for porting/parity fixtures.

    Only includes tokens relevant to static analysis:
    - BLOCK tokens: `{% ... %}`
    - VAR tokens: `{{ ... }}`

    Notes:
    - `contents` matches Django's `Token.contents` (no delimiters).
    - `split` matches `Token.split_contents()` for BLOCK tokens.
    - Opaque blocks are honored: tokens inside opaque regions are skipped, except
      for the matching end tag needed to close the region.
    """

    kind: Literal["block", "var"]
    line: int
    contents: str
    split: list[str] | None = None
    name: str | None = None


def tokenize_template(
    template: str,
    *,
    force_fallback: bool = False,
    opaque_blocks: dict[str, OpaqueBlockSpec] | None = None,
) -> list[TemplateToken]:
    """
    Tokenize a template into BLOCK/VAR tokens, honoring opaque blocks.
    """
    out: list[TemplateToken] = []
    opaque_blocks = resolve_opaque_blocks(opaque_blocks)

    if not force_fallback:
        tokens = Lexer(template).tokenize()
        opaque_stack: list[tuple[OpaqueBlockSpec, str]] = []

        for token in tokens:
            if token.token_type == TokenType.BLOCK:
                contents = token.contents
                bits = token.split_contents()
                if not bits:
                    continue

                name = bits[0]
                if opaque_stack:
                    spec, suffix = opaque_stack[-1]
                    if spec.match_suffix:
                        if name in spec.end_tags:
                            end_suffix = contents[len(name) :].strip()
                            if suffix != end_suffix:
                                continue
                        else:
                            continue
                    else:
                        # `skip_past("endtag foo")` matches full contents, not just tag name.
                        # Keep support for legacy `endtag`-only specs too.
                        if contents not in spec.end_tags and name not in spec.end_tags:
                            continue
                    out.append(
                        TemplateToken(
                            kind="block",
                            line=token.lineno,
                            contents=contents,
                            split=bits,
                            name=name,
                        )
                    )
                    opaque_stack.pop()
                    continue

                out.append(
                    TemplateToken(
                        kind="block",
                        line=token.lineno,
                        contents=contents,
                        split=bits,
                        name=name,
                    )
                )

                spec = opaque_blocks.get(name)
                if spec:
                    suffix = contents[len(name) :].strip() if spec.match_suffix else ""
                    opaque_stack.append((spec, suffix))
                continue

            if opaque_stack:
                continue

            if token.token_type == TokenType.VAR:
                out.append(
                    TemplateToken(
                        kind="var",
                        line=token.lineno,
                        contents=token.contents,
                        split=None,
                        name=None,
                    )
                )

        return out

    pattern = re.compile(r"(\{%.+?%\}|\{\{.+?\}\})", re.DOTALL)
    opaque_stack: list[tuple[OpaqueBlockSpec, str]] = []

    for match in pattern.finditer(template):
        tok = match.group(0)
        line = template.count("\n", 0, match.start()) + 1

        if tok.startswith("{%"):
            content = tok[2:-2].strip()
            bits = _split_tokens(content)
            if not bits:
                continue
            name = bits[0]

            if opaque_stack:
                spec, suffix = opaque_stack[-1]
                if spec.match_suffix:
                    if name in spec.end_tags:
                        end_suffix = content[len(name) :].strip()
                        if suffix != end_suffix:
                            continue
                    else:
                        continue
                else:
                    if content not in spec.end_tags and name not in spec.end_tags:
                        continue
                out.append(
                    TemplateToken(
                        kind="block",
                        line=line,
                        contents=content,
                        split=bits,
                        name=name,
                    )
                )
                opaque_stack.pop()
                continue

            out.append(
                TemplateToken(
                    kind="block",
                    line=line,
                    contents=content,
                    split=bits,
                    name=name,
                )
            )

            spec = opaque_blocks.get(name)
            if spec:
                suffix = content[len(name) :].strip() if spec.match_suffix else ""
                opaque_stack.append((spec, suffix))
            continue

        if opaque_stack:
            continue

        if tok.startswith("{{"):
            contents = tok[2:-2].strip()
            out.append(
                TemplateToken(
                    kind="var",
                    line=line,
                    contents=contents,
                    split=None,
                    name=None,
                )
            )

    return out


def _split_tokens(content: str) -> list[str]:
    """
    Split tag content into tokens, respecting quotes.

    Handles: {% tag "arg with spaces" other_arg %}
    """
    tokens: list[str] = []
    current: list[str] = []
    in_quotes = False
    quote_char: str | None = None

    for char in content:
        if char in ('"', "'") and not in_quotes:
            in_quotes = True
            quote_char = char
            current.append(char)
        elif char == quote_char and in_quotes:
            in_quotes = False
            current.append(char)
            quote_char = None
        elif char.isspace() and not in_quotes:
            if current:
                tokens.append("".join(current))
                current = []
        else:
            current.append(char)

    if current:
        tokens.append("".join(current))

    return tokens
