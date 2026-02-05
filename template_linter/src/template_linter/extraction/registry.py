"""
Helpers for discovering registered template tags and filters.
"""

from __future__ import annotations

import ast

# Names of `django.template.Library` decorator helpers that register tags.
#
# These cover the common Django surface area:
# - `@register.tag` / `@register.tag("name")`
# - `@register.simple_tag(...)`, `@register.inclusion_tag(...)`
# - `@register.simple_block_tag(...)` (Django 5.2+)
#
# Extraction is intentionally best-effort: we treat these as signals that the
# decorated function is a template tag, then extract constraints separately.
TAG_DECORATORS = ("tag", "simple_tag", "inclusion_tag", "simple_block_tag")

# Names of `django.template.Library` decorator helpers that register filters.
#
# Note: the call-form `register.filter(...)` is handled elsewhere; this constant
# is strictly for decorator detection.
FILTER_DECORATORS = ("filter",)

# Names of *non-Django* decorator helpers that behave like tag registration.
#
# Some projects wrap Django's registration APIs (e.g. Pretix wraps
# `simple_block_tag`-like semantics via `register_simple_block_tag(...)`).
# These aren't attributes on `template.Library`, but in practice they still
# define template tags, so we recognize them for corpus discovery.
#
# This is not a template exception/skiplist: it just ensures we don't miss tags
# that are real at runtime but registered through a thin wrapper.
TAG_HELPER_DECORATORS = ("register_simple_block_tag",)


def _callable_name(node: ast.AST) -> str | None:
    """
    Best-effort name for a callable used in register.tag/register.filter calls.

    This is used only for mapping a registration back to a definition when
    possible. Many real-world projects register class methods
    (e.g. `SomeNode.handle`), which we cannot always resolve to an AST
    `FunctionDef` without extra context; in those cases, returning a dotted name
    is still useful for debugging, but callers must treat it as optional.
    """
    if isinstance(node, ast.Name):
        return node.id
    if isinstance(node, ast.Attribute):
        base = _callable_name(node.value)
        if base:
            return f"{base}.{node.attr}"
    return None


def _kw_constant_str(keywords: list[ast.keyword], name: str) -> str | None:
    for kw in keywords:
        if kw.arg != name:
            continue
        if isinstance(kw.value, ast.Constant) and isinstance(kw.value.value, str):
            return kw.value.value
    return None


def _kw_name_from(keywords: list[ast.keyword]) -> str | None:
    return _kw_constant_str(keywords, "name")


def tag_name_from_decorator(dec: ast.AST, func_name: str) -> str | None:
    if isinstance(dec, ast.Attribute):
        if dec.attr in TAG_DECORATORS:
            return func_name

    if isinstance(dec, ast.Call) and isinstance(dec.func, ast.Attribute):
        if dec.func.attr in TAG_DECORATORS:
            name_override = None
            if dec.func.attr == "tag" and dec.args:
                if isinstance(dec.args[0], ast.Constant) and isinstance(
                    dec.args[0].value, str
                ):
                    name_override = dec.args[0].value
            name_override = _kw_name_from(dec.keywords) or name_override
            return name_override or func_name

    # Common wrapper used by pretix (and potentially others):
    #
    #   @register_simple_block_tag(register, name="foo", end_name="endfoo")
    #   def foo(content, ...): ...
    #
    # This wraps Django's `simple_block_tag` semantics but isn't an attribute on
    # `template.Library`, so treat it as a tag registration signal for corpus
    # discovery.
    if isinstance(dec, ast.Call) and isinstance(dec.func, ast.Name):
        if dec.func.id in TAG_HELPER_DECORATORS:
            return _kw_name_from(dec.keywords) or func_name
    return None


def tag_registration_from_call(node: ast.Call) -> tuple[str | None, str | None]:
    if not isinstance(node.func, ast.Attribute):
        return None, None
    if node.func.attr not in TAG_DECORATORS:
        return None, None

    name_override = _kw_name_from(node.keywords)
    func_name: str | None = None
    for kw in node.keywords:
        if kw.arg in ("compile_function", "func") and isinstance(kw.value, ast.Name):
            func_name = kw.value.id

    if len(node.args) >= 2:
        if isinstance(node.args[0], ast.Constant) and isinstance(
            node.args[0].value, str
        ):
            name = node.args[0].value
            func_name = _callable_name(node.args[1]) or func_name
            return name_override or name, func_name
    if len(node.args) == 1:
        # `register.simple_tag(func, name="alias")`
        func_name = _callable_name(node.args[0]) or func_name
        if name_override:
            return name_override, func_name
        if isinstance(node.args[0], ast.Name):
            return node.args[0].id, func_name

    return name_override, func_name


def tag_name_from_call(node: ast.Call) -> str | None:
    name, _func = tag_registration_from_call(node)
    return name


def filter_name_from_decorator(dec: ast.AST, func_name: str) -> str | None:
    if isinstance(dec, ast.Attribute):
        if dec.attr in FILTER_DECORATORS:
            return func_name

    if isinstance(dec, ast.Call) and isinstance(dec.func, ast.Attribute):
        if dec.func.attr in FILTER_DECORATORS:
            name_override = None
            if dec.args:
                if isinstance(dec.args[0], ast.Constant) and isinstance(
                    dec.args[0].value, str
                ):
                    name_override = dec.args[0].value
            name_override = _kw_name_from(dec.keywords) or name_override
            return name_override or func_name
    return None


def filter_registration_from_call(node: ast.Call) -> tuple[str | None, str | None]:
    # Decorator-call application form:
    #   register.filter(...)(func)
    # This appears in some real-world code where filters are registered in bulk.
    if (
        isinstance(node.func, ast.Call)
        and isinstance(node.func.func, ast.Attribute)
        and node.func.func.attr == "filter"
        and node.args
    ):
        inner = node.func
        func_name = _callable_name(node.args[0])
        if func_name is None:
            return None, None

        name_override: str | None = None
        if inner.args:
            if isinstance(inner.args[0], ast.Constant) and isinstance(
                inner.args[0].value, str
            ):
                name_override = inner.args[0].value
        name_override = _kw_name_from(inner.keywords) or name_override
        default = func_name.rsplit(".", 1)[-1]
        return name_override or default, default

    if not isinstance(node.func, ast.Attribute):
        return None, None
    # `register.filter(...)` is the runtime call form. (The decorator form is
    # handled separately in `filter_name_from_decorator`.)
    if node.func.attr != "filter":
        return None, None

    name_override = _kw_name_from(node.keywords)
    func_name: str | None = None
    for kw in node.keywords:
        if kw.arg in ("filter_func", "func") and isinstance(kw.value, ast.Name):
            func_name = kw.value.id

    if len(node.args) >= 2:
        if isinstance(node.args[0], ast.Constant) and isinstance(
            node.args[0].value, str
        ):
            name = node.args[0].value
            func_name = _callable_name(node.args[1]) or func_name
            return name_override or name, func_name
    if len(node.args) == 1:
        func_name = _callable_name(node.args[0]) or func_name
        if name_override:
            return name_override, func_name
        if isinstance(node.args[0], ast.Name):
            return node.args[0].id, func_name

    return name_override, func_name


def filter_name_from_call(node: ast.Call) -> str | None:
    name, _func = filter_registration_from_call(node)
    return name


def collect_registered_tags(tree: ast.AST) -> set[str]:
    tags: set[str] = set()
    const_strings: dict[str, str] = {}
    class_const_strings: dict[tuple[str, str], str] = {}

    for node in ast.walk(tree):
        if isinstance(node, ast.Assign) and len(node.targets) == 1:
            target = node.targets[0]
            if isinstance(target, ast.Name) and isinstance(node.value, ast.Constant):
                if isinstance(node.value.value, str):
                    const_strings[target.id] = node.value.value
        if isinstance(node, ast.AnnAssign):
            target = node.target
            if isinstance(target, ast.Name) and isinstance(node.value, ast.Constant):
                if isinstance(node.value.value, str):
                    const_strings[target.id] = node.value.value
        if isinstance(node, ast.ClassDef):
            for stmt in node.body:
                if isinstance(stmt, ast.Assign) and len(stmt.targets) == 1:
                    target = stmt.targets[0]
                    if isinstance(target, ast.Name) and isinstance(
                        stmt.value, ast.Constant
                    ):
                        if isinstance(stmt.value.value, str):
                            class_const_strings[(node.name, target.id)] = stmt.value.value
                if isinstance(stmt, ast.AnnAssign):
                    target = stmt.target
                    if isinstance(target, ast.Name) and isinstance(
                        stmt.value, ast.Constant
                    ):
                        if isinstance(stmt.value.value, str):
                            class_const_strings[(node.name, target.id)] = stmt.value.value

    def _resolve_string(node: ast.AST) -> str | None:
        if isinstance(node, ast.Constant) and isinstance(node.value, str):
            return node.value
        if isinstance(node, ast.Name):
            return const_strings.get(node.id)
        if isinstance(node, ast.Attribute) and isinstance(node.value, ast.Name):
            return class_const_strings.get((node.value.id, node.attr))
        return None

    for node in ast.walk(tree):
        if isinstance(node, ast.FunctionDef):
            for dec in node.decorator_list:
                name = tag_name_from_decorator(dec, node.name)
                if (
                    isinstance(dec, ast.Call)
                    and isinstance(dec.func, ast.Attribute)
                    and dec.func.attr == "tag"
                    and dec.args
                ):
                    resolved = _resolve_string(dec.args[0])
                    if resolved:
                        name = resolved
                if name:
                    tags.add(name)
        if isinstance(node, ast.Call):
            name = tag_name_from_call(node)
            if not name:
                # `register.tag(TAG_NAME, ...)` where TAG_NAME is a module-level
                # string constant (or a class constant like `Node.TAG_NAME`).
                if (
                    isinstance(node.func, ast.Attribute)
                    and node.func.attr == "tag"
                    and len(node.args) >= 2
                ):
                    name = _resolve_string(node.args[0])
            if name:
                tags.add(name)
    return tags


def collect_registered_filters(tree: ast.AST) -> set[str]:
    filters: set[str] = set()
    for node in ast.walk(tree):
        if isinstance(node, ast.FunctionDef):
            for dec in node.decorator_list:
                name = filter_name_from_decorator(dec, node.name)
                if name:
                    filters.add(name)
        if isinstance(node, ast.Call):
            name = filter_name_from_call(node)
            if name:
                filters.add(name)
    return filters
