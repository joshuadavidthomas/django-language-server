"""
Filter extraction and validation support.

Static extraction of filter signatures from Django source code.
"""

from __future__ import annotations

import ast
from dataclasses import dataclass
from pathlib import Path

from .files import iter_filter_files
from .registry import filter_name_from_decorator
from .registry import filter_registration_from_call


@dataclass
class FilterSpec:
    name: str
    pos_args: int
    defaults: int
    file_path: str = ""
    unrestricted: bool = False


class FilterExtractor(ast.NodeVisitor):
    def __init__(self, source: str, file_path: str):
        self.source = source
        self.file_path = file_path
        self.func_defs: dict[str, ast.FunctionDef] = {}
        self.filters: dict[str, FilterSpec] = {}
        self._in_function = False

    def collect_functions(self, tree: ast.AST) -> None:
        for node in ast.walk(tree):
            if isinstance(node, ast.FunctionDef):
                self.func_defs[node.name] = node

    def visit_FunctionDef(self, node: ast.FunctionDef):
        old = self._in_function
        self._in_function = True
        # decorator-based registration
        for dec in node.decorator_list:
            name = filter_name_from_decorator(dec, node.name)
            if name:
                self._add_filter(name, node)
        self.generic_visit(node)
        self._in_function = old

    def visit_Call(self, node: ast.Call):
        # only consider module-level register.filter() calls
        if not self._in_function:
            name, func_name = filter_registration_from_call(node)
            if name:
                func = self.func_defs.get(func_name) if func_name else None
                if func is not None:
                    self._add_filter(name, func)
                else:
                    self._add_filter_stub(name)
        self.generic_visit(node)

    def _add_filter(self, name: str, func: ast.FunctionDef) -> None:
        posonly = [a.arg for a in getattr(func.args, "posonlyargs", [])]
        args = [a.arg for a in func.args.args]
        params = posonly + args
        defaults = func.args.defaults
        spec = FilterSpec(
            name=name,
            pos_args=len(params),
            defaults=len(defaults or []),
            file_path=self.file_path,
            unrestricted=False,
        )
        self.filters.setdefault(name, spec)

    def _add_filter_stub(self, name: str) -> None:
        """
        Register a filter name when we can't statically resolve the function.

        This prevents "unknown filter" noise in strict corpus validation while
        avoiding incorrect argument-count enforcement.
        """
        spec = FilterSpec(
            name=name,
            pos_args=0,
            defaults=0,
            file_path=self.file_path,
            unrestricted=True,
        )
        self.filters.setdefault(name, spec)


def extract_filters_from_file(filepath: Path) -> dict[str, FilterSpec]:
    source = filepath.read_text()
    tree = ast.parse(source)
    extractor = FilterExtractor(source, str(filepath))
    extractor.collect_functions(tree)
    extractor.visit(tree)
    return extractor.filters


def extract_filters_from_django(django_root: Path) -> dict[str, FilterSpec]:
    all_filters: dict[str, FilterSpec] = {}
    for path in iter_filter_files(django_root):
        if path.exists():
            all_filters.update(extract_filters_from_file(path))
    return all_filters
