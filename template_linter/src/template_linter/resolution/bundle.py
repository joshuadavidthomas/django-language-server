"""
Bundled extraction helpers.

The core extractor functions operate per file or per Django checkout. For corpus
and "real project" validation we often want to:
- extract from many `templatetags/**/*.py` files under a project root
- combine those results with Django's built-ins
- merge conservatively when collisions are ambiguous (until we model `{% load %}`)
"""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
from typing import Literal
from typing import TypeVar

from ..extraction.api import extract_from_django
from ..extraction.api import extract_from_file
from ..extraction.filters import FilterSpec
from ..extraction.filters import extract_filters_from_django
from ..extraction.filters import extract_filters_from_file
from ..extraction.opaque import extract_opaque_blocks_from_django
from ..extraction.opaque import extract_opaque_blocks_from_file
from ..extraction.structural import extract_block_specs_from_django
from ..extraction.structural import extract_block_specs_from_file
from ..extraction.structural import extract_structural_rules_from_django
from ..extraction.structural import extract_structural_rules_from_file
from .compat import apply_legacy_unrestricted_tag_stubs
from ..types import BlockTagSpec
from ..types import ConditionalInnerTagRule
from ..types import OpaqueBlockSpec
from ..types import TagValidation
from ..types import merge_opaque_blocks

CollisionPolicy = Literal["omit", "override", "error"]
T = TypeVar("T")


@dataclass(frozen=True, slots=True)
class ExtractionBundle:
    rules: dict[str, TagValidation]
    filters: dict[str, FilterSpec]
    opaque_blocks: dict[str, OpaqueBlockSpec]
    structural_rules: list[ConditionalInnerTagRule]
    block_specs: list[BlockTagSpec]


def empty_bundle() -> ExtractionBundle:
    return ExtractionBundle(
        rules={},
        filters={},
        opaque_blocks={},
        structural_rules=[],
        block_specs=[],
    )


def extract_bundle_from_file(path: Path) -> ExtractionBundle:
    return ExtractionBundle(
        rules=extract_from_file(path),
        filters=extract_filters_from_file(path),
        opaque_blocks=extract_opaque_blocks_from_file(path),
        structural_rules=extract_structural_rules_from_file(path),
        block_specs=_dedupe_block_specs(extract_block_specs_from_file(path)),
    )


def select_from_bundle(
    bundle: ExtractionBundle,
    *,
    tags: set[str] | None = None,
    filters: set[str] | None = None,
) -> ExtractionBundle:
    """
    Return a new bundle containing only the selected tag/filter exports.

    Structural rules and block specs are kept as-is (they're extracted from the
    same module as the tags).
    """
    selected_rules = (
        {k: v for k, v in bundle.rules.items() if k in tags}
        if tags is not None
        else dict(bundle.rules)
    )
    selected_filters = (
        {k: v for k, v in bundle.filters.items() if k in filters}
        if filters is not None
        else dict(bundle.filters)
    )
    return ExtractionBundle(
        rules=selected_rules,
        filters=selected_filters,
        opaque_blocks=dict(bundle.opaque_blocks),
        structural_rules=list(bundle.structural_rules),
        block_specs=list(bundle.block_specs),
    )


def extract_bundle_from_django(django_root: Path) -> ExtractionBundle:
    rules = apply_legacy_unrestricted_tag_stubs(extract_from_django(django_root))
    return ExtractionBundle(
        rules=rules,
        filters=extract_filters_from_django(django_root),
        opaque_blocks=extract_opaque_blocks_from_django(django_root),
        structural_rules=extract_structural_rules_from_django(django_root),
        block_specs=_dedupe_block_specs(extract_block_specs_from_django(django_root)),
    )


def extract_bundle_from_templatetags(project_root: Path) -> ExtractionBundle:
    """
    Extract from all `templatetags/**/*.py` under `project_root`.

    This does not perform `{% load %}` resolution; callers should merge
    conservatively when combining multiple libraries.
    """
    all_rules: dict[str, TagValidation] = {}
    all_filters: dict[str, FilterSpec] = {}
    all_opaque: dict[str, OpaqueBlockSpec] = {}
    all_structural: list[ConditionalInnerTagRule] = []
    all_blocks: list[BlockTagSpec] = []

    for path in sorted(project_root.rglob("templatetags/**/*.py")):
        all_rules.update(extract_from_file(path))
        all_filters.update(extract_filters_from_file(path))
        all_opaque = merge_opaque_blocks(
            all_opaque, extract_opaque_blocks_from_file(path)
        )
        all_structural.extend(extract_structural_rules_from_file(path))
        all_blocks.extend(extract_block_specs_from_file(path))

    return ExtractionBundle(
        rules=all_rules,
        filters=all_filters,
        opaque_blocks=all_opaque,
        structural_rules=all_structural,
        block_specs=_dedupe_block_specs(all_blocks),
    )


def merge_named_dict(
    base: dict[str, T],
    extra: dict[str, T],
    *,
    what: str,
    collision_policy: CollisionPolicy = "omit",
) -> dict[str, T]:
    """
    Merge dicts with a configurable collision policy.

    This is intentionally conservative by default: without `{% load %}`
    resolution, a collision is ambiguous.
    """
    if collision_policy not in ("omit", "override", "error"):
        raise ValueError(f"Unknown collision_policy={collision_policy!r}")

    merged = dict(base)
    for name, val in extra.items():
        if name not in merged:
            merged[name] = val
            continue
        if collision_policy == "override":
            merged[name] = val
            continue
        if collision_policy == "error":
            raise ValueError(f"Collision for {what} name {name!r}")
        # "omit": remove the key so callers don't validate with a potentially wrong spec.
        merged.pop(name, None)
    return merged


def merge_bundles(
    base: ExtractionBundle,
    extra: ExtractionBundle,
    *,
    collision_policy: CollisionPolicy = "omit",
) -> ExtractionBundle:
    rules = merge_named_dict(
        base.rules, extra.rules, what="tag", collision_policy=collision_policy
    )
    filters = merge_named_dict(
        base.filters, extra.filters, what="filter", collision_policy=collision_policy
    )
    opaque = merge_opaque_blocks(base.opaque_blocks, extra.opaque_blocks)
    structural = list(base.structural_rules) + list(extra.structural_rules)
    blocks = _dedupe_block_specs(list(base.block_specs) + list(extra.block_specs))
    return ExtractionBundle(
        rules=rules,
        filters=filters,
        opaque_blocks=opaque,
        structural_rules=structural,
        block_specs=blocks,
    )


def _dedupe_block_specs(specs: list[BlockTagSpec]) -> list[BlockTagSpec]:
    seen: set[tuple[tuple[str, ...], tuple[str, ...], tuple[str, ...]]] = set()
    out: list[BlockTagSpec] = []
    for spec in specs:
        key = (
            tuple(sorted(spec.start_tags)),
            tuple(sorted(spec.end_tags)),
            tuple(sorted(spec.middle_tags)),
        )
        if key in seen:
            continue
        seen.add(key)
        out.append(spec)
    return out
