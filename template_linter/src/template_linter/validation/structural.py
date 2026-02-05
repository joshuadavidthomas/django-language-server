"""
Structural (block-aware) validation.

This module validates:
- delimiter placement/nesting for block tags derived from `parser.parse((...))`
- structural inner-tag requirements (e.g. i18n `plural` rules) extracted from source
"""

from __future__ import annotations

from dataclasses import dataclass
from dataclasses import field
from typing import Any

from ..template_syntax.parsing import parse_template_tags
from ..types import BlockTagSpec
from ..types import ConditionalInnerTagRule
from ..types import OpaqueBlockSpec
from ..types import TemplateTag
from ..types import ValidationError
from ..types import resolve_opaque_blocks
from ..types import simple_error


def _structure_tag_names(
    *,
    structural_rules: list[ConditionalInnerTagRule] | None,
    block_specs: list[BlockTagSpec] | None,
) -> set[str]:
    names: set[str] = set()
    if structural_rules:
        for rule in structural_rules:
            names.update(rule.end_tags)
            names.add(rule.inner_tag)
    if block_specs:
        for spec in block_specs:
            names.update(spec.end_tags)
            names.update(spec.middle_tags)
    return names


def _opaque_end_tag_names(
    opaque_blocks: dict[str, OpaqueBlockSpec] | None,
) -> set[str]:
    names: set[str] = set()
    merged = resolve_opaque_blocks(opaque_blocks)
    for spec in merged.values():
        names.update(spec.end_tags)
    return names


def _format_structural_message(rule: ConditionalInnerTagRule, tag_name: str) -> str:
    message = rule.message or ""
    if not message:
        if rule.require_when_option_present:
            return (
                f"{tag_name} with '{rule.option_token}' requires a "
                f"{{% {rule.inner_tag} %}} block"
            )
        return (
            f"{tag_name} without '{rule.option_token}' cannot include "
            f"{{% {rule.inner_tag} %}}"
        )

    # Avoid `str.format()` since structural messages may include Django template
    # braces like `{% plural %}` which would be interpreted as format fields.
    return (
        message.replace("{tag}", tag_name)
        .replace("{opt}", rule.option_token)
        .replace("{inner}", rule.inner_tag)
    )


def _validate_structural_rules(
    template: str,
    opaque_blocks: dict[str, OpaqueBlockSpec] | None = None,
    structural_rules: list[ConditionalInnerTagRule] | None = None,
) -> list[ValidationError]:
    tags = parse_template_tags(template, opaque_blocks=opaque_blocks)
    errors: list[ValidationError] = []
    stack: list[dict[str, Any]] = []
    rules = structural_rules or []
    if not rules:
        return []

    for tag in tags:
        name = tag.name
        matching_start = [r for r in rules if name in r.start_tags]
        if matching_start:
            options = set(tag.tokens[1:])
            stack.append(
                {
                    "tag": tag,
                    "rules": matching_start,
                    "options": options,
                    "seen_inner": set(),
                }
            )
            continue
        matching_end = [r for r in rules if name in r.end_tags]
        if matching_end:
            if not stack:
                continue
            entry = stack.pop()
            entry_rules = entry["rules"]
            for rule in entry_rules:
                if name not in rule.end_tags:
                    continue
                option_present = rule.option_token in entry["options"]
                if option_present != rule.require_when_option_present:
                    continue
                inner_seen = rule.inner_tag in entry["seen_inner"]
                if rule.require_when_option_present and not inner_seen:
                    message = _format_structural_message(rule, entry["tag"].name)
                    errors.append(simple_error(entry["tag"], message))
                if not rule.require_when_option_present and inner_seen:
                    message = _format_structural_message(rule, entry["tag"].name)
                    errors.append(simple_error(entry["tag"], message))
            continue
        if stack:
            current = stack[-1]
            for rule in current["rules"]:
                if name == rule.inner_tag:
                    current["seen_inner"].add(name)

    return errors


def _validate_block_structure(
    tags: list[TemplateTag],
    block_specs: list[BlockTagSpec] | None,
) -> list[ValidationError]:
    """
    Validate nesting/placement for delimiter tags derived from `parser.parse()`.

    This is intentionally conservative: it enforces basic stack discipline
    (delimiters must belong to the current open block) plus a small set of
    ordering constraints inferred from Django source:
    - non-repeatable delimiter tags cannot repeat within the same block
    - terminal delimiter tags (e.g. `else`, `empty`) forbid further delimiters
      until the closing end tag
    """
    if not block_specs:
        return []

    start_to_spec: dict[str, BlockTagSpec] = {}
    end_tags: set[str] = set()
    middle_tags: set[str] = set()
    for spec in block_specs:
        for start in spec.start_tags:
            start_to_spec[start] = spec
        end_tags.update(spec.end_tags)
        middle_tags.update(spec.middle_tags)

    # Implicit block tags:
    # Some real-world templates use block tags that are unknown to the current
    # Django version (e.g. deprecated built-ins like `ifequal`/`endifequal`, or
    # project-defined tags that follow the `end{tag}` convention). If we don't
    # treat them as blocks, generic delimiter tags like `{% else %}` can be
    # misattributed to an outer known block, causing false structural errors.
    #
    # We conservatively model any unknown tag with a matching `end{tag}` as an
    # "implicit block" whose middle tags are treated as repeatable. This avoids
    # false positives without claiming semantic correctness for the unknown tag.
    tag_names = {t.name for t in tags}
    implicit_middle = tuple(sorted(middle_tags))
    for name in sorted(tag_names):
        if name in start_to_spec:
            continue
        if name in end_tags or name in middle_tags:
            continue
        end_name = f"end{name}"
        if end_name not in tag_names:
            continue
        implicit_spec = BlockTagSpec(
            start_tags=(name,),
            end_tags=(end_name,),
            middle_tags=implicit_middle,
            repeatable_middle_tags=implicit_middle,
            terminal_middle_tags=(),
            end_suffix_from_start_index=None,
        )
        start_to_spec[name] = implicit_spec
        end_tags.add(end_name)

    errors: list[ValidationError] = []

    @dataclass
    class _BlockState:
        spec: BlockTagSpec
        start: TemplateTag
        seen: dict[str, int] = field(default_factory=dict)
        terminal_seen: bool = False

    stack: list[_BlockState] = []

    for tag in tags:
        spec = start_to_spec.get(tag.name)
        if spec is not None:
            stack.append(_BlockState(spec=spec, start=tag))
            continue

        if tag.name in middle_tags:
            if not stack:
                errors.append(
                    simple_error(tag, f"Unexpected '{tag.name}' outside any block")
                )
                continue
            entry = stack[-1]
            top_spec = entry.spec
            top_start = entry.start

            if tag.name not in top_spec.middle_tags:
                errors.append(
                    simple_error(
                        tag,
                        f"Unexpected '{tag.name}' inside '{top_start.name}' block",
                    )
                )
                continue

            if entry.terminal_seen:
                # A repeated terminal delimiter is best reported as a duplicate.
                if (
                    tag.name in set(top_spec.terminal_middle_tags)
                    and entry.seen.get(tag.name, 0) > 0
                ):
                    errors.append(
                        simple_error(
                            tag,
                            f"Duplicate '{tag.name}' inside '{top_start.name}' block",
                        )
                    )
                    continue
                errors.append(
                    simple_error(
                        tag,
                        f"Unexpected '{tag.name}' after terminal delimiter in '{top_start.name}' block",
                    )
                )
                continue

            count = entry.seen.get(tag.name, 0)

            is_repeatable = tag.name in set(top_spec.repeatable_middle_tags)
            if not is_repeatable and count > 0:
                errors.append(
                    simple_error(
                        tag,
                        f"Duplicate '{tag.name}' inside '{top_start.name}' block",
                    )
                )
                continue

            entry.seen[tag.name] = count + 1

            if tag.name in set(top_spec.terminal_middle_tags):
                entry.terminal_seen = True
            continue

        if tag.name in end_tags:
            if not stack:
                errors.append(
                    simple_error(tag, f"Unexpected '{tag.name}' outside any block")
                )
                continue
            entry = stack[-1]
            top_spec = entry.spec
            top_start = entry.start
            if tag.name in top_spec.end_tags:
                idx = top_spec.end_suffix_from_start_index
                if idx is not None:
                    # Tags like `{% block name %}` accept `{% endblock %}` or
                    # `{% endblock name %}` only. Extra tokens are invalid.
                    if len(tag.tokens) > 2:
                        errors.append(
                            simple_error(
                                tag,
                                f"End tag '{tag.name}' has too many arguments",
                            )
                        )
                        # Recover by closing the current block to avoid cascades.
                        stack.pop()
                        continue
                    if len(tag.tokens) == 2:
                        expected = (
                            top_start.tokens[idx]
                            if idx < len(top_start.tokens)
                            else None
                        )
                        actual = tag.tokens[1]
                        if expected != actual:
                            errors.append(
                                simple_error(
                                    tag,
                                    f"End tag '{tag.name}' suffix mismatch (expected '{expected}', got '{actual}')",
                                )
                            )
                            # Recover by closing the current block to avoid cascades.
                            stack.pop()
                            continue

                stack.pop()
                continue
            errors.append(
                simple_error(
                    tag,
                    f"Mismatched '{tag.name}' inside '{top_start.name}' block",
                )
            )
            continue

    for entry in reversed(stack):
        start_tag = entry.start
        errors.append(simple_error(start_tag, f"Unclosed '{start_tag.name}' block"))

    return errors
