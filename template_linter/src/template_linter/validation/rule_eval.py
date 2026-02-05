"""Rule and precondition evaluation.

Contains the token-view and expression evaluation machinery used by static
template-tag validation.

Kept dependency-light for portability.
"""

from __future__ import annotations

import ast
import re
from typing import Any

from ..overrides import TOKEN_LIST_VARS
from ..types import ExtractedRule
from ..types import Precondition
from ..types import RuleType
from ..types import TokenEnv
from ..types import TokenRef
from ..types import TokenView


def _has_token_kwargs_syntax(args: list[str]) -> bool:
    """
    Check if args contain token_kwargs syntax.

    token_kwargs accepts two syntaxes:
    - Modern: key=value (e.g., foo=bar)
    - Legacy: value as key (e.g., bar as foo) - with support_legacy=True

    Returns True if at least one valid kwarg pattern is found.
    """
    # Check for modern syntax: any token containing '='
    for arg in args:
        if "=" in arg:
            return True

    # Check for legacy syntax: 'value as key' pattern
    # Look for 'as' with tokens on both sides
    for i, arg in enumerate(args):
        if arg == "as" and i > 0 and i < len(args) - 1:
            return True

    return False


def _slice_indices(start: int | None, end: int | None, length: int) -> tuple[int, int]:
    """Normalize slice indices for a given length."""
    s = slice(start, end)
    norm_start, norm_end, step = s.indices(length)
    # Only support step = 1
    return norm_start, norm_end


def _apply_slice(
    view: TokenView, start: int | None, end: int | None, base_len: int
) -> TokenView:
    """Apply a slice to a view (relative to the view)."""
    if view.unknown:
        return TokenView(unknown=True)
    current_end = view.end if view.end is not None else base_len
    length = max(0, current_end - view.start)
    norm_start, norm_end = _slice_indices(start, end, length)
    return TokenView(start=view.start + norm_start, end=view.start + norm_end)


def _apply_pop(view: TokenView, index: int | None, base_len: int) -> TokenView:
    """Apply a pop() to a view."""
    if view.unknown:
        return TokenView(unknown=True)
    current_end = view.end if view.end is not None else base_len
    length = max(0, current_end - view.start)
    if length <= 0:
        return TokenView(unknown=True)

    if index is None or index == -1:
        # pop last
        return TokenView(start=view.start, end=current_end - 1)
    if index == 0:
        return TokenView(start=view.start + 1, end=current_end)

    # Removing from middle changes indices; mark unknown
    return TokenView(unknown=True)


def resolve_view(
    var_name: str,
    env: TokenEnv | None,
    tokens: list[str],
    view_override: dict[str, TokenView] | None = None,
) -> TokenView | None:
    """Resolve a variable's token view by applying conditional ops."""
    if view_override and var_name in view_override:
        return view_override[var_name]

    base_view = env.variables.get(var_name) if env else None
    if base_view is None:
        # Default only for primary token list
        if var_name in ("bits", "tokens"):
            base_view = TokenView()
        else:
            return None

    if base_view.unknown:
        return None

    current = TokenView(start=base_view.start, end=base_view.end)
    for op in base_view.ops:
        if op.guard is not None:
            guard_result = evaluate_precondition_expr(
                op.guard,
                tokens,
                env,
                view_override={var_name: current},
            )
            if guard_result is not True:
                continue

        if op.op == "slice":
            current = _apply_slice(current, op.start, op.end, len(tokens))
        elif op.op == "pop":
            current = _apply_pop(current, op.index, len(tokens))

        if current.unknown:
            return None

    return current


def resolve_ref(
    ref: TokenRef,
    tokens: list[str],
) -> str | None:
    """Resolve a TokenRef to an actual token string."""
    if ref.guard is not None:
        guard_env = TokenEnv(variables={ref.source: ref.view})
        guard_result = evaluate_precondition_expr(ref.guard, tokens, guard_env)
        if guard_result is not True:
            return None

    temp_env = TokenEnv(variables={ref.source: ref.view})
    view = resolve_view(ref.source, temp_env, tokens)
    if view is None:
        return None
    view_len = _view_length(view, len(tokens))
    idx = ref.index
    if idx < 0:
        idx += view_len
    if idx < 0 or idx >= view_len:
        return None
    base_index = view.start + idx
    if base_index < 0 or base_index >= len(tokens):
        return None
    return tokens[base_index]


def _view_length(view: TokenView, base_len: int) -> int:
    end = view.end if view.end is not None else base_len
    return max(0, end - view.start)


def _resolve_subscript_value(
    var_name: str,
    index: int,
    tokens: list[str],
    env: TokenEnv | None,
    view_override: dict[str, TokenView] | None = None,
) -> str | None:
    view = resolve_view(var_name, env, tokens, view_override=view_override)
    if view is None:
        return None
    view_len = _view_length(view, len(tokens))
    # Normalize index within view
    if index < 0:
        index += view_len
    if index < 0 or index >= view_len:
        return None
    base_index = view.start + index
    if base_index < 0 or base_index >= len(tokens):
        return None
    return tokens[base_index]


def evaluate_precondition_expr(
    expr: ast.AST,
    tokens: list[str],
    env: TokenEnv | None,
    view_override: dict[str, TokenView] | None = None,
) -> bool | None:
    """
    Evaluate a precondition AST against actual tokens.

    Returns:
    - True if precondition is satisfied
    - False if precondition is not satisfied
    - None if we can't evaluate (complex condition)
    """
    if isinstance(expr, ast.BoolOp):
        if isinstance(expr.op, ast.And):
            unknown = False
            for val in expr.values:
                res = evaluate_precondition_expr(
                    val, tokens, env, view_override=view_override
                )
                if res is False:
                    return False
                if res is None:
                    unknown = True
            return None if unknown else True
        elif isinstance(expr.op, ast.Or):
            unknown = False
            for val in expr.values:
                res = evaluate_precondition_expr(
                    val, tokens, env, view_override=view_override
                )
                if res is True:
                    return True
                if res is None:
                    unknown = True
            return None if unknown else False

    if isinstance(expr, ast.UnaryOp) and isinstance(expr.op, ast.Not):
        res = evaluate_precondition_expr(
            expr.operand, tokens, env, view_override=view_override
        )
        if res is None:
            return None
        return not res

    if isinstance(expr, ast.Compare):
        if not expr.ops or not expr.comparators:
            return None
        left_val = _eval_value(expr.left, tokens, env, view_override=view_override)
        if left_val is None:
            return None
        unknown = False
        for op, comp in zip(expr.ops, expr.comparators, strict=False):
            right_val = _eval_value(comp, tokens, env, view_override=view_override)
            if right_val is None:
                unknown = True
                break
            try:
                if isinstance(op, ast.Eq):
                    ok = left_val == right_val
                elif isinstance(op, ast.NotEq):
                    ok = left_val != right_val
                elif isinstance(op, ast.Lt):
                    ok = left_val < right_val
                elif isinstance(op, ast.LtE):
                    ok = left_val <= right_val
                elif isinstance(op, ast.Gt):
                    ok = left_val > right_val
                elif isinstance(op, ast.GtE):
                    ok = left_val >= right_val
                elif isinstance(op, ast.In):
                    ok = left_val in right_val
                elif isinstance(op, ast.NotIn):
                    ok = left_val not in right_val
                else:
                    return None
            except Exception:
                return None
            if not ok:
                return False
            left_val = right_val
        return None if unknown else True

    if isinstance(expr, ast.Constant):
        if isinstance(expr.value, bool):
            return expr.value

    if isinstance(expr, ast.Name):
        # Truthiness check on known token lists
        view = resolve_view(expr.id, env, tokens, view_override=view_override)
        if view is not None:
            return _view_length(view, len(tokens)) > 0
        if env and expr.id in env.values:
            value = resolve_ref(env.values[expr.id], tokens)
            if value is None:
                return None
            return bool(value)
        return None

    return None


def _eval_value(
    expr: ast.AST,
    tokens: list[str],
    env: TokenEnv | None,
    view_override: dict[str, TokenView] | None = None,
) -> Any | None:
    """Evaluate a simple value (constant, len(), subscript)."""
    if isinstance(expr, ast.Constant):
        return expr.value

    if isinstance(expr, (ast.Tuple, ast.List, ast.Set)):
        values = []
        for elt in expr.elts:
            val = _eval_value(elt, tokens, env, view_override=view_override)
            if val is None:
                return None
            values.append(val)
        return values

    # len(x)
    if isinstance(expr, ast.Call):
        if isinstance(expr.func, ast.Name) and expr.func.id == "len":
            if expr.args and isinstance(expr.args[0], ast.Name):
                view = resolve_view(
                    expr.args[0].id, env, tokens, view_override=view_override
                )
                if view is None:
                    return None
                return _view_length(view, len(tokens))

    # x[N]
    if isinstance(expr, ast.Subscript) and isinstance(expr.value, ast.Name):
        idx = None
        if isinstance(expr.slice, ast.Constant) and isinstance(expr.slice.value, int):
            idx = expr.slice.value
        elif isinstance(expr.slice, ast.UnaryOp) and isinstance(
            expr.slice.op, ast.USub
        ):
            val = _eval_value(
                expr.slice.operand, tokens, env, view_override=view_override
            )
            if isinstance(val, int):
                idx = -val
        if idx is not None:
            return _resolve_subscript_value(
                expr.value.id, idx, tokens, env, view_override=view_override
            )

    if isinstance(expr, ast.Name):
        if env and expr.id in env.values:
            return resolve_ref(env.values[expr.id], tokens)
        return None

    # name -> unknown (unless we can resolve directly)
    return None


def all_preconditions_met(
    preconditions: list[Precondition],
    tokens: list[str],
    env: TokenEnv | None,
) -> bool | None:
    """Check if all preconditions are met."""
    if not preconditions:
        return True

    unknown = False
    for prec in preconditions:
        result = evaluate_precondition_expr(prec.expr, tokens, env)
        if result is False:
            return False
        if result is None:
            unknown = True

    return None if unknown else True


def check_rule(
    rule: ExtractedRule, tokens: list[str], env: TokenEnv | None
) -> str | None:
    """
    Check if a rule is violated.

    Returns error message if violated, None if OK.

    Key insight: Rules describe when to RAISE an error.
    - `len(args) != 3` raises when length ISN'T 3
    - So we check: does the template MATCH the error condition?

    For compound OR rules:
    - `A or B` raises when EITHER A or B is true
    - So template is VALID only when BOTH A and B are false
    - We check each; if ANY matches the error condition, we report it
    """
    if rule.is_compound():
        return _check_compound_rule(rule, tokens, env)

    return _check_simple_rule(rule, tokens, env)


def _check_compound_rule(
    rule: ExtractedRule, tokens: list[str], env: TokenEnv | None
) -> str | None:
    """
    Check a compound rule (OR/AND of sub-rules).

    For OR: error triggers when ANY sub-rule matches
    For AND: error triggers when ALL sub-rules match
    """
    if rule.operator == "or":
        # OR: check each sub-rule, return first error
        for sub in rule.sub_rules:
            error = check_rule(sub, tokens, env)
            if error:
                return error
        return None

    elif rule.operator == "and":
        # AND: all must match for error to trigger
        errors = []
        for sub in rule.sub_rules:
            error = check_rule(sub, tokens, env)
            if error:
                errors.append(error)
            else:
                # One didn't match, so AND condition not met
                return None
        # All matched
        return errors[0] if errors else None

    return None


def _check_simple_rule(
    rule: ExtractedRule, tokens: list[str], env: TokenEnv | None
) -> str | None:
    """Check a simple (non-compound) rule."""
    values = rule.values
    inverted = values.get("inverted", False)

    if rule.rule_type == RuleType.EXACT_COUNT:
        expected = values.get("expected")
        var_name = values.get("variable")
        if expected is not None:
            view = resolve_view(var_name, env, tokens) if var_name else None
            if view is None and var_name and var_name not in TOKEN_LIST_VARS:
                return None
            actual = _view_length(view, len(tokens)) if view else len(tokens)
            # Rule says "len != N" (inverted), error when actual != expected
            if inverted and actual != expected:
                return f"Expected {expected} tokens, got {actual}"
            # Rule says "len == N", error when actual == expected (rare)
            elif not inverted and actual == expected:
                return f"Got exactly {expected} tokens (unexpected)"

    elif rule.rule_type == RuleType.MIN_COUNT:
        min_count = values.get("min")
        var_name = values.get("variable")
        if min_count is not None:
            view = resolve_view(var_name, env, tokens) if var_name else None
            if view is None and var_name and var_name not in TOKEN_LIST_VARS:
                return None
            actual = _view_length(view, len(tokens)) if view else len(tokens)
            if actual < min_count:
                return f"Expected at least {min_count} tokens, got {actual}"

    elif rule.rule_type == RuleType.MAX_COUNT:
        max_count = values.get("max")
        var_name = values.get("variable")
        if max_count is not None:
            view = resolve_view(var_name, env, tokens) if var_name else None
            if view is None and var_name and var_name not in TOKEN_LIST_VARS:
                return None
            actual = _view_length(view, len(tokens)) if view else len(tokens)
            if actual > max_count:
                return f"Expected at most {max_count} tokens, got {actual}"

    elif rule.rule_type == RuleType.KEYWORD_AT_POS:
        position = values.get("position")
        keyword = values.get("keyword")
        var_name = values.get("variable")

        if position is not None and keyword is not None:
            actual = None
            if var_name:
                actual = _resolve_subscript_value(var_name, position, tokens, env)
            else:
                try:
                    actual = tokens[position]
                except IndexError:
                    actual = None

            if actual is None:
                if inverted:
                    return f"Expected '{keyword}' at position {position}, but not enough tokens"
            else:
                # Inverted: "x[N] != keyword" - error when actual != keyword
                if inverted and actual != keyword:
                    return (
                        f"Expected '{keyword}' at position {position}, got '{actual}'"
                    )
                # Non-inverted: "x[N] == keyword" - error when actual == keyword
                elif not inverted and actual == keyword:
                    return f"Unexpected '{keyword}' at position {position}"

        elif position is None and keyword is not None:
            # Dynamic index case - position depends on other tokens
            # Pattern: `for` tag uses `in_index = -3 if is_reversed else -2`
            # where is_reversed = bits[-1] == "reversed"
            if keyword == "in" and inverted:
                # Check for 'in' at position -2, or -3 if 'reversed' is last
                is_reversed = len(tokens) > 0 and tokens[-1] == "reversed"
                check_pos = -3 if is_reversed else -2
                try:
                    actual = tokens[check_pos]
                    if actual != keyword:
                        return f"Expected '{keyword}' at position {check_pos}, got '{actual}'"
                except IndexError:
                    return f"Expected '{keyword}' but not enough tokens"

    elif rule.rule_type == RuleType.VALUE_NOT_IN_SET:
        allowed = values.get("allowed")
        variable = values.get("variable")

        if allowed and variable:
            actual: str | None = None

            # Prefer exact resolution via TokenRef when available.
            if env and variable in env.values:
                actual = resolve_ref(env.values[variable], tokens)

            # If variable refers to a token list, assume the first argument token.
            if actual is None and variable == "bits" and len(tokens) > 1:
                actual = tokens[1]

            # Otherwise, try to resolve variable as a single token if possible.
            if actual is None:
                # Heuristic: resolve index 1 on the named list variable.
                actual = _resolve_subscript_value(variable, 1, tokens, env)

            # Fallback: second token (common for simple one-arg tags).
            if actual is None and len(tokens) > 1:
                actual = tokens[1]

            if actual is None:
                return None

            # Special-case: membership in quote characters is almost always a
            # "argument must be quoted" check (e.g. app_name[0] in ('"', "'")).
            if isinstance(allowed, list) and set(allowed).issubset({'"', "'"}):
                quote = actual[0] if actual else ""
                if not quote or quote not in allowed:
                    return f"Expected a quoted string, got {actual!r}"
                if len(actual) < 2 or actual[-1] != quote:
                    return f"Expected a quoted string, got {actual!r}"
                return None

            # Common ergonomic behavior: if the template passes a quoted literal,
            # compare against the unquoted content.
            normalized = actual
            if normalized.startswith(('"', "'")) and normalized.endswith(('"', "'")):
                normalized = normalized[1:-1]

            if normalized not in allowed:
                return f"Invalid value '{normalized}', expected one of: {allowed}"

    elif rule.rule_type == RuleType.VALUE_IN_SET:
        allowed = values.get("allowed")
        variable = values.get("variable")

        if allowed and variable:
            actual: str | None = None

            if env and variable in env.values:
                actual = resolve_ref(env.values[variable], tokens)

            if actual is None and variable == "bits" and len(tokens) > 1:
                actual = tokens[1]

            if actual is None:
                actual = _resolve_subscript_value(variable, 1, tokens, env)

            if actual is None and len(tokens) > 1:
                actual = tokens[1]

            if actual is None:
                return None

            normalized = actual
            if normalized.startswith(('"', "'")) and normalized.endswith(('"', "'")):
                normalized = normalized[1:-1]

            if normalized in allowed:
                return (
                    f"Unexpected value '{normalized}', expected not one of: {allowed}"
                )
        return None

    elif rule.rule_type == RuleType.PARSER_STATE:
        # Can't validate statically
        return None

    elif rule.rule_type == RuleType.BOOLEAN_CHECK:
        # Pattern: `if not bits:` means "require at least one argument"
        # bits = token.split_contents()[1:] (all tokens except tag name)
        # In our model: tokens[0] is tag name, so need len(tokens) > 1
        var = values.get("variable")
        inverted = values.get("inverted", False)

        # Known arg-list variables
        if var in ("bits", "args"):
            view = resolve_view(var, env, tokens)
            if view is None:
                return None
            actual_len = _view_length(view, len(tokens))
            if inverted:
                # `if not bits:` - error when no args
                if actual_len == 0:
                    return "Expected at least one argument"
            else:
                # `if bits:` - error when there ARE args (rare)
                if actual_len > 0:
                    return "Unexpected arguments"

        # remaining_bits after token_kwargs - can't validate without simulating
        # token_kwargs consumption, so skip this check
        elif var == "remaining_bits":
            return None

        # token_kwargs return value pattern (spike 14)
        # `extra_context = token_kwargs(...)` followed by `if not extra_context:`
        # token_kwargs accepts: key=value OR value as key (with support_legacy)
        elif var == "extra_context" and inverted:
            args = tokens[1:]  # Everything after tag name
            has_kwarg = _has_token_kwargs_syntax(args)
            if not has_kwarg:
                return "Expected at least one variable assignment (key=value)"

        # Other variables need context we don't have
        return None

    elif rule.rule_type == RuleType.REGEX_MATCH:
        list_var = values.get("list")
        pattern = values.get("pattern")
        if list_var and pattern:
            view = resolve_view(list_var, env, tokens)
            if view is None:
                return None
            start = view.start
            end = view.end if view.end is not None else len(tokens)
            regex = None
            if pattern == "kwarg_re":
                regex = re.compile(r"(?:(\w+)=)?(.+)")
            if regex is None:
                return None
            for tok in tokens[start:end]:
                if not regex.match(tok):
                    return rule.message_template or "Malformed arguments"

    elif rule.rule_type == RuleType.METHOD_CHECK:
        var_name = values.get("variable")
        position = values.get("position")
        method = values.get("method")
        if var_name is not None and position is not None and method:
            actual = _resolve_subscript_value(var_name, position, tokens, env)
            if actual is None:
                return None
            method_fn = getattr(actual, method, None)
            if method_fn is None or not callable(method_fn):
                return None
            result = method_fn()
            if inverted and not result:
                return f"Invalid value '{actual}'"
            if not inverted and result:
                return f"Invalid value '{actual}'"

    return None
