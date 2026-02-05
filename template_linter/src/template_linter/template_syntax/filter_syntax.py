"""
Filter expression parsing for templates.

This module is intentionally small and deterministic so it can be ported and
tested independently (used by `validate_filters`).
"""

from __future__ import annotations


def _split_unquoted(s: str, sep: str) -> list[str]:
    parts: list[str] = []
    current: list[str] = []
    in_quotes = False
    quote_char = ""

    for ch in s:
        if ch in ("'", '"'):
            if not in_quotes:
                in_quotes = True
                quote_char = ch
            elif quote_char == ch:
                in_quotes = False
                quote_char = ""
        if ch == sep and not in_quotes:
            parts.append("".join(current))
            current = []
        else:
            current.append(ch)
    parts.append("".join(current))
    return parts


def _split_first_unquoted(s: str, sep: str) -> tuple[str, str | None]:
    in_quotes = False
    quote_char = ""
    for i, ch in enumerate(s):
        if ch in ("'", '"'):
            if not in_quotes:
                in_quotes = True
                quote_char = ch
            elif quote_char == ch:
                in_quotes = False
                quote_char = ""
        if ch == sep and not in_quotes:
            return s[:i], s[i + 1 :]
    return s, None


def _parse_filter_chain(expr: str) -> list[tuple[str, bool]]:
    parts = _split_unquoted(expr, "|")
    if len(parts) <= 1:
        return []
    filters: list[tuple[str, bool]] = []
    for part in parts[1:]:
        part = part.strip()
        if not part:
            continue
        name, arg = _split_first_unquoted(part, ":")
        name = name.strip()
        has_arg = arg is not None and arg.strip() != ""
        if name:
            filters.append((name, has_arg))
    return filters


def _extract_filter_exprs_from_token(token: str) -> list[str]:
    """
    Extract filter-able expression(s) from a tag token.

    Handles token_kwargs like `key=value|default:"x"` by returning the
    value expression only. Falls back to the full token otherwise.
    """
    if "|" not in token:
        return []

    # Avoid mistaking equality/inequality operators for kwargs
    if any(op in token for op in ("==", "!=", ">=", "<=")):
        return [token]

    key, value = _split_first_unquoted(token, "=")
    if value is not None:
        key = key.strip()
        if key.isidentifier() and value and not value.startswith("="):
            return [value.strip()]

    return [token]
