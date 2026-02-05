"""
Structural (block-aware) rule extraction from Django source.

Unlike per-tag "open tag" validation, some Django tags enforce constraints on
inner block tags. This module extracts those constraints statically from
Django's Python source, so we can validate templates without hard-coding tag
names.
"""

from __future__ import annotations

import ast
from pathlib import Path

from ..types import BlockTagSpec
from ..types import ConditionalInnerTagRule
from .files import iter_tag_files
from .registry import tag_name_from_decorator
from .registry import tag_registration_from_call


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


def _callable_key_from_name(
    func_name: str,
) -> str | tuple[str, str]:
    # Support simple class method registrations like `SomeNode.handle`.
    parts = func_name.split(".")
    if len(parts) == 2:
        return (parts[0], parts[1])
    return func_name


def _resolve_class_method(
    class_name: str,
    method_name: str,
    *,
    class_methods: dict[tuple[str, str], ast.FunctionDef],
    class_defs: dict[str, ast.ClassDef],
) -> ast.FunctionDef | None:
    """
    Resolve a method definition, following simple inheritance within the module.

    This is a best-effort helper to support patterns where tags are registered
    against `SomeSubclass.handle` but `handle` is actually defined on a base
    class (common for "block inclusion" nodes).
    """
    direct = class_methods.get((class_name, method_name))
    if direct is not None:
        return direct

    visited: set[str] = set()
    stack = [class_name]
    while stack:
        current = stack.pop()
        if current in visited:
            continue
        visited.add(current)
        cls = class_defs.get(current)
        if cls is None:
            continue
        for base in cls.bases:
            base_name: str | None = None
            if isinstance(base, ast.Name):
                base_name = base.id
            elif isinstance(base, ast.Attribute):
                # foo.Bar -> Bar
                base_name = base.attr
            if not base_name:
                continue
            fn = class_methods.get((base_name, method_name))
            if fn is not None:
                return fn
            stack.append(base_name)
    return None


def _has_classmethod_decorator(func: ast.FunctionDef) -> bool:
    for dec in func.decorator_list:
        if isinstance(dec, ast.Name) and dec.id == "classmethod":
            return True
    return False


def _has_register_simple_block_tag_decorator(func: ast.FunctionDef) -> bool:
    for dec in func.decorator_list:
        if (
            isinstance(dec, ast.Call)
            and isinstance(dec.func, ast.Name)
            and dec.func.id == "register_simple_block_tag"
        ):
            return True
    return False


def _extract_register_simple_block_tag_end_names(
    func: ast.FunctionDef,
    module_strings: dict[str, str],
    class_strings: dict[tuple[str, str], str],
) -> set[str]:
    end_names: set[str] = set()
    for dec in func.decorator_list:
        if not (
            isinstance(dec, ast.Call)
            and isinstance(dec.func, ast.Name)
            and dec.func.id == "register_simple_block_tag"
        ):
            continue
        for kw in dec.keywords:
            if kw.arg == "end_name":
                vals = _extract_const_str_list(kw.value, module_strings, class_strings)
                for v in vals:
                    cmd = v.split()[0] if v else ""
                    if cmd:
                        end_names.add(cmd)
    return end_names


def _parser_arg_name(func: ast.FunctionDef) -> str | None:
    """
    Return the variable name used for the Parser argument.

    For classmethods (e.g. `def handle(cls, parser, token)`), parser is the
    second positional argument.
    """
    if not func.args.args:
        return None
    idx = 1 if _has_classmethod_decorator(func) else 0
    if idx >= len(func.args.args):
        return None
    name = func.args.args[idx].arg

    # Instance methods typically start with `self`. For Node `__init__()` and
    # similar patterns, the parser arg is commonly the next positional
    # argument, often named `parser`.
    if not _has_classmethod_decorator(func) and name == "self":
        for arg in func.args.args[1:]:
            if arg.arg == "parser":
                return "parser"
        if func.name == "__init__" and len(func.args.args) >= 2:
            return func.args.args[1].arg

    return name


def extract_structural_rules_from_file(filepath: Path) -> list[ConditionalInnerTagRule]:
    source = filepath.read_text()
    tree = ast.parse(source)

    func_defs, class_methods = _collect_function_definitions(tree)
    class_defs = {n.name: n for n in tree.body if isinstance(n, ast.ClassDef)}
    func_to_tags: dict[str | tuple[str, str], set[str]] = {}

    for node in ast.walk(tree):
        if isinstance(node, ast.FunctionDef):
            for dec in node.decorator_list:
                tag_name = tag_name_from_decorator(dec, node.name)
                if tag_name:
                    func_to_tags.setdefault(node.name, set()).add(tag_name)
        elif isinstance(node, ast.Call):
            tag_name, func_name = tag_registration_from_call(node)
            if tag_name and func_name:
                key = _callable_key_from_name(func_name)
                func_to_tags.setdefault(key, set()).add(tag_name)

    rules: list[ConditionalInnerTagRule] = []

    for key, tag_names in func_to_tags.items():
        func: ast.FunctionDef | None
        if isinstance(key, tuple):
            func = _resolve_class_method(
                key[0],
                key[1],
                class_methods=class_methods,
                class_defs=class_defs,
            )
        else:
            func = func_defs.get(key)
        if func is None or not tag_names:
            continue
        rules.extend(_extract_conditional_inner_tag_rules(func, sorted(tag_names)))

    return rules


def extract_structural_rules_from_django(
    django_root: Path,
) -> list[ConditionalInnerTagRule]:
    all_rules: list[ConditionalInnerTagRule] = []
    for path in iter_tag_files(django_root):
        all_rules.extend(extract_structural_rules_from_file(path))
    return all_rules


def extract_block_specs_from_file(filepath: Path) -> list[BlockTagSpec]:
    """
    Extract block-tag delimiter specs for tags that use `parser.parse((...))`.

    This does not attempt to validate the full grammar/order of delimiters yet;
    it focuses on mapping:
    - which start tags introduce which delimiter tags
    - which delimiter tags count as end tags vs middle tags
    """
    source = filepath.read_text()
    tree = ast.parse(source)

    module_strings, class_strings = _collect_str_constants(tree)
    module_sequences = _collect_str_sequences(tree, module_strings, class_strings)

    func_defs, class_methods = _collect_function_definitions(tree)
    class_defs = {n.name: n for n in tree.body if isinstance(n, ast.ClassDef)}
    func_to_tags: dict[str | tuple[str, str], set[str]] = {}

    for node in ast.walk(tree):
        if isinstance(node, ast.FunctionDef):
            for dec in node.decorator_list:
                tag_name = tag_name_from_decorator(dec, node.name)
                if tag_name:
                    func_to_tags.setdefault(node.name, set()).add(tag_name)
        elif isinstance(node, ast.Call):
            tag_name, func_name = tag_registration_from_call(node)
            if tag_name and func_name:
                key = _callable_key_from_name(func_name)
                func_to_tags.setdefault(key, set()).add(tag_name)

    specs: list[BlockTagSpec] = []

    for key, tag_names in func_to_tags.items():
        func: ast.FunctionDef | None
        if isinstance(key, tuple):
            func = _resolve_class_method(
                key[0],
                key[1],
                class_methods=class_methods,
                class_defs=class_defs,
            )
        else:
            func = func_defs.get(key)
        if func is None and isinstance(key, str) and key in class_defs and tag_names:
            stop_tags = _extract_block_specs_from_tag_class(
                class_defs[key],
                module_sequences=module_sequences,
                module_strings=module_strings,
                class_strings=class_strings,
                class_defs=class_defs,
            )
            if stop_tags:
                end_tags = tuple(sorted({t for t in stop_tags if t.startswith("end")}))
                middle_tags = tuple(
                    sorted({t for t in stop_tags if not t.startswith("end")})
                )
                if end_tags or middle_tags:
                    specs.append(
                        BlockTagSpec(
                            start_tags=tuple(sorted(tag_names)),
                            end_tags=end_tags,
                            middle_tags=middle_tags,
                            repeatable_middle_tags=(),
                            terminal_middle_tags=(),
                            end_suffix_from_start_index=None,
                        )
                    )
            continue

        if func is None or not tag_names:
            continue
        stop_tags = _extract_parser_parse_stop_tags(
            func,
            module_sequences,
            module_strings,
            class_strings,
        )
        if not stop_tags:
            stop_tags = _extract_parser_parse_stop_tags_from_returned_node(
                func,
                module_sequences=module_sequences,
                module_strings=module_strings,
                class_strings=class_strings,
                class_defs=class_defs,
            )
        if stop_tags:
            end_tags = tuple(sorted({t for t in stop_tags if t.startswith("end")}))
            middle_tags = tuple(
                sorted({t for t in stop_tags if not t.startswith("end")})
            )
            if not end_tags and not middle_tags:
                continue

            repeatable, terminal = _extract_middle_tag_ordering(
                func, module_sequences, module_strings, class_strings
            )
            suffix_index = _infer_end_suffix_index(func, set(end_tags))

            specs.append(
                BlockTagSpec(
                    start_tags=tuple(sorted(tag_names)),
                    end_tags=end_tags,
                    middle_tags=middle_tags,
                    repeatable_middle_tags=tuple(
                        sorted(set(repeatable) & set(middle_tags))
                    ),
                    terminal_middle_tags=tuple(
                        sorted(set(terminal) & set(middle_tags))
                    ),
                    end_suffix_from_start_index=suffix_index,
                )
            )
            continue

        if _has_register_simple_block_tag_decorator(func):
            end_names = set(_extract_register_simple_block_tag_end_names(
                func, module_strings, class_strings
            ))
            end_names.update({f"end{name}" for name in tag_names})
            specs.append(
                BlockTagSpec(
                    start_tags=tuple(sorted(tag_names)),
                    end_tags=tuple(sorted(end_names)),
                    middle_tags=(),
                    repeatable_middle_tags=(),
                    terminal_middle_tags=(),
                    end_suffix_from_start_index=None,
                )
            )
            continue

        dynamic = _extract_dynamic_end_parse_spec(func, sorted(tag_names))
        if dynamic is not None:
            specs.append(dynamic)
            continue

        manual = _extract_manual_loop_block_spec(func, sorted(tag_names))
        if manual is not None:
            specs.append(manual)

    return specs


def extract_block_specs_from_django(django_root: Path) -> list[BlockTagSpec]:
    all_specs: list[BlockTagSpec] = []
    for path in iter_tag_files(django_root):
        all_specs.extend(extract_block_specs_from_file(path))
    return all_specs


def _extract_parser_parse_stop_tags(
    func: ast.FunctionDef,
    module_sequences: dict[str, list[str]],
    module_strings: dict[str, str],
    class_strings: dict[tuple[str, str], str],
) -> set[str]:
    parser_var = _parser_arg_name(func)
    if not parser_var:
        return set()

    local_sequences = dict(module_sequences)
    local_sequences.update(_collect_str_sequences(func, module_strings, class_strings))

    def _is_parse_receiver(value: ast.AST) -> bool:
        # Standard Django pattern: `parser.parse((...))`
        if isinstance(value, ast.Name) and value.id == parser_var:
            return True
        # classytags-like pattern: `self.parser.parse((...))`
        if (
            isinstance(value, ast.Attribute)
            and value.attr == "parser"
            and isinstance(value.value, ast.Name)
            and value.value.id == parser_var
        ):
            return True
        return False

    stop_tags: set[str] = set()
    for node in ast.walk(func):
        if not isinstance(node, ast.Call):
            continue
        if not isinstance(node.func, ast.Attribute):
            continue
        if node.func.attr != "parse":
            continue
        if not _is_parse_receiver(node.func.value):
            continue
        if not node.args:
            continue
        for raw in _resolve_str_sequence(
            node.args[0], local_sequences, module_strings, class_strings
        ):
            # Django Parser.parse() compares against `command = token.contents.split()[0]`,
            # so only the first token of a parse-until string matters.
            cmd = raw.split()[0] if raw else ""
            if cmd:
                stop_tags.add(cmd)

    return stop_tags


def _extract_parser_parse_stop_tags_from_returned_node(
    func: ast.FunctionDef,
    *,
    module_sequences: dict[str, list[str]],
    module_strings: dict[str, str],
    class_strings: dict[tuple[str, str], str],
    class_defs: dict[str, ast.ClassDef],
) -> set[str]:
    """
    Best-effort stop-tag extraction for tags implemented via Node instantiation.

    Many tags are registered as a simple function that returns a Node instance:
        @register.tag
        def thumbnail(parser, token):
            return ThumbnailNode(parser, token)

    In these cases the `parser.parse((...))` call is often in `__init__()`
    rather than the compile function. This helper follows the return value to
    a local class definition and inspects `__init__()` for `parser.parse()`.
    """
    if not func.body:
        return set()

    stop_tags: set[str] = set()
    for node in ast.walk(func):
        if not isinstance(node, ast.Return) or node.value is None:
            continue
        if not isinstance(node.value, ast.Call):
            continue
        call = node.value
        class_name: str | None = None
        if isinstance(call.func, ast.Name):
            class_name = call.func.id
        elif isinstance(call.func, ast.Attribute):
            class_name = call.func.attr
        if not class_name:
            continue
        cls = class_defs.get(class_name)
        if cls is None:
            continue
        init_fn: ast.FunctionDef | None = None
        for child in cls.body:
            if isinstance(child, ast.FunctionDef) and child.name == "__init__":
                init_fn = child
                break
        if init_fn is None:
            continue
        stop_tags |= _extract_parser_parse_stop_tags(
            init_fn,
            module_sequences,
            module_strings,
            class_strings,
        )

    return stop_tags


def _extract_manual_loop_block_spec(
    func: ast.FunctionDef,
    tag_names: list[str],
) -> BlockTagSpec | None:
    """
    Extract a BlockTagSpec from "manual loop" block parsing patterns.

    This covers patterns like Django's i18n block translate tag, which:
    - loops over tokens, consuming TEXT/VAR until a BLOCK token
    - may require a specific inner tag (e.g. `plural`) before the end tag
    - computes the end tag name dynamically: `end%s` % bits[0]
    """
    if not tag_names:
        return None
    if not _has_dynamic_end_tag_check(func):
        return None

    parser_var = _parser_arg_name(func)
    if not parser_var:
        return None
    if not _calls_parser_next_token(func, parser_var):
        return None

    inner = _find_required_inner_tag(func)
    start_tags = tuple(tag_names)
    end_tags = tuple(f"end{name}" for name in tag_names)
    middle_tags = (inner,) if inner is not None else ()

    # The "required inner tag" pattern is a terminal delimiter: after it is
    # handled, the next block tag must be the end tag.
    return BlockTagSpec(
        start_tags=start_tags,
        end_tags=end_tags,
        middle_tags=middle_tags,
        repeatable_middle_tags=(),
        terminal_middle_tags=middle_tags,
        end_suffix_from_start_index=None,
    )


def _extract_dynamic_end_parse_spec(
    func: ast.FunctionDef,
    tag_names: list[str],
) -> BlockTagSpec | None:
    """
    Detect `parser.parse((f"end{tag_name}",))`-style dynamic end tags.

    Many projects implement generic "block inclusion" nodes where:
    - `tag_name, *rest = token.split_contents()`
    - `parser.parse((f"end{tag_name}",))`

    For statically-known registrations, this implies end tags of the form
    `end<start_tag>` for each registered start tag.
    """
    if not tag_names:
        return None

    parser_var = _parser_arg_name(func)
    if not parser_var:
        return None

    for node in ast.walk(func):
        if not isinstance(node, ast.Call):
            continue
        if not isinstance(node.func, ast.Attribute) or node.func.attr != "parse":
            continue
        if (
            not isinstance(node.func.value, ast.Name)
            or node.func.value.id != parser_var
        ):
            continue
        if not node.args:
            continue
        seq = node.args[0]
        # Match parse((f"end{...}",)) or parse([f"end{...}"]).
        if not isinstance(seq, (ast.Tuple, ast.List)):
            continue
        if not seq.elts:
            continue
        first = seq.elts[0]
        if not isinstance(first, ast.JoinedStr):
            continue
        has_end_prefix = any(
            isinstance(v, ast.Constant)
            and isinstance(v.value, str)
            and v.value.startswith("end")
            for v in first.values
        )
        has_formatted = any(isinstance(v, ast.FormattedValue) for v in first.values)
        if not (has_end_prefix and has_formatted):
            continue

        start_tags = tuple(tag_names)
        end_tags = tuple(f"end{name}" for name in tag_names)
        return BlockTagSpec(
            start_tags=start_tags,
            end_tags=end_tags,
            middle_tags=(),
            repeatable_middle_tags=(),
            terminal_middle_tags=(),
            end_suffix_from_start_index=None,
        )

    return None


def _calls_parser_next_token(func: ast.FunctionDef, parser_var: str) -> bool:
    for node in ast.walk(func):
        if not isinstance(node, ast.Call):
            continue
        if not isinstance(node.func, ast.Attribute) or node.func.attr != "next_token":
            continue
        if isinstance(node.func.value, ast.Name) and node.func.value.id == parser_var:
            return True
    return False


def _extract_middle_tag_ordering(
    func: ast.FunctionDef,
    module_sequences: dict[str, list[str]],
    module_strings: dict[str, str],
    class_strings: dict[tuple[str, str], str],
) -> tuple[set[str], set[str]]:
    """
    Best-effort inference of delimiter ordering from common Django patterns.

    Targets patterns like:
    - `while token.contents.startswith("elif"):` => "elif" repeatable
    - `if token.contents == "else": nodelist = parser.parse(("endif",))` => "else" terminal
    - `if token.contents == "empty": nodelist = parser.parse(("endfor",))` => "empty" terminal

    This stays generic by extracting constants/parse-stop lists rather than
    hard-coding tag names.
    """
    parser_var = func.args.args[0].arg if func.args.args else None
    if not parser_var:
        return set(), set()

    module_sequences = dict(module_sequences)
    module_sequences.update(_collect_str_sequences(func, module_strings, class_strings))

    token_var = _detect_token_var(func, parser_var)
    if not token_var:
        return set(), set()

    repeatable: set[str] = set()
    terminal: set[str] = set()

    for node in ast.walk(func):
        if isinstance(node, ast.While):
            tag = _extract_token_contents_startswith_const(node.test, token_var)
            if tag:
                repeatable.add(tag)

        if isinstance(node, ast.If):
            tag = _extract_token_contents_eq_const(node.test, token_var)
            if not tag:
                continue
            if _body_has_end_only_parse(
                node.body,
                parser_var,
                module_sequences,
                module_strings,
                class_strings,
            ):
                terminal.add(tag)

    return repeatable, terminal


def _detect_token_var(func: ast.FunctionDef, parser_var: str) -> str | None:
    """
    Find the variable name assigned from `parser.next_token()`.
    """
    for node in ast.walk(func):
        if isinstance(node, ast.Assign) and len(node.targets) == 1:
            target = node.targets[0]
            if not isinstance(target, ast.Name):
                continue
            if (
                isinstance(node.value, ast.Call)
                and isinstance(node.value.func, ast.Attribute)
                and node.value.func.attr == "next_token"
                and isinstance(node.value.func.value, ast.Name)
                and node.value.func.value.id == parser_var
            ):
                return target.id
    return None


def _extract_token_contents_startswith_const(
    test: ast.AST, token_var: str
) -> str | None:
    # Pattern: token.contents.startswith("elif")
    if not isinstance(test, ast.Call):
        return None
    if not isinstance(test.func, ast.Attribute) or test.func.attr != "startswith":
        return None
    if not _is_token_contents_attr(test.func.value, token_var):
        return None
    if not test.args:
        return None
    arg0 = test.args[0]
    if isinstance(arg0, ast.Constant) and isinstance(arg0.value, str):
        return arg0.value
    return None


def _extract_token_contents_eq_const(test: ast.AST, token_var: str) -> str | None:
    # Pattern: token.contents == "else"
    if not isinstance(test, ast.Compare):
        return None
    if len(test.ops) != 1 or not isinstance(test.ops[0], ast.Eq):
        return None
    if len(test.comparators) != 1:
        return None
    left = test.left
    comp = test.comparators[0]
    if not _is_token_contents_attr(left, token_var):
        return None
    if isinstance(comp, ast.Constant) and isinstance(comp.value, str):
        return comp.value
    return None


def _is_token_contents_attr(node: ast.AST, token_var: str) -> bool:
    return (
        isinstance(node, ast.Attribute)
        and node.attr == "contents"
        and isinstance(node.value, ast.Name)
        and node.value.id == token_var
    )


def _body_has_end_only_parse(
    body: list[ast.stmt],
    parser_var: str,
    sequences: dict[str, list[str]],
    module_strings: dict[str, str],
    class_strings: dict[tuple[str, str], str],
) -> bool:
    # Look for `parser.parse(("endif",))`-style calls where stop tags are all end tags.
    pseudo = ast.Module(body=body, type_ignores=[])
    for node in ast.walk(pseudo):
        if not isinstance(node, ast.Call):
            continue
        if not isinstance(node.func, ast.Attribute) or node.func.attr != "parse":
            continue
        if not (
            (isinstance(node.func.value, ast.Name) and node.func.value.id == parser_var)
            or (
                isinstance(node.func.value, ast.Attribute)
                and node.func.value.attr == "parser"
                and isinstance(node.func.value.value, ast.Name)
                and node.func.value.value.id == parser_var
            )
        ):
            continue
        if not node.args:
            continue
        stop = _resolve_str_sequence(node.args[0], sequences, module_strings, class_strings)
        if stop and all(t.startswith("end") for t in stop):
            return True
    return False


def _extract_block_specs_from_tag_class(
    cls: ast.ClassDef,
    *,
    module_sequences: dict[str, list[str]],
    module_strings: dict[str, str],
    class_strings: dict[tuple[str, str], str],
    class_defs: dict[str, ast.ClassDef],
) -> set[str]:
    """
    Best-effort block spec extraction for class-registered tags.

    This targets common `classytags`-style patterns where:
    - a tag is registered via `register.tag("name", TagClass)`
    - TagClass has `options = Options(..., parser_class=SomeParser)`
    - SomeParser.parse_blocks() calls `self.parser.parse((...,))`

    We extract the `parser.parse((...))` stop tags from the parser class (and
    any static `blocks=[(...), ...]` options) to treat end/middle tags as
    structural delimiters during validation.
    """
    options_call: ast.Call | None = None
    for stmt in cls.body:
        if isinstance(stmt, ast.Assign) and len(stmt.targets) == 1:
            target = stmt.targets[0]
            if isinstance(target, ast.Name) and target.id == "options":
                if isinstance(stmt.value, ast.Call):
                    options_call = stmt.value
        if isinstance(stmt, ast.AnnAssign):
            target = stmt.target
            if isinstance(target, ast.Name) and target.id == "options":
                if isinstance(stmt.value, ast.Call):
                    options_call = stmt.value

    if options_call is None:
        return set()

    stop_tags: set[str] = set()

    def _call_name(node: ast.AST) -> str | None:
        if isinstance(node, ast.Name):
            return node.id
        if isinstance(node, ast.Attribute):
            return node.attr
        return None

    if _call_name(options_call.func) != "Options":
        return set()

    # Option blocks: `blocks=[("end_x", "nodelist"), ...]`
    blocks_expr: ast.AST | None = None
    parser_class_name: str | None = None
    for kw in options_call.keywords:
        if kw.arg == "blocks":
            blocks_expr = kw.value
        if kw.arg == "parser_class":
            if isinstance(kw.value, ast.Name):
                parser_class_name = kw.value.id
            elif isinstance(kw.value, ast.Attribute):
                parser_class_name = kw.value.attr

    if isinstance(blocks_expr, (ast.List, ast.Tuple)):
        for elt in blocks_expr.elts:
            if not isinstance(elt, (ast.Tuple, ast.List)) or not elt.elts:
                continue
            for raw in _extract_const_str_list(
                elt.elts[0], module_strings, class_strings
            ):
                cmd = raw.split()[0] if raw else ""
                if cmd:
                    stop_tags.add(cmd)

    if parser_class_name:
        parser_cls = class_defs.get(parser_class_name)
        if parser_cls is not None:
            parse_blocks: ast.FunctionDef | None = None
            for child in parser_cls.body:
                if isinstance(child, ast.FunctionDef) and child.name == "parse_blocks":
                    parse_blocks = child
                    break
            if parse_blocks is not None:
                stop_tags |= _extract_parser_parse_stop_tags(
                    parse_blocks, module_sequences, module_strings, class_strings
                )

    return stop_tags


def _collect_str_constants(
    node: ast.AST,
) -> tuple[dict[str, str], dict[tuple[str, str], str]]:
    module_strings: dict[str, str] = {}
    class_strings: dict[tuple[str, str], str] = {}
    for stmt in ast.walk(node):
        if isinstance(stmt, ast.Assign) and len(stmt.targets) == 1:
            target = stmt.targets[0]
            if isinstance(target, ast.Name) and isinstance(stmt.value, ast.Constant):
                if isinstance(stmt.value.value, str):
                    module_strings[target.id] = stmt.value.value
        if isinstance(stmt, ast.AnnAssign):
            target = stmt.target
            if isinstance(target, ast.Name) and isinstance(stmt.value, ast.Constant):
                if isinstance(stmt.value.value, str):
                    module_strings[target.id] = stmt.value.value
        if isinstance(stmt, ast.ClassDef):
            for child in stmt.body:
                if isinstance(child, ast.Assign) and len(child.targets) == 1:
                    target = child.targets[0]
                    if isinstance(target, ast.Name) and isinstance(
                        child.value, ast.Constant
                    ):
                        if isinstance(child.value.value, str):
                            class_strings[(stmt.name, target.id)] = child.value.value
                if isinstance(child, ast.AnnAssign):
                    target = child.target
                    if isinstance(target, ast.Name) and isinstance(
                        child.value, ast.Constant
                    ):
                        if isinstance(child.value.value, str):
                            class_strings[(stmt.name, target.id)] = child.value.value
    return module_strings, class_strings


def _collect_str_sequences(
    node: ast.AST,
    module_strings: dict[str, str],
    class_strings: dict[tuple[str, str], str],
) -> dict[str, list[str]]:
    """
    Collect simple `NAME = ("a", "b")` / `["a", "b"]` style assignments.
    """
    sequences: dict[str, list[str]] = {}
    for stmt in ast.walk(node):
        if not isinstance(stmt, ast.Assign) or len(stmt.targets) != 1:
            continue
        target = stmt.targets[0]
        if not isinstance(target, ast.Name):
            continue
        values = _extract_const_str_list(stmt.value, module_strings, class_strings)
        if values:
            sequences[target.id] = values
    return sequences


def _resolve_str_sequence(
    expr: ast.AST,
    sequences: dict[str, list[str]],
    module_strings: dict[str, str],
    class_strings: dict[tuple[str, str], str],
) -> list[str]:
    if isinstance(expr, ast.Name):
        return sequences.get(expr.id, [])
    return _extract_const_str_list(expr, module_strings, class_strings)


def _extract_const_str_list(
    node: ast.AST,
    module_strings: dict[str, str],
    class_strings: dict[tuple[str, str], str],
) -> list[str]:
    if isinstance(node, ast.Constant) and isinstance(node.value, str):
        return [node.value]
    if isinstance(node, ast.Name):
        v = module_strings.get(node.id)
        return [v] if v is not None else []
    if isinstance(node, ast.Attribute) and isinstance(node.value, ast.Name):
        v = class_strings.get((node.value.id, node.attr))
        return [v] if v is not None else []
    if isinstance(node, ast.BinOp) and isinstance(node.op, ast.Add):
        left = _extract_const_str_list(node.left, module_strings, class_strings)
        right = _extract_const_str_list(node.right, module_strings, class_strings)
        if len(left) == 1 and len(right) == 1:
            return [left[0] + right[0]]
    if isinstance(node, (ast.Tuple, ast.List, ast.Set)):
        values: list[str] = []
        for elt in node.elts:
            values.extend(_extract_const_str_list(elt, module_strings, class_strings))
        return values
    return []


def _infer_end_suffix_index(func: ast.FunctionDef, end_tags: set[str]) -> int | None:
    """
    Infer whether an end tag can optionally carry a suffix that must match a
    value from the start tag (e.g. `{% endblock name %}`).

    This is extracted statically from patterns like:
    - `acceptable_endblocks = ("endblock", "endblock %s" % block_name)`
    - `valid_endpartials = ("endpartialdef", f"endpartialdef {partial_name}")`
    plus best-effort mapping of `block_name`/`partial_name` to `bits[<idx>]`.
    """
    if not end_tags:
        return None

    suffix_vars: dict[str, str] = {}

    for node in ast.walk(func):
        if isinstance(node, ast.Assign) and len(node.targets) == 1:
            target = node.targets[0]
            if not isinstance(target, ast.Name):
                continue
            end_tag, var = _extract_end_tag_suffix_var(node.value, end_tags)
            if end_tag and var:
                suffix_vars[end_tag] = var

    # If we found any, map the var to a token index, else default to 1.
    for _end_tag, var in suffix_vars.items():
        idx = _infer_var_token_index(func, var)
        if idx is not None:
            return idx
        return 1

    return None


def _extract_end_tag_suffix_var(
    expr: ast.AST,
    end_tags: set[str],
) -> tuple[str | None, str | None]:
    """
    Detect patterns like:
    - ("endblock", "endblock %s" % block_name)
    - ("endpartialdef", f"endpartialdef {partial_name}")
    """
    if not isinstance(expr, (ast.Tuple, ast.List)):
        return None, None

    consts: set[str] = set()
    patterns: list[tuple[str, str]] = []

    for elt in expr.elts:
        if isinstance(elt, ast.Constant) and isinstance(elt.value, str):
            consts.add(elt.value.split()[0])
            continue
        pat = _extract_formatted_end_tag_prefix_and_var(elt)
        if pat:
            patterns.append(pat)

    for end_tag in end_tags:
        if end_tag in consts:
            for prefix, var in patterns:
                if prefix == f"{end_tag} ":
                    return end_tag, var

    return None, None


def _extract_formatted_end_tag_prefix_and_var(expr: ast.AST) -> tuple[str, str] | None:
    # "endblock %s" % block_name
    if isinstance(expr, ast.BinOp) and isinstance(expr.op, ast.Mod):
        if isinstance(expr.left, ast.Constant) and isinstance(expr.left.value, str):
            left = expr.left.value
            if "%s" in left and left.startswith("end"):
                if isinstance(expr.right, ast.Name):
                    return (left.split("%s")[0], expr.right.id)

    # f"endpartialdef {partial_name}"
    if isinstance(expr, ast.JoinedStr):
        prefix = ""
        var = None
        for part in expr.values:
            if isinstance(part, ast.Constant) and isinstance(part.value, str):
                prefix += part.value
            if isinstance(part, ast.FormattedValue):
                if isinstance(part.value, ast.Name):
                    var = part.value.id
        if var and prefix.startswith("end"):
            return prefix, var

    return None


def _infer_var_token_index(func: ast.FunctionDef, var: str) -> int | None:
    # Look for `var = bits[<idx>]` style assignments.
    for node in ast.walk(func):
        if not isinstance(node, ast.Assign) or len(node.targets) != 1:
            continue
        target = node.targets[0]
        if not isinstance(target, ast.Name) or target.id != var:
            continue
        value = node.value
        if isinstance(value, ast.Subscript):
            if isinstance(value.slice, ast.Constant) and isinstance(
                value.slice.value, int
            ):
                return value.slice.value
    return None


def _extract_conditional_inner_tag_rules(
    func: ast.FunctionDef,
    tag_names: list[str],
) -> list[ConditionalInnerTagRule]:
    """
    Extract rules of the form:
    - if "<opt>" in options: ... later expects inner tag "<inner>" inside block

    Currently targets patterns used by Django's i18n block translation tags.
    """
    option_token = _find_const_in_options_membership(func)
    if option_token is None:
        return []

    inner_tag = _find_required_inner_tag(func)
    if inner_tag is None:
        return []

    # Heuristic: if the function computes an "end%s" tag name from bits[0],
    # it's very likely that any other block tags (including the inner tag) are
    # rejected when the option isn't active. This lets us infer both:
    # - require inner tag when option is present
    # - forbid inner tag when option is absent
    has_dynamic_end_tag = _has_dynamic_end_tag_check(func)
    if not has_dynamic_end_tag:
        return []

    start_tags = tuple(tag_names)
    end_tags = tuple(f"end{name}" for name in tag_names)

    require_msg = "{tag} with '{opt}' requires a {% " + inner_tag + " %} block"
    forbid_msg = "{tag} without '{opt}' cannot include {% " + inner_tag + " %}"

    return [
        ConditionalInnerTagRule(
            start_tags=start_tags,
            end_tags=end_tags,
            inner_tag=inner_tag,
            option_token=option_token,
            require_when_option_present=True,
            message=require_msg,
        ),
        ConditionalInnerTagRule(
            start_tags=start_tags,
            end_tags=end_tags,
            inner_tag=inner_tag,
            option_token=option_token,
            require_when_option_present=False,
            message=forbid_msg,
        ),
    ]


def _find_const_in_options_membership(func: ast.FunctionDef) -> str | None:
    # Pattern: if "count" in <name>:
    for node in ast.walk(func):
        if not isinstance(node, ast.Compare):
            continue
        if len(node.ops) != 1 or not isinstance(node.ops[0], ast.In):
            continue
        if not isinstance(node.left, ast.Constant) or not isinstance(
            node.left.value, str
        ):
            continue
        if len(node.comparators) != 1 or not isinstance(node.comparators[0], ast.Name):
            continue
        return node.left.value
    return None


def _find_required_inner_tag(func: ast.FunctionDef) -> str | None:
    # Pattern:
    # if token.contents.strip() != "plural": raise TemplateSyntaxError(...)
    for node in ast.walk(func):
        if not isinstance(node, ast.If):
            continue
        test = node.test
        if not isinstance(test, ast.Compare):
            continue
        if len(test.ops) != 1 or not isinstance(test.ops[0], ast.NotEq):
            continue
        if len(test.comparators) != 1:
            continue
        comp = test.comparators[0]
        if not isinstance(comp, ast.Constant) or not isinstance(comp.value, str):
            continue
        if not _is_token_contents_strip_call(test.left):
            continue
        if _body_raises_template_syntax_error(node.body):
            return comp.value
    return None


def _has_dynamic_end_tag_check(func: ast.FunctionDef) -> bool:
    # Look for an "end%s" % bits[0] style string formatting, or a compare
    # against an end_tag_name variable.
    for node in ast.walk(func):
        if isinstance(node, ast.BinOp) and isinstance(node.op, ast.Mod):
            if isinstance(node.left, ast.Constant) and node.left.value == "end%s":
                return True
        if isinstance(node, ast.JoinedStr):
            for value in node.values:
                if isinstance(value, ast.Constant) and "end" in str(value.value):
                    return True
    for node in ast.walk(func):
        if isinstance(node, ast.Compare) and _is_token_contents_strip_call(node.left):
            for comp in node.comparators:
                if isinstance(comp, ast.Name) and comp.id == "end_tag_name":
                    return True
    return False


def _is_token_contents_strip_call(node: ast.AST) -> bool:
    # token.contents.strip()
    if not isinstance(node, ast.Call):
        return False
    if not isinstance(node.func, ast.Attribute) or node.func.attr != "strip":
        return False
    value = node.func.value
    return (
        isinstance(value, ast.Attribute)
        and value.attr == "contents"
        and isinstance(value.value, ast.Name)
        and value.value.id == "token"
    )


def _body_raises_template_syntax_error(body: list[ast.stmt]) -> bool:
    for stmt in body:
        if not isinstance(stmt, ast.Raise):
            continue
        exc = stmt.exc
        if isinstance(exc, ast.Call):
            fn = exc.func
            if isinstance(fn, ast.Name) and fn.id == "TemplateSyntaxError":
                return True
            if isinstance(fn, ast.Attribute) and fn.attr == "TemplateSyntaxError":
                return True
    return False
