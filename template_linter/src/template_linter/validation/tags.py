"""Tag-level validation.

Validates a single TemplateTag against extracted TagValidation rules.

Template-wide orchestration lives in `validation.py`.
"""

from __future__ import annotations

import ast
import re

from ..types import ContextualRule
from ..types import ExtractedRule
from ..types import ParseBitsSpec
from ..types import RuleType
from ..types import TagValidation
from ..types import TemplateTag
from ..types import ValidationError
from ..types import simple_error as _simple_error
from .rule_eval import all_preconditions_met
from .rule_eval import check_rule
from .rule_eval import resolve_view


def _consume_token_kwargs(
    tokens: list[str],
    start: int,
    *,
    support_legacy: bool = False,
) -> tuple[int, int]:
    """
    Consume token_kwargs-style arguments.

    Returns (count, next_index).
    Supports `key=value` and legacy `value as key`.
    """
    if start >= len(tokens):
        return 0, start

    # Mirror Django's `token_kwargs()` behavior:
    # - If the first token is `key=value`, only consume `key=value` pairs.
    # - If legacy is enabled and the first token looks like `value as key`,
    #   consume `value as key` pairs, allowing `and` separators.
    i = start
    count = 0

    kwarg_format = "=" in tokens[i]
    if not kwarg_format:
        if not support_legacy:
            return 0, start
        if i + 2 >= len(tokens) or tokens[i + 1] != "as":
            return 0, start

    while i < len(tokens):
        if kwarg_format:
            if "=" not in tokens[i]:
                break
            count += 1
            i += 1
            continue

        # legacy format: value as key
        if i + 2 >= len(tokens) or tokens[i + 1] != "as":
            break
        count += 1
        i += 3
        if i < len(tokens):
            if tokens[i] != "and":
                break
            i += 1

    return count, i


def validate_tag(tag: TemplateTag, validation: TagValidation) -> list[ValidationError]:
    """
    Validate a template tag against its extracted rules.

    Respects preconditions: rules only apply when preconditions are met.
    """
    errors = []
    tokens = tag.tokens
    all_preconditions_failed = True
    min_tokens_from_preconditions = 0

    for ctx_rule in validation.rules:
        env = ctx_rule.env
        # Check preconditions
        prec_result = all_preconditions_met(ctx_rule.preconditions, tokens, env)
        if prec_result is False:
            # Track minimum token count from failed preconditions (spike 15)
            # Pattern: `len(bits) == N` precondition implies min N tokens
            for prec in ctx_rule.preconditions:
                expr = prec.expr
                if (
                    isinstance(expr, ast.Compare)
                    and expr.ops
                    and isinstance(expr.ops[0], ast.Eq)
                ):
                    if isinstance(expr.left, ast.Call) and isinstance(
                        expr.left.func, ast.Name
                    ):
                        if (
                            expr.left.func.id == "len"
                            and expr.left.args
                            and isinstance(expr.left.args[0], ast.Name)
                        ):
                            var_name = expr.left.args[0].id
                            if expr.comparators and isinstance(
                                expr.comparators[0], ast.Constant
                            ):
                                expected = expr.comparators[0].value
                                if isinstance(expected, int):
                                    view = resolve_view(var_name, env, tokens)
                                    if view is not None:
                                        min_tokens_from_preconditions = max(
                                            min_tokens_from_preconditions,
                                            expected + view.start,
                                        )
            continue  # Precondition not met, rule doesn't apply

        if prec_result is None:
            # Unknown preconditions - skip rule to avoid false positives
            continue

        all_preconditions_failed = False

        # Check the rule
        error_msg = check_rule(ctx_rule.rule, tokens, env)
        if error_msg:
            errors.append(
                ValidationError(
                    tag=tag,
                    rule=ctx_rule,
                    message=error_msg,
                )
            )

    # Option loop validation (include, some i18n tags, etc.)
    option_errors = _validate_options(tag, validation)
    errors.extend(option_errors)

    # parse_bits validation for simple_tag/inclusion_tag
    parse_bits_errors = _validate_parse_bits(tag, validation.parse_bits_spec)
    errors.extend(parse_bits_errors)

    # Spike 15: If all rules had failed preconditions and we have very few tokens,
    # infer a "not enough arguments" error (handles widthratio else clause)
    # Only trigger for clearly insufficient args (1-2 tokens) to avoid false positives
    # when there are multiple valid token counts (e.g., widthratio accepts 4 OR 6)
    if (
        all_preconditions_failed
        and validation.rules
        and min_tokens_from_preconditions > 0
    ):
        if len(tokens) <= 2 and len(tokens) < min_tokens_from_preconditions:
            # Create a synthetic rule for the error
            synthetic_rule = ContextualRule(
                rule=ExtractedRule(
                    rule_type=RuleType.MIN_COUNT,
                    values={"min": min_tokens_from_preconditions},
                    condition_source="(inferred from preconditions)",
                )
            )
            errors.append(
                ValidationError(
                    tag=tag,
                    rule=synthetic_rule,
                    message=f"Expected at least {min_tokens_from_preconditions} tokens, got {len(tokens)}",
                )
            )

    return errors


def _validate_parse_bits(
    tag: TemplateTag, spec: ParseBitsSpec | None
) -> list[ValidationError]:
    if spec is None:
        return []

    bits = tag.tokens[1:]
    if spec.allow_as_var and len(bits) >= 2 and bits[-2] == "as":
        bits = bits[:-2]

    errors: list[ValidationError] = []
    args: list[str] = []
    kwargs: dict[str, str] = {}
    saw_kwarg = False

    for bit in bits:
        key = _kwarg_key(bit)
        if key:
            # unexpected keyword
            if not spec.varkw and key not in spec.params and key not in spec.kwonly:
                errors.append(
                    _simple_error(
                        tag,
                        f"'{tag.name}' received unexpected keyword argument '{key}'",
                    )
                )
                continue

            # duplicate
            if key in kwargs:
                errors.append(
                    _simple_error(
                        tag,
                        f"'{tag.name}' received multiple values for keyword argument '{key}'",
                    )
                )
                continue

            # keyword provided for positional already filled
            if key in spec.params:
                idx = spec.params.index(key)
                if idx < len(args):
                    errors.append(
                        _simple_error(
                            tag,
                            f"'{tag.name}' received multiple values for keyword argument '{key}'",
                        )
                    )
                    continue

            kwargs[key] = bit
            saw_kwarg = True
        else:
            if saw_kwarg:
                errors.append(
                    _simple_error(
                        tag,
                        f"'{tag.name}' received some positional argument(s) after some keyword argument(s)",
                    )
                )
            args.append(bit)

    if not spec.varargs and len(args) > len(spec.params):
        errors.append(
            _simple_error(tag, f"'{tag.name}' received too many positional arguments")
        )

    provided_pos = set(spec.params[: min(len(args), len(spec.params))])
    missing = [
        p for p in spec.required_params if p not in provided_pos and p not in kwargs
    ]
    if missing:
        errors.append(
            _simple_error(
                tag,
                f"'{tag.name}' did not receive value(s) for the argument(s): {', '.join(missing)}",
            )
        )

    missing_kwonly = [k for k in spec.required_kwonly if k not in kwargs]
    if missing_kwonly:
        errors.append(
            _simple_error(
                tag,
                f"'{tag.name}' did not receive value(s) for the argument(s): {', '.join(missing_kwonly)}",
            )
        )

    return errors


def _kwarg_key(bit: str) -> str | None:
    m = re.match(r"^(\w+)=", bit)
    if m:
        return m.group(1)
    return None


def _validate_options(
    tag: TemplateTag, validation: TagValidation
) -> list[ValidationError]:
    if not validation.valid_options:
        return []
    if not validation.option_loop_var or not validation.option_loop_env:
        return []

    view = resolve_view(
        validation.option_loop_var, validation.option_loop_env, tag.tokens
    )
    if view is None:
        return []

    tokens = tag.tokens
    end = view.end if view.end is not None else len(tokens)
    option_tokens = tokens[view.start : end]
    if not option_tokens:
        return []

    errors: list[ValidationError] = []
    seen: set[str] = set()
    i = 0

    while i < len(option_tokens):
        option = option_tokens[i]

        if validation.no_duplicate_options and option in seen:
            errors.append(
                ValidationError(
                    tag=tag,
                    rule=ContextualRule(rule=ExtractedRule(rule_type=RuleType.UNKNOWN)),
                    message=f"Duplicate option '{option}'",
                )
            )
            i += 1
            continue
        seen.add(option)

        if option not in validation.valid_options:
            if validation.rejects_unknown_options:
                errors.append(
                    ValidationError(
                        tag=tag,
                        rule=ContextualRule(
                            rule=ExtractedRule(rule_type=RuleType.UNKNOWN)
                        ),
                        message=f"Unknown option '{option}'",
                    )
                )
            i += 1
            continue

        spec = validation.option_constraints.get(option, {})
        opt_type = spec.get("type", "boolean")

        if opt_type == "boolean":
            i += 1
            continue

        if opt_type == "single_arg":
            if i + 1 >= len(option_tokens):
                errors.append(
                    ValidationError(
                        tag=tag,
                        rule=ContextualRule(
                            rule=ExtractedRule(rule_type=RuleType.UNKNOWN)
                        ),
                        message=f"Option '{option}' requires an argument",
                    )
                )
                break
            arg_value = option_tokens[i + 1]
            disallow = spec.get("arg_disallow")
            allow = spec.get("arg_allow")
            if disallow and arg_value in disallow:
                errors.append(
                    ValidationError(
                        tag=tag,
                        rule=ContextualRule(
                            rule=ExtractedRule(rule_type=RuleType.UNKNOWN)
                        ),
                        message=f"Option '{option}' received invalid argument '{arg_value}'",
                    )
                )
            if allow and arg_value not in allow:
                errors.append(
                    ValidationError(
                        tag=tag,
                        rule=ContextualRule(
                            rule=ExtractedRule(rule_type=RuleType.UNKNOWN)
                        ),
                        message=f"Option '{option}' expects one of {sorted(allow)}",
                    )
                )
            i += 2
            continue

        if opt_type == "kwargs":
            count, next_i = _consume_token_kwargs(
                option_tokens,
                i + 1,
                support_legacy=bool(spec.get("support_legacy")),
            )
            min_kwargs = spec.get("min_kwargs")
            exact_count = spec.get("exact_count")

            if min_kwargs is not None and count < min_kwargs:
                errors.append(
                    ValidationError(
                        tag=tag,
                        rule=ContextualRule(
                            rule=ExtractedRule(rule_type=RuleType.UNKNOWN)
                        ),
                        message=f"Option '{option}' expects at least {min_kwargs} keyword argument(s)",
                    )
                )
            if exact_count is not None and count != exact_count:
                errors.append(
                    ValidationError(
                        tag=tag,
                        rule=ContextualRule(
                            rule=ExtractedRule(rule_type=RuleType.UNKNOWN)
                        ),
                        message=f"Option '{option}' expects exactly {exact_count} keyword argument(s)",
                    )
                )

            # If no kwargs parsed, avoid infinite loop
            if next_i == i + 1:
                next_i = i + 1
            i = next_i
            continue

        # Unknown option type; advance
        i += 1

    return errors
