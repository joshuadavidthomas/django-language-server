"""
Shared types for extraction and validation.

Consolidates definitions from spikes 03-07, 10.
"""

from __future__ import annotations

import ast
import copy
from dataclasses import dataclass
from dataclasses import field
from enum import Enum
from enum import auto
from typing import Any


class RuleType(Enum):
    """Classification of validation rule types."""

    EXACT_COUNT = auto()  # len(x) == N
    MIN_COUNT = auto()  # len(x) >= N
    MAX_COUNT = auto()  # len(x) <= N
    KEYWORD_AT_POS = auto()  # x[N] == "keyword"
    VALUE_IN_SET = auto()  # x in (a, b, c)
    VALUE_NOT_IN_SET = auto()  # x not in (a, b, c)
    BOOLEAN_CHECK = auto()  # if x: / if not x:
    REGEX_MATCH = auto()  # kwarg_re.match(bit)
    METHOD_CHECK = auto()  # tokens[1].isdigit()
    PARSER_STATE = auto()  # parser.something (can't validate statically)
    EXCEPTION_HANDLER = auto()  # try/except pattern
    MATCH_CASE = auto()  # match/case pattern
    COMPARISON = auto()  # other comparisons
    COMPOUND = auto()  # OR/AND of multiple rules
    UNKNOWN = auto()


@dataclass(frozen=True)
class ConditionalOp:
    """A list-operation applied conditionally to a token view."""

    guard: ast.AST | None  # None means unconditional
    op: str  # "slice" or "pop"
    start: int | None = None
    end: int | None = None
    index: int | None = None


@dataclass
class TokenView:
    """
    A view into the base token list (tag tokens).

    start/end are indices into the base token list. end is exclusive.
    ops are applied in order at validation time.
    """

    start: int = 0
    end: int | None = None
    unknown: bool = False
    ops: list[ConditionalOp] = field(default_factory=list)

    def copy(self) -> TokenView:
        return copy.deepcopy(self)


@dataclass
class TokenEnv:
    """Holds token list views for variables (bits, args, remaining_bits, etc.)."""

    variables: dict[str, TokenView] = field(default_factory=dict)
    values: dict[str, TokenRef] = field(default_factory=dict)
    regex_matches: dict[str, RegexMatch] = field(default_factory=dict)

    def copy(self) -> TokenEnv:
        return TokenEnv(
            variables=copy.deepcopy(self.variables),
            values=copy.deepcopy(self.values),
            regex_matches=copy.deepcopy(self.regex_matches),
        )


@dataclass
class Precondition:
    """A precondition expression and its source text."""

    expr: ast.AST
    source: str = ""


@dataclass
class TokenRef:
    """Reference to a specific token within a list view."""

    source: str
    view: TokenView
    index: int
    guard: ast.AST | None = None


@dataclass
class RegexMatch:
    """Reference to a regex match against items in a token list."""

    list_var: str
    pattern: str


@dataclass
class ParseBitsSpec:
    """Signature-based validation for simple_tag/inclusion_tag."""

    params: list[str]
    required_params: list[str]
    kwonly: list[str]
    required_kwonly: list[str]
    varargs: bool
    varkw: bool
    allow_as_var: bool = True


@dataclass
class OpaqueBlockSpec:
    """Spec for tags that skip parsing of their inner content."""

    end_tags: list[str]
    match_suffix: bool = False
    kind: str = ""


@dataclass(frozen=True)
class ConditionalInnerTagRule:
    """
    A structural rule enforced by scanning block tags in a template.

    This is used for cases where Django enforces constraints on *inner* block
    tags (not just the opening tag syntax). Example (Django i18n):
    - In `{% blocktranslate count ... %}`, `{% plural %}` is required.
    - In `{% blocktranslate %}`, `{% plural %}` is forbidden.
    """

    start_tags: tuple[str, ...]
    end_tags: tuple[str, ...]
    inner_tag: str
    option_token: str
    require_when_option_present: bool
    message: str = ""


@dataclass(frozen=True)
class BlockTagSpec:
    """
    Structural spec for block tags that use `parser.parse((...))`.

    These tags introduce unregistered "delimiter" tags in templates:
    - end tags like `endif`, `endfor`, `endblock`, ...
    - middle tags like `else`, `elif`, `empty`, ...

    We extract these stop tokens statically from Django source so we can:
    - avoid false "Unknown tag" errors for delimiter tags
    - validate nesting and basic placement without running Django templates
    """

    start_tags: tuple[str, ...]
    end_tags: tuple[str, ...]
    middle_tags: tuple[str, ...] = ()
    repeatable_middle_tags: tuple[str, ...] = ()
    terminal_middle_tags: tuple[str, ...] = ()
    end_suffix_from_start_index: int | None = None


@dataclass
class ExtractedRule:
    """
    A single extracted validation rule with semantic values.

    From spike 04's value extraction. Handles compound rules correctly
    (unlike the simplified extractors in spikes 05-06 that only took
    the first part of OR conditions).
    """

    rule_type: RuleType
    values: dict[str, Any] = field(default_factory=dict)
    condition_source: str = ""
    message_template: str | None = None

    # For compound rules (OR/AND)
    operator: str | None = None  # "or" or "and"
    sub_rules: list[ExtractedRule] = field(default_factory=list)

    def is_compound(self) -> bool:
        return self.rule_type == RuleType.COMPOUND and bool(self.sub_rules)


@dataclass
class ContextualRule:
    """
    A rule with preconditions from enclosing if statements.

    From spike 06's precondition tracking. Preconditions are conditions
    that must be true for this rule to apply.

    Example: For `{% cycle 'a' 'b' 'c' as name verbose %}`, the rule
    checking for 'silent' has precondition `args[-3] == "as"`.
    """

    rule: ExtractedRule
    preconditions: list[Precondition] = field(default_factory=list)
    env: TokenEnv | None = None


@dataclass
class TagValidation:
    """All validation rules for a single template tag."""

    tag_name: str
    file_path: str = ""
    rules: list[ContextualRule] = field(default_factory=list)

    # Option-based validation (from spike 10)
    valid_options: list[str] = field(default_factory=list)
    option_constraints: dict[str, dict[str, Any]] = field(default_factory=dict)
    no_duplicate_options: bool = False
    rejects_unknown_options: bool = False
    option_loop_var: str | None = None
    option_loop_env: TokenEnv | None = None
    parse_bits_spec: ParseBitsSpec | None = None
    # Tags that ignore token args entirely (no syntax constraints).
    unrestricted: bool = False


@dataclass
class TemplateTag:
    """A parsed template tag for validation."""

    name: str
    tokens: list[str]  # All tokens including tag name
    raw: str = ""  # Original tag string
    line: int = 0  # Line number in template

    @property
    def args(self) -> list[str]:
        """Arguments (tokens without tag name)."""
        return self.tokens[1:] if len(self.tokens) > 1 else []

    @property
    def token_count(self) -> int:
        """Total token count including tag name."""
        return len(self.tokens)


@dataclass
class ValidationError:
    """A validation error found in a template."""

    tag: TemplateTag
    rule: ContextualRule
    message: str

    def __str__(self) -> str:
        loc = f"line {self.tag.line}" if self.tag.line else "unknown location"
        return f"{self.tag.name} ({loc}): {self.message}"


# ---------------------------------------------------------------------------
# Opaque block utilities
# ---------------------------------------------------------------------------


def merge_opaque_blocks(
    base: dict[str, OpaqueBlockSpec],
    extra: dict[str, OpaqueBlockSpec],
) -> dict[str, OpaqueBlockSpec]:
    """
    Merge opaque-block specs.

    Semantics:
    - `end_tags` are unioned and sorted
    - `match_suffix` / `kind` are combined via logical-or / first-truthy
    """
    merged = dict(base)
    for name, spec in extra.items():
        existing = merged.get(name)
        if existing is None:
            merged[name] = spec
            continue
        merged[name] = OpaqueBlockSpec(
            end_tags=sorted(set(existing.end_tags + spec.end_tags)),
            match_suffix=existing.match_suffix or spec.match_suffix,
            kind=existing.kind or spec.kind,
        )
    return merged


def resolve_opaque_blocks(
    opaque_blocks: dict[str, OpaqueBlockSpec] | None,
    *,
    defaults: dict[str, OpaqueBlockSpec] | None = None,
) -> dict[str, OpaqueBlockSpec]:
    """
    Normalize an optional opaque-block mapping with optional defaults.

    This is a convenience wrapper around `merge_opaque_blocks()` used by
    tokenization and validation.
    """
    base = dict(defaults or {})
    if not opaque_blocks:
        return base
    return merge_opaque_blocks(base, opaque_blocks)


# ---------------------------------------------------------------------------
# Diagnostic helpers
# ---------------------------------------------------------------------------


def simple_error(tag: TemplateTag, message: str) -> ValidationError:
    """
    Create a generic validation error without a specific extracted rule.
    """
    return ValidationError(
        tag=tag,
        rule=ContextualRule(rule=ExtractedRule(rule_type=RuleType.UNKNOWN)),
        message=message,
    )
