"""Opaque-block extraction.

Detects template tags that skip parsing their inner content (e.g. via
`parser.skip_past(...)` or manual token loops).
"""

from __future__ import annotations

import ast
from pathlib import Path

from ..types import OpaqueBlockSpec
from ..types import merge_opaque_blocks
from .files import iter_tag_files
from .registry import tag_name_from_decorator
from .registry import tag_registration_from_call


class OpaqueTagExtractor:
    """
    Extract tags that skip parsing of their inner content (opaque blocks).

    Detects patterns like:
    - parser.skip_past("endtag")
    - manual token loops that only scan for an end tag
    """

    def __init__(self, source: str, file_path: str):
        self.source = source
        self.file_path = file_path
        self.func_defs: dict[str, ast.FunctionDef] = {}
        self.decorator_tags: dict[str, set[str]] = {}
        self.registered_tags: dict[str, set[str]] = {}

    def collect_functions(self, tree: ast.AST) -> None:
        for node in ast.walk(tree):
            if isinstance(node, ast.FunctionDef):
                self.func_defs[node.name] = node

    def collect_registrations(self, tree: ast.Module) -> None:
        for node in ast.walk(tree):
            if isinstance(node, ast.FunctionDef):
                for dec in node.decorator_list:
                    name = tag_name_from_decorator(dec, node.name)
                    if name:
                        self.decorator_tags.setdefault(node.name, set()).add(name)

        for node in tree.body:
            if isinstance(node, ast.Expr) and isinstance(node.value, ast.Call):
                tag_name, func_name = tag_registration_from_call(node.value)
                if tag_name and func_name:
                    self.registered_tags.setdefault(func_name, set()).add(tag_name)

    def extract(self) -> dict[str, OpaqueBlockSpec]:
        opaque: dict[str, OpaqueBlockSpec] = {}
        func_to_tags: dict[str, set[str]] = {}
        for func_name, tags in self.decorator_tags.items():
            func_to_tags.setdefault(func_name, set()).update(tags)
        for func_name, tags in self.registered_tags.items():
            func_to_tags.setdefault(func_name, set()).update(tags)

        for func_name, tags in func_to_tags.items():
            func = self.func_defs.get(func_name)
            if not func or not tags:
                continue

            parser_var = self._parser_arg_name(func)
            if not parser_var:
                continue

            spec = self._detect_opaque(func, parser_var)
            if spec is None:
                continue

            for tag_name in tags:
                existing = opaque.get(tag_name)
                if existing is None:
                    opaque[tag_name] = spec
                else:
                    merged = sorted(set(existing.end_tags + spec.end_tags))
                    opaque[tag_name] = OpaqueBlockSpec(
                        end_tags=merged,
                        match_suffix=existing.match_suffix or spec.match_suffix,
                        kind=existing.kind or spec.kind,
                    )
        return opaque

    def _parser_arg_name(self, func: ast.FunctionDef) -> str | None:
        if not func.args.args:
            return None
        return func.args.args[0].arg

    def _detect_opaque(
        self, func: ast.FunctionDef, parser_var: str
    ) -> OpaqueBlockSpec | None:
        constants = self._collect_str_constants(func)
        end_tags: list[str] = []

        for node in ast.walk(func):
            if isinstance(node, ast.Call) and self._is_parser_call(
                node, parser_var, "skip_past"
            ):
                if node.args:
                    end_tags.extend(self._resolve_end_tags(node.args[0], constants))

        if end_tags:
            return OpaqueBlockSpec(
                end_tags=sorted(set(end_tags)),
                kind="skip_past",
            )

        loop_end_tags = self._detect_manual_opaque(func, parser_var, constants)
        if loop_end_tags:
            return OpaqueBlockSpec(
                end_tags=sorted(set(loop_end_tags)),
                kind="manual_loop",
            )
        return None

    def _collect_str_constants(self, func: ast.FunctionDef) -> dict[str, str]:
        constants: dict[str, str] = {}
        for node in ast.walk(func):
            if isinstance(node, ast.Assign) and len(node.targets) == 1:
                target = node.targets[0]
                if isinstance(target, ast.Name) and isinstance(
                    node.value, ast.Constant
                ):
                    if isinstance(node.value.value, str):
                        constants[target.id] = node.value.value
        return constants

    def _resolve_end_tags(self, node: ast.AST, constants: dict[str, str]) -> list[str]:
        if isinstance(node, ast.Constant) and isinstance(node.value, str):
            return [node.value]
        if isinstance(node, ast.Name) and node.id in constants:
            return [constants[node.id]]
        if isinstance(node, (ast.List, ast.Tuple)):
            tags = []
            for elt in node.elts:
                tags.extend(self._resolve_end_tags(elt, constants))
            return tags
        return []

    def _is_parser_call(self, node: ast.Call, parser_var: str, attr: str) -> bool:
        if isinstance(node.func, ast.Attribute):
            if node.func.attr == attr and isinstance(node.func.value, ast.Name):
                return node.func.value.id == parser_var
        return False

    def _detect_manual_opaque(
        self,
        func: ast.FunctionDef,
        parser_var: str,
        constants: dict[str, str],
    ) -> list[str]:
        end_tags: list[str] = []
        for node in ast.walk(func):
            if not isinstance(node, (ast.For, ast.While)):
                continue
            token_var = self._loop_token_var(node, parser_var)
            if not token_var:
                continue
            matches = self._find_endtag_compares(node, token_var, constants)
            if not matches:
                continue
            if self._token_used_only_for_endtag(node, token_var, matches):
                end_tags.extend(matches)
        return end_tags

    def _loop_token_var(self, node: ast.AST, parser_var: str) -> str | None:
        if isinstance(node, ast.For):
            if isinstance(node.iter, ast.Attribute):
                if (
                    isinstance(node.iter.value, ast.Name)
                    and node.iter.value.id == parser_var
                ):
                    if node.iter.attr == "tokens" and isinstance(node.target, ast.Name):
                        return node.target.id

        for child in ast.walk(node):
            if isinstance(child, ast.Assign) and len(child.targets) == 1:
                target = child.targets[0]
                if not isinstance(target, ast.Name):
                    continue
                if isinstance(child.value, ast.Call) and self._is_parser_call(
                    child.value, parser_var, "next_token"
                ):
                    return target.id
        return None

    def _find_endtag_compares(
        self,
        node: ast.AST,
        token_var: str,
        constants: dict[str, str],
    ) -> list[str]:
        end_tags: list[str] = []
        for child in ast.walk(node):
            if isinstance(child, ast.Compare):
                end_tags.extend(
                    self._extract_endtag_from_compare(child, token_var, constants)
                )
            if isinstance(child, ast.BoolOp):
                for val in child.values:
                    if isinstance(val, ast.Compare):
                        end_tags.extend(
                            self._extract_endtag_from_compare(val, token_var, constants)
                        )
        return end_tags

    def _extract_endtag_from_compare(
        self,
        node: ast.Compare,
        token_var: str,
        constants: dict[str, str],
    ) -> list[str]:
        if not node.ops or not node.comparators:
            return []
        op = node.ops[0]
        if not isinstance(op, (ast.Eq, ast.In)):
            return []
        left = node.left
        right = node.comparators[0]

        if self._is_token_contents(left, token_var):
            return self._resolve_end_tags(right, constants)
        if self._is_token_contents(right, token_var):
            return self._resolve_end_tags(left, constants)
        return []

    def _is_token_contents(self, node: ast.AST, token_var: str) -> bool:
        if isinstance(node, ast.Attribute):
            return (
                isinstance(node.value, ast.Name)
                and node.value.id == token_var
                and node.attr == "contents"
            )
        if isinstance(node, ast.Call):
            if isinstance(node.func, ast.Attribute) and node.func.attr == "strip":
                return self._is_token_contents(node.func.value, token_var)
        return False

    def _token_used_only_for_endtag(
        self,
        node: ast.AST,
        token_var: str,
        end_tags: list[str],
    ) -> bool:
        allowed_nodes: set[int] = set()

        for child in ast.walk(node):
            if isinstance(child, ast.Compare):
                if self._compare_is_allowed(child, token_var, end_tags):
                    for sub in ast.walk(child):
                        allowed_nodes.add(id(sub))
            if isinstance(child, ast.BoolOp):
                for val in child.values:
                    if isinstance(val, ast.Compare) and self._compare_is_allowed(
                        val, token_var, end_tags
                    ):
                        for sub in ast.walk(val):
                            allowed_nodes.add(id(sub))

        for child in ast.walk(node):
            if isinstance(child, ast.Assign):
                for target in child.targets:
                    if isinstance(target, ast.Name) and target.id == token_var:
                        allowed_nodes.add(id(target))
            if isinstance(child, ast.For):
                if isinstance(child.target, ast.Name) and child.target.id == token_var:
                    allowed_nodes.add(id(child.target))

        usage = _TokenUsageVisitor(token_var, allowed_nodes)
        usage.visit(node)
        return not usage.unsafe

    def _compare_is_allowed(
        self, node: ast.Compare, token_var: str, end_tags: list[str]
    ) -> bool:
        if not node.ops or not node.comparators:
            return False
        op = node.ops[0]
        left = node.left
        right = node.comparators[0]

        if isinstance(op, ast.In):
            if self._is_token_contents(left, token_var):
                return True
            if self._is_token_type(left, token_var):
                return True

        if isinstance(op, ast.Eq):
            if self._is_token_contents(left, token_var) or self._is_token_contents(
                right, token_var
            ):
                return True
            if self._is_token_type(left, token_var) or self._is_token_type(
                right, token_var
            ):
                return True

        return False

    def _is_token_type(self, node: ast.AST, token_var: str) -> bool:
        return (
            isinstance(node, ast.Attribute)
            and isinstance(node.value, ast.Name)
            and node.value.id == token_var
            and node.attr == "token_type"
        )


class _TokenUsageVisitor(ast.NodeVisitor):
    def __init__(self, token_var: str, allowed_nodes: set[int]):
        self.token_var = token_var
        self.allowed_nodes = allowed_nodes
        self.unsafe = False

    def visit_Name(self, node: ast.Name) -> None:
        if node.id == self.token_var and id(node) not in self.allowed_nodes:
            self.unsafe = True

    def visit_Attribute(self, node: ast.Attribute) -> None:
        if isinstance(node.value, ast.Name) and node.value.id == self.token_var:
            if id(node) not in self.allowed_nodes:
                self.unsafe = True
        self.generic_visit(node)


# =============================================================================
def extract_opaque_blocks_from_file(filepath: Path) -> dict[str, OpaqueBlockSpec]:
    source = filepath.read_text()
    tree = ast.parse(source)
    extractor = OpaqueTagExtractor(source, str(filepath))
    extractor.collect_functions(tree)
    extractor.collect_registrations(tree)
    return extractor.extract()


def extract_opaque_blocks_from_django(django_root: Path) -> dict[str, OpaqueBlockSpec]:
    all_opaque: dict[str, OpaqueBlockSpec] = {}
    for filepath in iter_tag_files(django_root):
        if filepath.exists():
            all_opaque = merge_opaque_blocks(
                all_opaque, extract_opaque_blocks_from_file(filepath)
            )

    # Also detect lexer-level verbatim-style tags.
    all_opaque = merge_opaque_blocks(
        all_opaque, _extract_lexer_opaque_blocks(django_root)
    )

    return all_opaque


def _extract_lexer_opaque_blocks(django_root: Path) -> dict[str, OpaqueBlockSpec]:
    base_path = django_root / "template" / "base.py"
    if not base_path.exists():
        return {}
    source = base_path.read_text()
    tree = ast.parse(source)
    tags: dict[str, OpaqueBlockSpec] = {}

    for node in ast.walk(tree):
        if isinstance(node, ast.Compare) and node.ops:
            if not isinstance(node.ops[0], ast.In):
                continue
            if not _is_content_prefix_slice(node.left):
                continue
            values = _extract_const_strs(node.comparators[0])
            for val in values:
                tag = val.split()[0]
                if tag:
                    tags[tag] = OpaqueBlockSpec(
                        end_tags=[f"end{tag}"],
                        match_suffix=True,
                        kind="lexer_verbatim",
                    )
    return tags


def _is_content_prefix_slice(node: ast.AST) -> bool:
    # Match content[:N]
    if isinstance(node, ast.Subscript) and isinstance(node.value, ast.Name):
        if node.value.id != "content":
            return False
        if isinstance(node.slice, ast.Slice):
            return isinstance(node.slice.upper, ast.Constant)
    return False


def _extract_const_strs(node: ast.AST) -> list[str]:
    if isinstance(node, ast.Constant) and isinstance(node.value, str):
        return [node.value]
    if isinstance(node, (ast.Tuple, ast.List)):
        vals = []
        for elt in node.elts:
            vals.extend(_extract_const_strs(elt))
        return vals
    return []
