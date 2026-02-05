"""Secondary extraction helpers.

These helpers augment tag validations by following helper functions invoked from
registered tag handlers (best-effort, static).
"""

from __future__ import annotations

import ast

from ..types import ParseBitsSpec
from ..types import TagValidation
from .registry import tag_name_from_decorator
from .registry import tag_registration_from_call
from .rules import RuleExtractor


def _augment_tag_validations_with_helpers(
    tree: ast.Module,
    source: str,
    file_path: str,
    rules: dict[str, TagValidation],
) -> None:
    func_defs, class_methods = _collect_function_definitions(tree)
    tag_functions = _collect_tag_function_map(tree)

    for func_name, tag_names in tag_functions.items():
        func_def = func_defs.get(func_name)
        if not func_def:
            continue
        for call in ast.walk(func_def):
            if not isinstance(call, ast.Call):
                continue
            _maybe_attach_parse_bits_from_inclusion_admin(
                call,
                func_defs,
                source,
                file_path,
                tag_names,
                rules,
            )
            helper_def = _resolve_helper_call(call, func_defs, class_methods)
            if helper_def is None:
                continue
            helper_func, implicit_first = helper_def
            if not _call_uses_parser_token(call, helper_func, implicit_first):
                continue
            helper_rules = _extract_rules_for_helper(
                helper_func, source, file_path, tag_names
            )
            _merge_helper_rules(rules, helper_rules)

    _propagate_tag_aliases(tag_functions, rules)
    _mark_unrestricted_tags(tag_functions, func_defs, rules)


def _propagate_tag_aliases(
    tag_functions: dict[str, set[str]],
    rules: dict[str, TagValidation],
) -> None:
    for _func_name, tag_names in tag_functions.items():
        if len(tag_names) < 2:
            continue
        source_name = None
        for name in sorted(tag_names):
            validation = rules.get(name)
            if validation and _has_validation(validation):
                source_name = name
                break
        if source_name is None:
            continue
        source_validation = rules[source_name]
        for name in tag_names:
            if name == source_name:
                continue
            if not _has_validation(rules.get(name)):
                rules[name] = _clone_validation_for_tag(source_validation, name)


def _clone_validation_for_tag(
    validation: TagValidation,
    tag_name: str,
) -> TagValidation:
    cloned = TagValidation(tag_name=tag_name, file_path=validation.file_path)
    cloned.rules = list(validation.rules)
    cloned.valid_options = list(validation.valid_options)
    cloned.option_constraints = dict(validation.option_constraints)
    cloned.no_duplicate_options = validation.no_duplicate_options
    cloned.rejects_unknown_options = validation.rejects_unknown_options
    cloned.option_loop_var = validation.option_loop_var
    cloned.option_loop_env = (
        validation.option_loop_env.copy() if validation.option_loop_env else None
    )
    cloned.parse_bits_spec = validation.parse_bits_spec
    cloned.unrestricted = validation.unrestricted
    return cloned


def _has_validation(validation: TagValidation | None) -> bool:
    if validation is None:
        return False
    return bool(validation.rules) or (
        validation.parse_bits_spec is not None
        or bool(validation.valid_options)
        or bool(validation.option_constraints)
        or validation.no_duplicate_options
        or validation.rejects_unknown_options
        or validation.unrestricted
    )


def _mark_unrestricted_tags(
    tag_functions: dict[str, set[str]],
    func_defs: dict[str, ast.FunctionDef],
    rules: dict[str, TagValidation],
) -> None:
    for func_name, tag_names in tag_functions.items():
        func_def = func_defs.get(func_name)
        if func_def is None:
            continue
        if _function_raises_template_syntax_error(func_def):
            continue
        for tag_name in tag_names:
            validation = rules.get(tag_name)
            if validation is None:
                continue
            if not _has_validation(validation):
                validation.unrestricted = True


def _function_raises_template_syntax_error(func: ast.FunctionDef) -> bool:
    for node in ast.walk(func):
        if not isinstance(node, ast.Raise):
            continue
        if node.exc is None:
            continue
        if isinstance(node.exc, ast.Call):
            func_node = node.exc.func
            if (
                isinstance(func_node, ast.Name)
                and func_node.id == "TemplateSyntaxError"
            ):
                return True
            if (
                isinstance(func_node, ast.Attribute)
                and func_node.attr == "TemplateSyntaxError"
            ):
                return True
    return False


def _collect_function_definitions(
    tree: ast.Module,
) -> tuple[dict[str, ast.FunctionDef], dict[tuple[str, str], ast.FunctionDef]]:
    func_defs: dict[str, ast.FunctionDef] = {}
    class_methods: dict[tuple[str, str], ast.FunctionDef] = {}
    for node in tree.body:
        if isinstance(node, ast.FunctionDef):
            func_defs[node.name] = node
        elif isinstance(node, ast.ClassDef):
            for child in node.body:
                if isinstance(child, ast.FunctionDef):
                    class_methods[(node.name, child.name)] = child
    return func_defs, class_methods


def _collect_tag_function_map(tree: ast.AST) -> dict[str, set[str]]:
    mapping: dict[str, set[str]] = {}
    for node in ast.walk(tree):
        if isinstance(node, ast.FunctionDef):
            for dec in node.decorator_list:
                name = tag_name_from_decorator(dec, node.name)
                if name:
                    mapping.setdefault(node.name, set()).add(name)
        if isinstance(node, ast.Call):
            name, func_name = tag_registration_from_call(node)
            if name and func_name:
                mapping.setdefault(func_name, set()).add(name)
    return mapping


def _resolve_helper_call(
    call: ast.Call,
    func_defs: dict[str, ast.FunctionDef],
    class_methods: dict[tuple[str, str], ast.FunctionDef],
) -> tuple[ast.FunctionDef, bool] | None:
    if isinstance(call.func, ast.Name):
        func = func_defs.get(call.func.id)
        if func:
            return func, False
    if isinstance(call.func, ast.Attribute) and isinstance(call.func.value, ast.Name):
        key = (call.func.value.id, call.func.attr)
        func = class_methods.get(key)
        if func:
            implicit_first = _has_classmethod_decorator(func)
            return func, implicit_first
    return None


def _has_classmethod_decorator(func: ast.FunctionDef) -> bool:
    for dec in func.decorator_list:
        if isinstance(dec, ast.Name) and dec.id == "classmethod":
            return True
    return False


def _call_uses_parser_token(
    call: ast.Call,
    func: ast.FunctionDef,
    implicit_first: bool,
) -> bool:
    params = [a.arg for a in getattr(func.args, "posonlyargs", [])] + [
        a.arg for a in func.args.args
    ]
    offset = 1 if implicit_first else 0
    parser_idx = None
    token_idx = None
    for i, name in enumerate(params[offset:], 0):
        if name == "parser":
            parser_idx = i
        if name == "token":
            token_idx = i

    if parser_idx is None or token_idx is None:
        return False

    def _arg_is_name(idx: int, name: str) -> bool:
        if idx < len(call.args):
            arg = call.args[idx]
            return isinstance(arg, ast.Name) and arg.id == name
        return False

    parser_ok = _arg_is_name(parser_idx, "parser")
    token_ok = _arg_is_name(token_idx, "token")

    for kw in call.keywords:
        if kw.arg == "parser":
            parser_ok = isinstance(kw.value, ast.Name) and kw.value.id == "parser"
        if kw.arg == "token":
            token_ok = isinstance(kw.value, ast.Name) and kw.value.id == "token"

    return parser_ok and token_ok


def _extract_rules_for_helper(
    func: ast.FunctionDef,
    source: str,
    file_path: str,
    tag_names: set[str],
) -> dict[str, TagValidation]:
    extractor = RuleExtractor(source, file_path)
    for tag_name in tag_names:
        extractor.decorator_tags[func.name] = tag_name
    extractor.visit(func)
    return extractor.rules_by_tag


def _merge_helper_rules(
    rules: dict[str, TagValidation],
    helper_rules: dict[str, TagValidation],
) -> None:
    for tag_name, validation in helper_rules.items():
        existing = rules.get(tag_name)
        if existing is None:
            rules[tag_name] = validation
            continue
        existing.rules.extend(validation.rules)
        if existing.parse_bits_spec is None and validation.parse_bits_spec is not None:
            existing.parse_bits_spec = validation.parse_bits_spec


def _maybe_attach_parse_bits_from_inclusion_admin(
    call: ast.Call,
    func_defs: dict[str, ast.FunctionDef],
    source: str,
    file_path: str,
    tag_names: set[str],
    rules: dict[str, TagValidation],
) -> None:
    if not isinstance(call.func, ast.Name) or call.func.id != "InclusionAdminNode":
        return

    func_node = None
    takes_context = True
    func_is_lambda = False

    if len(call.args) >= 4 and func_node is None:
        if isinstance(call.args[3], ast.Name):
            func_node = func_defs.get(call.args[3].id)

    for kw in call.keywords:
        if kw.arg == "func" and isinstance(kw.value, ast.Name):
            func_node = func_defs.get(kw.value.id)
        if kw.arg == "func" and isinstance(kw.value, ast.Lambda):
            func_is_lambda = True
        if kw.arg == "takes_context" and isinstance(kw.value, ast.Constant):
            takes_context = bool(kw.value.value)

    if func_node is None and not func_is_lambda:
        return

    if func_node is not None:
        extractor = RuleExtractor(source, file_path)
        spec = extractor._build_parse_bits_spec(
            func_node, takes_context, "inclusion_tag"
        )
    else:
        spec = ParseBitsSpec(
            params=[],
            required_params=[],
            kwonly=[],
            required_kwonly=[],
            varargs=False,
            varkw=False,
            allow_as_var=True,
        )
    for tag_name in tag_names:
        validation = rules.get(tag_name)
        if validation is None:
            validation = TagValidation(tag_name=tag_name, file_path=file_path)
            rules[tag_name] = validation
        if validation.parse_bits_spec is None:
            validation.parse_bits_spec = spec
