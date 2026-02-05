#!/usr/bin/env python3
"""
Unwrap markdown paragraphs that were hard-wrapped at arbitrary line lengths.

Rules:
- Join consecutive lines that are part of the same logical block (paragraph or list item)
- Preserve blank lines (paragraph separators)
- Preserve markdown structures: headers, code blocks, blockquotes, tables, hr
- Handle list item continuations (join wrapped list items)
- Preserve frontmatter (YAML between --- markers)
"""

import re
import sys
from pathlib import Path


def starts_new_block(line: str) -> bool:
    """Check if a line starts a new block that shouldn't be joined to previous."""
    stripped = line.strip()

    # Empty line - paragraph break
    if not stripped:
        return True

    # Header
    if re.match(r'^#{1,6}\s', stripped):
        return True

    # Unordered list item (-, *, +)
    if re.match(r'^[-*+]\s', stripped):
        return True

    # Ordered list item (1. 2. etc)
    if re.match(r'^\d+\.\s', stripped):
        return True

    # Blockquote
    if stripped.startswith('>'):
        return True

    # Code fence
    if stripped.startswith('```') or stripped.startswith('~~~'):
        return True

    # Table row (starts with |)
    if stripped.startswith('|'):
        return True

    # Horizontal rule (---, ***, ___)
    if re.match(r'^[-*_]{3,}\s*$', stripped):
        return True

    # Indented code block (4+ spaces or tab at start of raw line)
    if line.startswith('    ') or line.startswith('\t'):
        return True

    return False


def get_line_indent(line: str) -> int:
    """Get the indentation level of a line."""
    return len(line) - len(line.lstrip())


def unwrap_markdown(content: str) -> str:
    """Unwrap hard-wrapped paragraphs in markdown content."""
    lines = content.split('\n')
    result = []
    i = 0

    # Handle frontmatter
    if lines and lines[0].strip() == '---':
        result.append(lines[0])
        i = 1
        while i < len(lines) and lines[i].strip() != '---':
            result.append(lines[i])
            i += 1
        if i < len(lines):
            result.append(lines[i])  # closing ---
            i += 1

    in_code_block = False
    current_block = []  # Buffer for lines to be joined
    current_block_indent = 0  # Indent level of the current block

    def flush_block():
        """Join buffered block lines and add to result."""
        nonlocal current_block, current_block_indent
        if current_block:
            # Preserve the original indentation of the first line
            indent = ' ' * current_block_indent
            # Join all lines with a single space
            joined = ' '.join(line.strip() for line in current_block)
            result.append(indent + joined)
            current_block = []
            current_block_indent = 0

    while i < len(lines):
        line = lines[i]
        stripped = line.strip()

        # Track code blocks
        if stripped.startswith('```') or stripped.startswith('~~~'):
            flush_block()
            result.append(line)
            in_code_block = not in_code_block
            i += 1
            continue

        # Inside code block - preserve as-is
        if in_code_block:
            result.append(line)
            i += 1
            continue

        # Blank line - flush and preserve
        if not stripped:
            flush_block()
            result.append(line)
            i += 1
            continue

        # Check if this starts a new block
        if starts_new_block(line):
            flush_block()
            # Start a new block with this line
            current_block = [line]
            current_block_indent = get_line_indent(line)
            i += 1
            continue

        # This is a continuation line - add to current block or start new one
        if current_block:
            # Check if this line could be a continuation
            # (not indented code, reasonable indent for continuation)
            line_indent = get_line_indent(line)
            # Allow continuation if indent is similar or less
            # (wrapped lines often have less or same indent)
            current_block.append(line)
        else:
            # Start a new paragraph
            current_block = [line]
            current_block_indent = get_line_indent(line)

        i += 1

    # Flush any remaining block
    flush_block()

    return '\n'.join(result)


def process_file(filepath: Path, dry_run: bool = False) -> bool:
    """Process a single file. Returns True if changes were made."""
    content = filepath.read_text()
    new_content = unwrap_markdown(content)

    if content == new_content:
        return False

    if dry_run:
        print(f"Would modify: {filepath}")
    else:
        filepath.write_text(new_content)
        print(f"Modified: {filepath}")

    return True


def main():
    import argparse

    parser = argparse.ArgumentParser(description='Unwrap hard-wrapped markdown paragraphs')
    parser.add_argument('paths', nargs='+', help='Files or directories to process')
    parser.add_argument('--dry-run', '-n', action='store_true', help='Show what would change without modifying')
    args = parser.parse_args()

    modified = 0
    for path_str in args.paths:
        path = Path(path_str)
        if path.is_file():
            if process_file(path, args.dry_run):
                modified += 1
        elif path.is_dir():
            for md_file in path.rglob('*.md'):
                if process_file(md_file, args.dry_run):
                    modified += 1

    print(f"\n{'Would modify' if args.dry_run else 'Modified'}: {modified} file(s)")


if __name__ == '__main__':
    main()
