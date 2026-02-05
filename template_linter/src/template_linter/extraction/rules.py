"""Rule extraction for template tags.

This module contains the core AST-based rule extraction logic.

Kept separate from file-system traversal and opaque extraction to keep module
boundaries clear for porting.
"""

from __future__ import annotations

import ast
from typing import Any

from ..types import ConditionalOp
from ..types import ContextualRule
from ..types import ExtractedRule
from ..types import ParseBitsSpec
from ..types import Precondition
from ..types import RegexMatch
from ..types import RuleType
from ..types import TagValidation
from ..types import TokenEnv
from ..types import TokenRef
from ..types import TokenView
from .registry import TAG_DECORATORS


def extract_constant(node: ast.AST) -> Any:
    """Extract a constant value from an AST node."""
    if isinstance(node, ast.Constant):
        return node.value
    if isinstance(node, ast.Tuple):
        return tuple(extract_constant(elt) for elt in node.elts)
    if isinstance(node, ast.List):
        return [extract_constant(elt) for elt in node.elts]
    if isinstance(node, ast.Set):
        return {extract_constant(elt) for elt in node.elts}
    if isinstance(node, ast.UnaryOp) and isinstance(node.op, ast.USub):
        val = extract_constant(node.operand)
        return -val if val is not None else None
    return None


def extract_subscript_index(node: ast.Subscript) -> int | None:
    """Extract index from a subscript like bits[2] or bits[-1]."""
    if isinstance(node.slice, ast.Constant) and isinstance(node.slice.value, int):
        return node.slice.value
    if isinstance(node.slice, ast.UnaryOp) and isinstance(node.slice.op, ast.USub):
        val = extract_constant(node.slice.operand)
        return -val if isinstance(val, int) else None
    # Variable index (like bits[in_index]) - can't extract statically
    return None


def extract_variable_name(node: ast.AST) -> str | None:
    """Extract variable name from Name or Subscript."""
    if isinstance(node, ast.Name):
        return node.id
    if isinstance(node, ast.Subscript) and isinstance(node.value, ast.Name):
        return node.value.id
    return None


def is_len_call(node: ast.AST) -> tuple[bool, str | None]:
    """Check if node is len(x) and return the variable name."""
    if isinstance(node, ast.Call):
        if isinstance(node.func, ast.Name) and node.func.id == "len":
            if node.args and isinstance(node.args[0], ast.Name):
                return True, node.args[0].id
    return False, None


# =============================================================================
# Rule Extraction
# =============================================================================


def extract_rule(condition: ast.AST, source: str) -> ExtractedRule:
    """
    Extract a validation rule from a condition AST.

    This is the key function. It handles compound OR/AND conditions by
    extracting ALL sub-rules, unlike the simplified versions in spikes
    05-06 that only took the first part.

    For validation, compound OR rules are inverted to AND:
    - `len(args) != 3 or args[1] != "as"` raises when EITHER fails
    - So validity requires BOTH: `len(args) == 3 AND args[1] == "as"`
    """
    condition_source = ast.get_source_segment(source, condition) or ""

    # Handle compound conditions (OR/AND)
    if isinstance(condition, ast.BoolOp):
        operator = "or" if isinstance(condition.op, ast.Or) else "and"
        sub_rules = [extract_rule(val, source) for val in condition.values]
        return ExtractedRule(
            rule_type=RuleType.COMPOUND,
            operator=operator,
            sub_rules=sub_rules,
            condition_source=condition_source,
        )

    # Handle comparisons
    if isinstance(condition, ast.Compare):
        return _extract_comparison_rule(condition, source, condition_source)

    # Handle unary not (with range special-case)
    if isinstance(condition, ast.UnaryOp) and isinstance(condition.op, ast.Not):
        range_rule = _extract_len_range_rule(
            condition.operand, source, condition_source
        )
        if range_rule is not None:
            return range_rule
        inner = extract_rule(condition.operand, source)
        return _negate_rule(inner)

    # Handle attribute access (parser.something)
    if isinstance(condition, ast.Attribute):
        return ExtractedRule(
            rule_type=RuleType.PARSER_STATE,
            values={"attribute": condition.attr},
            condition_source=condition_source,
        )

    # Handle bare name (boolean check)
    if isinstance(condition, ast.Name):
        return ExtractedRule(
            rule_type=RuleType.BOOLEAN_CHECK,
            values={"variable": condition.id},
            condition_source=condition_source,
        )

    # Handle method calls
    if isinstance(condition, ast.Call):
        method_rule = _extract_method_check_rule(condition, source, condition_source)
        if method_rule is not None:
            return method_rule
        return ExtractedRule(
            rule_type=RuleType.UNKNOWN,
            values={"call": ast.get_source_segment(source, condition)},
            condition_source=condition_source,
        )

    # Fallback
    return ExtractedRule(
        rule_type=RuleType.UNKNOWN,
        condition_source=condition_source,
    )


def _extract_comparison_rule(
    condition: ast.Compare, source: str, condition_source: str
) -> ExtractedRule:
    """Extract rule from a comparison expression."""
    left = condition.left
    ops = condition.ops
    comparators = condition.comparators

    if not ops or not comparators:
        return ExtractedRule(
            rule_type=RuleType.UNKNOWN, condition_source=condition_source
        )

    op = ops[0]
    comparator = comparators[0]

    # Range comparisons (e.g., 3 <= len(bits) <= 6)
    range_rule = _extract_len_range_rule(condition, source, condition_source)
    if range_rule is not None:
        return range_rule

    # len(x) comparisons
    is_len, var_name = is_len_call(left)
    if is_len and var_name:
        expected = extract_constant(comparator)
        values: dict[str, Any] = {"variable": var_name}

        if isinstance(op, (ast.Eq, ast.NotEq)):
            if expected is not None:
                values["expected"] = expected
            if isinstance(op, ast.NotEq):
                values["inverted"] = True
            return ExtractedRule(
                rule_type=RuleType.EXACT_COUNT,
                values=values,
                condition_source=condition_source,
            )
        elif isinstance(op, ast.Lt):
            if expected is not None:
                values["min"] = expected
            return ExtractedRule(
                rule_type=RuleType.MIN_COUNT,
                values=values,
                condition_source=condition_source,
            )
        elif isinstance(op, ast.LtE):
            if expected is not None:
                values["min"] = expected + 1 if expected else None
            return ExtractedRule(
                rule_type=RuleType.MIN_COUNT,
                values=values,
                condition_source=condition_source,
            )
        elif isinstance(op, ast.Gt):
            if expected is not None:
                values["max"] = expected
            return ExtractedRule(
                rule_type=RuleType.MAX_COUNT,
                values=values,
                condition_source=condition_source,
            )
        elif isinstance(op, ast.GtE):
            if expected is not None:
                values["max"] = expected - 1 if expected else None
            return ExtractedRule(
                rule_type=RuleType.MAX_COUNT,
                values=values,
                condition_source=condition_source,
            )

    # Membership tests: x in (a, b, c) or x not in (a, b, c)
    if isinstance(op, (ast.In, ast.NotIn)):
        var_name = extract_variable_name(left)
        allowed = extract_constant(comparator)

        values = {}
        if var_name:
            values["variable"] = var_name
        if allowed is not None:
            if isinstance(allowed, (tuple, list, set)):
                values["allowed"] = list(allowed)
            else:
                values["allowed"] = allowed

        rule_type = (
            RuleType.VALUE_NOT_IN_SET
            if isinstance(op, ast.NotIn)
            else RuleType.VALUE_IN_SET
        )
        return ExtractedRule(
            rule_type=rule_type, values=values, condition_source=condition_source
        )

    # Subscript comparisons: bits[N] == "keyword"
    if isinstance(left, ast.Subscript):
        var_name = extract_variable_name(left)
        position = extract_subscript_index(left)
        keyword = extract_constant(comparator)

        values = {"variable": var_name}
        if position is not None:
            values["position"] = position
        if keyword is not None:
            values["keyword"] = keyword
        if isinstance(op, ast.NotEq):
            values["inverted"] = True

        return ExtractedRule(
            rule_type=RuleType.KEYWORD_AT_POS,
            values=values,
            condition_source=condition_source,
        )

    # Generic comparison
    return ExtractedRule(
        rule_type=RuleType.COMPARISON,
        values={
            "left": ast.get_source_segment(source, left),
            "op": type(op).__name__,
            "right": ast.get_source_segment(source, comparator),
        },
        condition_source=condition_source,
    )


def _extract_len_range_rule(
    condition: ast.AST,
    source: str,
    condition_source: str,
) -> ExtractedRule | None:
    """Handle range checks on len(x), including negated ranges."""
    if not isinstance(condition, ast.Compare):
        return None
    if len(condition.ops) != 2 or len(condition.comparators) != 2:
        return None

    op1, op2 = condition.ops
    left = condition.left
    mid = condition.comparators[0]
    right = condition.comparators[1]

    # Match: CONST <= len(var) <= CONST (or <)
    is_len, var_name = is_len_call(mid)
    if not is_len or not var_name:
        return None
    if not isinstance(left, ast.Constant) or not isinstance(right, ast.Constant):
        return None
    if not isinstance(left.value, int) or not isinstance(right.value, int):
        return None
    if not isinstance(op1, (ast.Lt, ast.LtE)) or not isinstance(op2, (ast.Lt, ast.LtE)):
        return None

    lower = left.value
    upper = right.value
    inclusive_lower = isinstance(op1, ast.LtE)
    inclusive_upper = isinstance(op2, ast.LtE)

    # Convert to error conditions: len < min OR len > max
    min_val = lower if inclusive_lower else lower + 1
    max_val = upper if inclusive_upper else upper - 1

    min_rule = ExtractedRule(
        rule_type=RuleType.MIN_COUNT,
        values={"variable": var_name, "min": min_val},
        condition_source=condition_source,
    )
    max_rule = ExtractedRule(
        rule_type=RuleType.MAX_COUNT,
        values={"variable": var_name, "max": max_val},
        condition_source=condition_source,
    )
    return ExtractedRule(
        rule_type=RuleType.COMPOUND,
        operator="or",
        sub_rules=[min_rule, max_rule],
        condition_source=condition_source,
    )


def _extract_method_check_rule(
    condition: ast.Call,
    source: str,
    condition_source: str,
) -> ExtractedRule | None:
    """Extract method checks like tokens[1].isdigit()."""
    if isinstance(condition.func, ast.Attribute):
        method = condition.func.attr
        if method in ("isdigit", "isidentifier"):
            value = condition.func.value
            if isinstance(value, ast.Subscript):
                var_name = extract_variable_name(value)
                position = extract_subscript_index(value)
                if var_name and position is not None:
                    return ExtractedRule(
                        rule_type=RuleType.METHOD_CHECK,
                        values={
                            "variable": var_name,
                            "position": position,
                            "method": method,
                        },
                        condition_source=condition_source,
                    )
    return None


def _negate_rule(rule: ExtractedRule) -> ExtractedRule:
    """Negate an extracted rule (apply De Morgan for compounds)."""
    if rule.is_compound():
        operator = "and" if rule.operator == "or" else "or"
        return ExtractedRule(
            rule_type=RuleType.COMPOUND,
            operator=operator,
            sub_rules=[_negate_rule(r) for r in rule.sub_rules],
            condition_source=rule.condition_source,
        )

    # Membership checks are more naturally negated by swapping rule types.
    if rule.rule_type == RuleType.VALUE_IN_SET:
        return ExtractedRule(
            rule_type=RuleType.VALUE_NOT_IN_SET,
            values=dict(rule.values),
            condition_source=rule.condition_source,
            message_template=rule.message_template,
        )
    if rule.rule_type == RuleType.VALUE_NOT_IN_SET:
        return ExtractedRule(
            rule_type=RuleType.VALUE_IN_SET,
            values=dict(rule.values),
            condition_source=rule.condition_source,
            message_template=rule.message_template,
        )

    values = dict(rule.values)
    values["inverted"] = not values.get("inverted", False)
    return ExtractedRule(
        rule_type=rule.rule_type,
        values=values,
        condition_source=rule.condition_source,
        message_template=rule.message_template,
    )


# =============================================================================
# Full Extractor with Precondition Tracking
# =============================================================================


class RuleExtractor(ast.NodeVisitor):
    """
    Extract validation rules from Django template tag compile functions.

    Combines:
    - Full compound rule extraction (spike 04)
    - Precondition tracking via condition stack (spike 06)
    """

    def __init__(self, source: str, file_path: str):
        self.source = source
        self.file_path = file_path
        self.rules_by_tag: dict[str, TagValidation] = {}
        self.function_stack: list[str] = []
        self.condition_stack: list[ast.expr] = []
        self.env = TokenEnv()
        self.decorator_tags: dict[str, str] = {}
        self.loop_stack: list[tuple[str, str]] = []
        self.constant_sets_stack: list[dict[str, set[str]]] = []

    def get_tag_name(self, func_name: str) -> str:
        """Convert function name to tag name."""
        if func_name in self.decorator_tags:
            return self.decorator_tags[func_name]

        if func_name.startswith("do_"):
            name = func_name[3:]
        elif func_name.startswith("compile_"):
            name = func_name[8:]
        else:
            name = func_name

        # Normalize common suffixes
        if name.endswith("_tag"):
            name = name[:-4]

        return name

    def visit_FunctionDef(self, node: ast.FunctionDef):
        # New function scope
        self.function_stack.append(node.name)
        old_env = self.env
        self.env = TokenEnv()
        self.constant_sets_stack.append(self._collect_constant_sets(node))
        self._register_decorated_tag(node)
        self._register_parse_bits_tag(node)
        self.generic_visit(node)
        self.env = old_env
        self.constant_sets_stack.pop()
        self.function_stack.pop()

    def _register_decorated_tag(self, node: ast.FunctionDef) -> None:
        """Record explicit tag names from decorators."""
        info = self._extract_decorator_info(node)
        if info:
            _, tag_name, _ = info
            self.decorator_tags[node.name] = tag_name

    def _extract_decorator_info(
        self,
        node: ast.FunctionDef,
    ) -> tuple[str, str, bool] | None:
        """
        Return (kind, tag_name, takes_context) if decorator registers a tag.
        kind is one of: tag, simple_tag, inclusion_tag, simple_block_tag.
        """
        for dec in node.decorator_list:
            info = self._parse_decorator(dec, node.name)
            if info:
                return info
        return None

    def _parse_decorator(
        self,
        dec: ast.AST,
        func_name: str,
    ) -> tuple[str, str, bool] | None:
        # Bare decorator: @register.simple_tag
        if isinstance(dec, ast.Attribute):
            if dec.attr in TAG_DECORATORS:
                return (dec.attr, func_name, False)

        if isinstance(dec, ast.Call) and isinstance(dec.func, ast.Attribute):
            if dec.func.attr in TAG_DECORATORS:
                kind = dec.func.attr
                takes_context = False
                name_override = None

                for kw in dec.keywords:
                    if kw.arg == "takes_context" and isinstance(kw.value, ast.Constant):
                        takes_context = bool(kw.value.value)
                    if kw.arg == "name" and isinstance(kw.value, ast.Constant):
                        if isinstance(kw.value.value, str):
                            name_override = kw.value.value

                if kind == "tag":
                    if dec.args:
                        if isinstance(dec.args[0], ast.Constant) and isinstance(
                            dec.args[0].value, str
                        ):
                            name_override = dec.args[0].value

                tag_name = name_override or func_name
                return (kind, tag_name, takes_context)

        return None

    def _register_parse_bits_tag(self, node: ast.FunctionDef) -> None:
        """Register signature-based validation for simple_tag/inclusion_tag."""
        info = self._extract_decorator_info(node)
        if not info:
            return
        kind, tag_name, takes_context = info
        if kind not in ("simple_tag", "inclusion_tag", "simple_block_tag"):
            return

        spec = self._build_parse_bits_spec(node, takes_context, kind)
        validation = self.rules_by_tag.get(tag_name)
        if validation is None:
            validation = TagValidation(tag_name=tag_name, file_path=self.file_path)
            self.rules_by_tag[tag_name] = validation
        validation.parse_bits_spec = spec

    def _build_parse_bits_spec(
        self,
        node: ast.FunctionDef,
        takes_context: bool,
        kind: str,
    ) -> ParseBitsSpec:
        params, has_default, varargs, varkw, kwonly, kwonly_defaults = (
            self._extract_signature(node)
        )

        drop = 0
        if takes_context:
            drop += 1
        if kind == "simple_block_tag":
            drop += 1  # content is provided by block

        params = params[drop:]
        has_default = has_default[drop:]

        required_params = [
            p for p, d in zip(params, has_default, strict=False) if not d
        ]
        required_kwonly = [
            n for n, d in zip(kwonly, kwonly_defaults, strict=False) if d is None
        ]

        allow_as_var = kind in ("simple_tag", "inclusion_tag")

        return ParseBitsSpec(
            params=params,
            required_params=required_params,
            kwonly=kwonly,
            required_kwonly=required_kwonly,
            varargs=varargs,
            varkw=varkw,
            allow_as_var=allow_as_var,
        )

    def _extract_signature(
        self,
        node: ast.FunctionDef,
    ) -> tuple[list[str], list[bool], bool, bool, list[str], list[ast.expr | None]]:
        posonly: list[str] = [a.arg for a in getattr(node.args, "posonlyargs", [])]
        args: list[str] = [a.arg for a in node.args.args]
        params: list[str] = posonly + args
        defaults = node.args.defaults
        default_start = len(params) - len(defaults)
        has_default: list[bool] = [(i >= default_start) for i in range(len(params))]

        varargs = node.args.vararg is not None
        varkw = node.args.kwarg is not None

        kwonly: list[str] = [a.arg for a in node.args.kwonlyargs]
        kwonly_defaults: list[ast.expr | None] = list(node.args.kw_defaults)

        return params, has_default, varargs, varkw, kwonly, kwonly_defaults

    def visit_If(self, node: ast.If):
        """
        Track if/elif/else conditions for precondition extraction.

        We expand elif chains into explicit preconditions:
        - if A: preconditions [A]
        - elif B: preconditions [not A, B]
        - else: preconditions [not A, not B]
        """
        previous_tests: list[ast.expr] = []
        current = node

        while True:
            # if/elif branch
            branch_conds = [self._negate(t) for t in previous_tests]
            branch_conds.append(current.test)
            self._visit_with_conditions(branch_conds, current.body)

            # elif chain
            if len(current.orelse) == 1 and isinstance(current.orelse[0], ast.If):
                previous_tests.append(current.test)
                current = current.orelse[0]
                continue

            # else branch (if any)
            if current.orelse:
                else_conds = [self._negate(t) for t in previous_tests + [current.test]]
                self._visit_with_conditions(else_conds, current.orelse)
            break

    def visit_While(self, node: ast.While):
        """Detect option-parsing while loops."""
        loop_var = self._extract_loop_var(node.test)
        if loop_var and self.function_stack:
            func_name = self.function_stack[-1]
            tag_name = self.get_tag_name(func_name)
            if tag_name not in self.rules_by_tag:
                self.rules_by_tag[tag_name] = TagValidation(
                    tag_name=tag_name,
                    file_path=self.file_path,
                )
            self._extract_option_loop(node, loop_var, self.rules_by_tag[tag_name])
        self.generic_visit(node)

    def visit_For(self, node: ast.For):
        """Track simple for-loops over token lists."""
        if isinstance(node.target, ast.Name) and isinstance(node.iter, ast.Name):
            item_var = node.target.id
            list_var = node.iter.id
            prev_regex = self.env.regex_matches.copy()
            self.loop_stack.append((item_var, list_var))
            for stmt in node.body:
                self.visit(stmt)
            self.loop_stack.pop()
            self.env.regex_matches = prev_regex
            for stmt in node.orelse:
                self.visit(stmt)
            return
        self.generic_visit(node)

    def visit_Match(self, node: ast.Match):
        """Handle match/case patterns for validation extraction."""
        subject_var = self._match_subject_var(node.subject)
        if not subject_var:
            self.generic_visit(node)
            return

        previous_patterns: list[ast.expr] = []

        for case in node.cases:
            pattern_cond = self._pattern_condition(case.pattern, subject_var)
            conds: list[ast.expr] = []
            if previous_patterns:
                for prev in previous_patterns:
                    conds.append(self._negate(prev))
            if pattern_cond is not None:
                conds.append(pattern_cond)
            if case.guard is not None:
                conds.append(case.guard)

            if conds:
                self._visit_with_conditions(conds, case.body)
            else:
                # Unconditional case
                for stmt in case.body:
                    self.visit(stmt)

            if pattern_cond is not None:
                previous_patterns.append(pattern_cond)
            else:
                # Case _ matches anything; no further cases apply
                break

    def _match_subject_var(self, node: ast.AST) -> str | None:
        """Return list variable name used in match subject."""
        if isinstance(node, ast.Name):
            return node.id
        if isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute):
            if isinstance(node.func.value, ast.Name) and node.func.value.id == "token":
                if node.func.attr == "split_contents":
                    return "bits"
            if isinstance(node.func.value, ast.Attribute):
                if (
                    isinstance(node.func.value.value, ast.Name)
                    and node.func.value.value.id == "token"
                ):
                    if node.func.value.attr == "contents" and node.func.attr == "split":
                        return "bits"
        return None

    def _pattern_condition(
        self, pattern: ast.pattern, var_name: str
    ) -> ast.expr | None:
        """Convert a match pattern to a boolean condition expression."""
        # case _:
        if (
            isinstance(pattern, ast.MatchAs)
            and pattern.name is None
            and pattern.pattern is None
        ):
            return None

        if isinstance(pattern, ast.MatchSequence):
            conditions: list[ast.expr] = []
            # len(var) == N
            len_call = ast.Call(
                func=ast.Name(id="len", ctx=ast.Load()),
                args=[ast.Name(id=var_name, ctx=ast.Load())],
                keywords=[],
            )
            conditions.append(
                ast.Compare(
                    left=len_call,
                    ops=[ast.Eq()],
                    comparators=[ast.Constant(value=len(pattern.patterns))],
                )
            )

            for idx, sub in enumerate(pattern.patterns):
                if isinstance(sub, ast.MatchValue) and isinstance(
                    sub.value, ast.Constant
                ):
                    # var[idx] == "const"
                    subscript = ast.Subscript(
                        value=ast.Name(id=var_name, ctx=ast.Load()),
                        slice=ast.Constant(value=idx),
                        ctx=ast.Load(),
                    )
                    conditions.append(
                        ast.Compare(
                            left=subscript,
                            ops=[ast.Eq()],
                            comparators=[ast.Constant(value=sub.value.value)],
                        )
                    )
                elif isinstance(sub, ast.MatchAs):
                    # capture or wildcard, no constraint
                    continue
                else:
                    # Unknown pattern
                    return None

            if not conditions:
                return None
            if len(conditions) == 1:
                return conditions[0]
            return ast.BoolOp(op=ast.And(), values=conditions)

        return None

    def _extract_loop_var(self, test: ast.AST) -> str | None:
        if isinstance(test, ast.Name):
            if test.id in ("bits", "remaining_bits", "remaining", "args"):
                return test.id
        return None

    def _extract_option_loop(
        self,
        node: ast.While,
        loop_var: str,
        validation: TagValidation,
    ) -> None:
        """Extract option loop metadata for validation."""
        option_var = self._find_option_var(node.body, loop_var)
        if not option_var:
            return

        # Record loop environment for validation
        if validation.option_loop_var is None:
            validation.option_loop_var = loop_var
            validation.option_loop_env = self.env.copy()

        for stmt in node.body:
            if isinstance(stmt, ast.If):
                self._analyze_option_if(stmt, option_var, loop_var, validation)

    def _find_option_var(self, body: list[ast.stmt], loop_var: str) -> str | None:
        """Find option variable assigned from loop_var.pop(0)."""
        for stmt in body:
            if isinstance(stmt, ast.Assign):
                if len(stmt.targets) == 1 and isinstance(stmt.targets[0], ast.Name):
                    if self._is_pop_zero(stmt.value, loop_var):
                        return stmt.targets[0].id
        return None

    def _is_pop_zero(self, node: ast.AST, loop_var: str) -> bool:
        if isinstance(node, ast.Call):
            if isinstance(node.func, ast.Attribute) and node.func.attr == "pop":
                if (
                    isinstance(node.func.value, ast.Name)
                    and node.func.value.id == loop_var
                ):
                    if not node.args:
                        return False
                    if (
                        isinstance(node.args[0], ast.Constant)
                        and node.args[0].value == 0
                    ):
                        return True
        return False

    def _analyze_option_if(
        self,
        node: ast.If,
        option_var: str,
        loop_var: str,
        validation: TagValidation,
    ) -> None:
        # Duplicate detection: if option in options
        if self._is_duplicate_check(node.test, option_var):
            validation.no_duplicate_options = True
            for stmt in node.body:
                if isinstance(stmt, ast.If):
                    self._analyze_option_if(stmt, option_var, loop_var, validation)
            for stmt in node.orelse:
                if isinstance(stmt, ast.If):
                    self._analyze_option_if(stmt, option_var, loop_var, validation)
            return

        opt_name = self._extract_option_check(node.test, option_var)
        if opt_name:
            spec = self._analyze_option_body(opt_name, node.body, loop_var)
            if opt_name not in validation.valid_options:
                validation.valid_options.append(opt_name)
            validation.option_constraints[opt_name] = spec

        # elif chain
        if node.orelse:
            if len(node.orelse) == 1 and isinstance(node.orelse[0], ast.If):
                self._analyze_option_if(
                    node.orelse[0], option_var, loop_var, validation
                )
            else:
                if self._has_template_syntax_error(node.orelse):
                    validation.rejects_unknown_options = True

    def _is_duplicate_check(self, test: ast.AST, option_var: str) -> bool:
        if isinstance(test, ast.Compare):
            if len(test.ops) == 1 and isinstance(test.ops[0], ast.In):
                if isinstance(test.left, ast.Name) and test.left.id == option_var:
                    if isinstance(test.comparators[0], ast.Name):
                        return True
        return False

    def _extract_option_check(self, test: ast.AST, option_var: str) -> str | None:
        if isinstance(test, ast.Compare):
            if len(test.ops) == 1 and isinstance(test.ops[0], ast.Eq):
                if isinstance(test.left, ast.Name) and test.left.id == option_var:
                    if isinstance(test.comparators[0], ast.Constant):
                        value = test.comparators[0].value
                        if isinstance(value, str):
                            return value
        return None

    def _analyze_option_body(
        self,
        option_name: str,
        body: list[ast.stmt],
        loop_var: str,
    ) -> dict[str, Any]:
        """Return option constraint dict."""
        spec: dict[str, Any] = {"type": "boolean"}
        value_vars: set[str] = set()
        arg_vars: set[str] = set()

        for stmt in body:
            if isinstance(stmt, ast.Assign):
                if len(stmt.targets) == 1 and isinstance(stmt.targets[0], ast.Name):
                    target = stmt.targets[0].id
                    if self._is_boolean_assignment(stmt):
                        spec["type"] = "boolean"
                    elif self._is_token_kwargs_call(stmt):
                        spec["type"] = "kwargs"
                        spec["support_legacy"] = self._token_kwargs_support_legacy(stmt)
                        value_vars.add(target)
                    elif self._is_pop_assign(stmt, loop_var):
                        spec["type"] = "single_arg"
                        arg_vars.add(target)
            elif isinstance(stmt, ast.Try):
                # used in some options to pop with validation
                spec["type"] = "single_arg"
                arg_var = self._extract_pop_target_from_try(stmt, loop_var)
                if arg_var:
                    arg_vars.add(arg_var)
            elif isinstance(stmt, ast.If):
                constraint = self._extract_option_constraint(stmt, value_vars, arg_vars)
                if constraint:
                    spec.update(constraint)

        return spec

    def _is_boolean_assignment(self, stmt: ast.Assign) -> bool:
        if len(stmt.targets) == 1 and isinstance(stmt.targets[0], ast.Name):
            if isinstance(stmt.value, ast.Constant) and stmt.value.value is True:
                return True
        return False

    def _is_token_kwargs_call(self, stmt: ast.Assign) -> bool:
        if isinstance(stmt.value, ast.Call):
            if isinstance(stmt.value.func, ast.Name):
                return stmt.value.func.id == "token_kwargs"
        return False

    def _token_kwargs_support_legacy(self, stmt: ast.Assign) -> bool:
        """
        Determine whether a token_kwargs(...) call enables legacy parsing.

        In Django, token_kwargs(..., support_legacy=True) accepts `value as key`
        with `and` separators. We mirror that statically by annotating the
        option constraints so validation can consume legacy kwargs correctly.
        """
        if not isinstance(stmt.value, ast.Call):
            return False
        for kw in stmt.value.keywords:
            if kw.arg == "support_legacy" and isinstance(kw.value, ast.Constant):
                return kw.value.value is True
        return False

    def _is_pop_assign(self, stmt: ast.Assign, loop_var: str) -> bool:
        if isinstance(stmt.value, ast.Call):
            if (
                isinstance(stmt.value.func, ast.Attribute)
                and stmt.value.func.attr == "pop"
            ):
                if isinstance(stmt.value.func.value, ast.Name):
                    return stmt.value.func.value.id == loop_var
        return False

    def _extract_pop_target_from_try(self, stmt: ast.Try, loop_var: str) -> str | None:
        for inner in stmt.body:
            if isinstance(inner, ast.Assign):
                if len(inner.targets) == 1 and isinstance(inner.targets[0], ast.Name):
                    if self._is_pop_assign(inner, loop_var):
                        return inner.targets[0].id
        return None

    def _extract_option_constraint(
        self,
        node: ast.If,
        value_vars: set[str],
        arg_vars: set[str],
    ) -> dict[str, Any] | None:
        test = node.test

        # if not value: -> min_kwargs 1
        if isinstance(test, ast.UnaryOp) and isinstance(test.op, ast.Not):
            if isinstance(test.operand, ast.Name):
                if not value_vars or test.operand.id in value_vars:
                    if self._has_template_syntax_error(node.body):
                        return {"min_kwargs": 1}

        # if len(value) != 1: -> exact_count 1
        if isinstance(test, ast.Compare):
            if self._is_len_check(test.left, value_vars):
                if len(test.ops) == 1 and isinstance(test.ops[0], ast.NotEq):
                    if isinstance(test.comparators[0], ast.Constant):
                        if self._has_template_syntax_error(node.body):
                            return {"exact_count": test.comparators[0].value}
            # if value in {"as", "noop"}: -> disallow values
            if (
                len(test.ops) == 1
                and isinstance(test.left, ast.Name)
                and test.left.id in arg_vars
                and self._has_template_syntax_error(node.body)
            ):
                disallowed = self._resolve_str_set(test.comparators)
                if disallowed:
                    if isinstance(test.ops[0], ast.In):
                        return {"arg_disallow": sorted(disallowed)}
                    if isinstance(test.ops[0], ast.NotIn):
                        return {"arg_allow": sorted(disallowed)}
        return None

    def _resolve_str_set(self, comparators: list[ast.expr]) -> set[str] | None:
        if len(comparators) != 1:
            return None
        comp = comparators[0]
        if isinstance(comp, ast.Name):
            return self._current_constant_sets().get(comp.id)
        if isinstance(comp, (ast.Set, ast.Tuple, ast.List)):
            values: set[str] = set()
            for elt in comp.elts:
                if isinstance(elt, ast.Constant) and isinstance(elt.value, str):
                    values.add(elt.value)
                else:
                    return None
            return values
        return None

    def _collect_constant_sets(self, node: ast.FunctionDef) -> dict[str, set[str]]:
        constants: dict[str, set[str]] = {}
        for stmt in ast.walk(node):
            if isinstance(stmt, ast.Assign):
                if len(stmt.targets) == 1 and isinstance(stmt.targets[0], ast.Name):
                    target = stmt.targets[0].id
                    values = self._extract_str_set(stmt.value)
                    if values:
                        constants[target] = values
        return constants

    def _extract_str_set(self, node: ast.AST) -> set[str] | None:
        if isinstance(node, (ast.Set, ast.List, ast.Tuple)):
            values: set[str] = set()
            for elt in node.elts:
                if isinstance(elt, ast.Constant) and isinstance(elt.value, str):
                    values.add(elt.value)
                else:
                    return None
            return values
        return None

    def _current_constant_sets(self) -> dict[str, set[str]]:
        if not self.constant_sets_stack:
            return {}
        return self.constant_sets_stack[-1]

    def _is_len_check(self, node: ast.AST, value_vars: set[str]) -> bool:
        if isinstance(node, ast.Call):
            if isinstance(node.func, ast.Name) and node.func.id == "len":
                if len(node.args) == 1 and isinstance(node.args[0], ast.Name):
                    if not value_vars:
                        return True
                    return node.args[0].id in value_vars
        return False

    def _has_template_syntax_error(self, body: list[ast.stmt]) -> bool:
        for stmt in body:
            if isinstance(stmt, ast.Raise):
                if isinstance(stmt.exc, ast.Call):
                    func = stmt.exc.func
                    if isinstance(func, ast.Name) and func.id == "TemplateSyntaxError":
                        return True
                    if (
                        isinstance(func, ast.Attribute)
                        and func.attr == "TemplateSyntaxError"
                    ):
                        return True
        return False

    def _visit_with_conditions(self, conds: list[ast.expr], stmts: list[ast.stmt]):
        """Visit statements with additional preconditions."""
        self.condition_stack.extend(conds)
        for stmt in stmts:
            self.visit(stmt)
        for _ in conds:
            self.condition_stack.pop()

    def _negate(self, expr: ast.expr) -> ast.expr:
        """Return NOT expr."""
        return ast.UnaryOp(op=ast.Not(), operand=expr)

    def _current_guard(self) -> ast.expr | None:
        """Return conjunction of current conditions, or None."""
        if not self.condition_stack:
            return None
        if len(self.condition_stack) == 1:
            return self.condition_stack[0]
        return ast.BoolOp(op=ast.And(), values=list(self.condition_stack))

    # ---------------------------------------------------------------------
    # Token env tracking (assignments and list ops)
    # ---------------------------------------------------------------------
    def visit_Assign(self, node: ast.Assign):
        """
        Track simple token list assignments and slice mutations.
        """
        # Only handle simple name assignments
        if not node.targets:
            return

        guard = self._current_guard()

        for target in node.targets:
            if not isinstance(target, ast.Name):
                continue

            var_name = target.id
            value = node.value

            # Pattern: bits = token.split_contents() / token.contents.split()
            base_view = self._extract_base_view(value)
            if base_view is not None:
                if guard is None:
                    self.env.variables[var_name] = base_view
                else:
                    # Conditional base assignment not supported; mark unknown
                    self.env.variables[var_name] = TokenView(unknown=True)
                continue

            # Pattern: bits = bits[1:], args = bits[1:], bits = bits[:-2], etc.
            if isinstance(value, ast.Subscript):
                slice_info = self._extract_slice(value.slice)
                if slice_info is not None:
                    src_name = extract_variable_name(value)
                    if src_name:
                        # Same-var slice => treat as conditional slice op
                        if src_name == var_name:
                            self._append_slice_op(var_name, slice_info, guard)
                        else:
                            # Derived slice from another var
                            if guard is None:
                                src_view = self.env.variables.get(src_name)
                                if src_view:
                                    new_view = src_view.copy()
                                    new_view.ops.append(
                                        ConditionalOp(
                                            guard=None,
                                            op="slice",
                                            start=slice_info[0],
                                            end=slice_info[1],
                                        )
                                    )
                                    self.env.variables[var_name] = new_view
                            else:
                                # Conditional derived assignment not supported
                                self.env.variables[var_name] = TokenView(unknown=True)
                        continue

                # Pattern: var = bits[index]
                if isinstance(value.value, ast.Name):
                    src_name = value.value.id
                    index = extract_subscript_index(value)
                    if index is not None:
                        self._assign_token_ref(var_name, src_name, index, guard)
                        continue

            # Pattern: x = bits.pop()
            if isinstance(value, ast.Call):
                if isinstance(value.func, ast.Attribute) and value.func.attr == "pop":
                    if isinstance(value.func.value, ast.Name):
                        list_name = value.func.value.id
                        # Snapshot before pop
                        self._assign_token_ref(var_name, list_name, -1, guard)
                        index = None
                        if value.args:
                            if isinstance(value.args[0], ast.Constant) and isinstance(
                                value.args[0].value, int
                            ):
                                index = value.args[0].value
                            elif isinstance(value.args[0], ast.UnaryOp) and isinstance(
                                value.args[0].op, ast.USub
                            ):
                                val = extract_constant(value.args[0].operand)
                                if isinstance(val, int):
                                    index = -val
                            if index is not None:
                                # Update token ref to match index provided
                                self._assign_token_ref(
                                    var_name, list_name, index, guard
                                )
                        self._append_pop_op(list_name, index, guard)
                        continue

            # Pattern: match = kwarg_re.match(bit)
            if isinstance(value, ast.Call) and isinstance(value.func, ast.Attribute):
                if (
                    isinstance(value.func.value, ast.Name)
                    and value.func.value.id == "kwarg_re"
                ):
                    if value.func.attr == "match" and value.args:
                        if isinstance(value.args[0], ast.Name):
                            item_var = value.args[0].id
                            list_var = self._list_var_for_item(item_var)
                            if list_var:
                                self.env.regex_matches[var_name] = RegexMatch(
                                    list_var=list_var,
                                    pattern="kwarg_re",
                                )
                                continue

            # Pattern: args = bits (simple name copy)
            if isinstance(value, ast.Name):
                src_name = value.id
                if guard is None:
                    src_view = self.env.variables.get(src_name)
                    if src_view:
                        self.env.variables[var_name] = src_view.copy()
                else:
                    self.env.variables[var_name] = TokenView(unknown=True)

        self.generic_visit(node)

    def visit_Expr(self, node: ast.Expr):
        """Track list mutations like bits.pop() and bits.pop(0)."""
        if isinstance(node.value, ast.Call):
            call = node.value
            if isinstance(call.func, ast.Attribute) and call.func.attr == "pop":
                if isinstance(call.func.value, ast.Name):
                    var_name = call.func.value.id
                    index = None
                    if call.args:
                        if isinstance(call.args[0], ast.Constant) and isinstance(
                            call.args[0].value, int
                        ):
                            index = call.args[0].value
                        elif isinstance(call.args[0], ast.UnaryOp) and isinstance(
                            call.args[0].op, ast.USub
                        ):
                            val = extract_constant(call.args[0].operand)
                            if isinstance(val, int):
                                index = -val
                        else:
                            index = None
                    self._append_pop_op(var_name, index, self._current_guard())
                    return
        self.generic_visit(node)

    def _extract_base_view(self, value: ast.AST) -> TokenView | None:
        """Return base TokenView if value is token.split_contents() or similar."""
        # token.split_contents()
        if isinstance(value, ast.Call) and isinstance(value.func, ast.Attribute):
            if (
                isinstance(value.func.value, ast.Name)
                and value.func.value.id == "token"
            ):
                if value.func.attr == "split_contents":
                    return TokenView(start=0, end=None)
            # token.contents.split()
            if isinstance(value.func.value, ast.Attribute):
                if (
                    isinstance(value.func.value.value, ast.Name)
                    and value.func.value.value.id == "token"
                ):
                    if (
                        value.func.value.attr == "contents"
                        and value.func.attr == "split"
                    ):
                        return TokenView(start=0, end=None)

        # list(token.split_contents())
        if (
            isinstance(value, ast.Call)
            and isinstance(value.func, ast.Name)
            and value.func.id == "list"
        ):
            if value.args:
                inner = value.args[0]
                return self._extract_base_view(inner)

        # token.split_contents()[slice]
        if isinstance(value, ast.Subscript):
            base = self._extract_base_view(value.value)
            if base is not None:
                slice_info = self._extract_slice(value.slice)
                if slice_info is not None:
                    base.ops.append(
                        ConditionalOp(
                            guard=None,
                            op="slice",
                            start=slice_info[0],
                            end=slice_info[1],
                        )
                    )
                return base

        return None

    def _extract_slice(self, node: ast.AST) -> tuple[int | None, int | None] | None:
        """Extract slice bounds from a Subscript slice."""
        if isinstance(node, ast.Slice):
            start = extract_constant(node.lower) if node.lower else None
            end = extract_constant(node.upper) if node.upper else None
            if isinstance(start, int) or start is None:
                if isinstance(end, int) or end is None:
                    return (start, end)
        return None

    def _append_slice_op(
        self,
        var_name: str,
        slice_info: tuple[int | None, int | None],
        guard: ast.expr | None,
    ) -> None:
        view = self.env.variables.get(var_name)
        if view is None:
            view = TokenView()
            self.env.variables[var_name] = view
        view.ops.append(
            ConditionalOp(
                guard=guard,
                op="slice",
                start=slice_info[0],
                end=slice_info[1],
            )
        )

    def _append_pop_op(
        self,
        var_name: str,
        index: int | None,
        guard: ast.expr | None,
    ) -> None:
        view = self.env.variables.get(var_name)
        if view is None:
            view = TokenView()
            self.env.variables[var_name] = view
        view.ops.append(
            ConditionalOp(
                guard=guard,
                op="pop",
                index=index,
            )
        )

    def _assign_token_ref(
        self,
        var_name: str,
        source_name: str,
        index: int,
        guard: ast.expr | None,
    ) -> None:
        """Assign a variable to reference a specific token in a list view."""
        source_view = self.env.variables.get(source_name)
        if source_view is None:
            if source_name == "bits":
                source_view = TokenView()
            else:
                return

        ref = TokenRef(
            source=source_name,
            view=source_view.copy(),
            index=index,
            guard=guard,
        )
        self.env.values[var_name] = ref

    def _list_var_for_item(self, item_var: str) -> str | None:
        for item, lst in reversed(self.loop_stack):
            if item == item_var:
                return lst
        return None

    def visit_Raise(self, node: ast.Raise):
        """Extract rule when TemplateSyntaxError is raised."""
        if not self._is_template_syntax_error(node):
            return

        if not self.function_stack:
            return

        func_name = self.function_stack[-1]
        tag_name = self.get_tag_name(func_name)

        if tag_name not in self.rules_by_tag:
            self.rules_by_tag[tag_name] = TagValidation(
                tag_name=tag_name,
                file_path=self.file_path,
            )

        if self.condition_stack:
            trigger = self.condition_stack[-1]
            rule = self._extract_rule_with_env(trigger, node)

            preconditions = []
            for cond in self.condition_stack[:-1]:
                preconditions.append(
                    Precondition(
                        expr=cond,
                        source=ast.get_source_segment(self.source, cond) or "",
                    )
                )

            contextual_rule = ContextualRule(
                rule=rule,
                preconditions=preconditions,
                env=self.env.copy(),
            )
            self.rules_by_tag[tag_name].rules.append(contextual_rule)

    def _extract_rule_with_env(
        self, trigger: ast.AST, node: ast.Raise
    ) -> ExtractedRule:
        """Extract rule with special handling for regex match patterns."""
        if isinstance(trigger, ast.UnaryOp) and isinstance(trigger.op, ast.Not):
            if isinstance(trigger.operand, ast.Name):
                var_name = trigger.operand.id
                if var_name in self.env.regex_matches:
                    regex = self.env.regex_matches[var_name]
                    rule = ExtractedRule(
                        rule_type=RuleType.REGEX_MATCH,
                        values={"list": regex.list_var, "pattern": regex.pattern},
                        condition_source=ast.get_source_segment(self.source, trigger)
                        or "",
                    )
                    rule.message_template = self._extract_message(node)
                    return rule

        rule = extract_rule(trigger, self.source)
        rule.message_template = self._extract_message(node)
        return rule

    def _is_template_syntax_error(self, node: ast.Raise) -> bool:
        """Check if this raises TemplateSyntaxError."""
        if node.exc is None:
            return False
        if isinstance(node.exc, ast.Call):
            func = node.exc.func
            if isinstance(func, ast.Name) and func.id == "TemplateSyntaxError":
                return True
            if isinstance(func, ast.Attribute) and func.attr == "TemplateSyntaxError":
                return True
        return False

    def _extract_message(self, node: ast.Raise) -> str | None:
        """Extract error message template from raise statement."""
        if isinstance(node.exc, ast.Call) and node.exc.args:
            first_arg = node.exc.args[0]
            if isinstance(first_arg, ast.Constant):
                value = first_arg.value
                if isinstance(value, str):
                    return value
                return None
            return ast.get_source_segment(self.source, first_arg)
        return None


# =============================================================================
