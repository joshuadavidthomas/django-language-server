from __future__ import annotations

from pathlib import Path

from lsprotocol.types import Position


def position_in(path: Path, needle: str) -> Position:
    return _position_at(path, needle, 0)


def position_after(path: Path, needle: str) -> Position:
    return _position_at(path, needle, len(needle))


def _position_at(path: Path, needle: str, needle_offset: int) -> Position:
    if not needle:
        raise AssertionError("position needle must not be empty")

    text = path.read_text(encoding="utf-8")
    needle_start = text.find(needle)
    if needle_start == -1:
        raise AssertionError(f"{needle!r} was not found in {path}")

    second_offset = text.find(needle, needle_start + 1)
    if second_offset != -1:
        raise AssertionError(
            f"{needle!r} is ambiguous in {path}; use a more specific needle"
        )

    offset = needle_start + needle_offset
    before = text[:offset]
    line = before.count("\n")
    line_start = before.rfind("\n") + 1
    return Position(line=line, character=offset - line_start)
