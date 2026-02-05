"""Extraction entry points.

File and tree traversal for rule extraction.
"""

from __future__ import annotations

import ast
from pathlib import Path

from ..types import TagValidation
from .files import iter_tag_files
from .helpers import _augment_tag_validations_with_helpers
from .registry import collect_registered_tags
from .rules import RuleExtractor


def extract_from_file(filepath: Path) -> dict[str, TagValidation]:
    """Extract all validation rules from a Django template tags file."""
    source = filepath.read_text()
    tree = ast.parse(source)
    registered_tags = collect_registered_tags(tree)
    extractor = RuleExtractor(source, str(filepath))
    extractor.visit(tree)
    rules = extractor.rules_by_tag
    for tag_name in registered_tags:
        if tag_name not in rules:
            rules[tag_name] = TagValidation(
                tag_name=tag_name,
                file_path=str(filepath),
            )
    _augment_tag_validations_with_helpers(tree, source, str(filepath), rules)
    rules = {k: v for k, v in rules.items() if k in registered_tags}
    return rules


def extract_from_django(django_root: Path) -> dict[str, TagValidation]:
    """Extract validation rules from all Django template tag files."""
    all_rules: dict[str, TagValidation] = {}

    for filepath in iter_tag_files(django_root):
        if filepath.exists():
            rules = extract_from_file(filepath)
            all_rules.update(rules)

    return all_rules
