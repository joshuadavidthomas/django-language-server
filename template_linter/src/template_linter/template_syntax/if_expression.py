"""
Static syntax validation for `{% if %}` / `{% elif %}` expressions.

This mirrors Django's `smartif.py` parser enough to catch compile-time
expression syntax errors (operator/operand placement, dangling operators,
unused trailing tokens).

It intentionally does *not* attempt to validate the syntax of individual
operands (which are parsed by Django's `compile_filter()` at runtime).
"""

from __future__ import annotations


class _TokenBase:
    id: str | None = None
    lbp: int = 0

    def nud(self, parser: _IfExpressionParser) -> _TokenBase:
        raise parser.error_class(
            f"Not expecting '{self.id}' in this position in if tag."
        )

    def led(self, left: _TokenBase, parser: _IfExpressionParser) -> _TokenBase:
        raise parser.error_class(
            f"Not expecting '{self.id}' as infix operator in if tag."
        )

    def display(self) -> str:
        return str(self.id)


class _Literal(_TokenBase):
    id = "literal"
    lbp = 0

    def __init__(self, text: str) -> None:
        self.text = text

    def display(self) -> str:
        return self.text

    def nud(self, parser: _IfExpressionParser) -> _TokenBase:
        return self


class _EndToken(_TokenBase):
    lbp = 0

    def nud(self, parser: _IfExpressionParser) -> _TokenBase:
        raise parser.error_class("Unexpected end of expression in if tag.")


_END = _EndToken()


def _infix(bp: int) -> type[_TokenBase]:
    class _Op(_TokenBase):
        lbp = bp

        def led(self, left: _TokenBase, parser: _IfExpressionParser) -> _TokenBase:
            # Consume a RHS expression at this binding power.
            parser.expression(bp)
            return self

    return _Op


def _prefix(bp: int) -> type[_TokenBase]:
    class _Op(_TokenBase):
        lbp = bp

        def nud(self, parser: _IfExpressionParser) -> _TokenBase:
            parser.expression(bp)
            return self

    return _Op


# Operator precedence matches Django's `smartif.py` (and Python).
_OPERATORS: dict[str, type[_TokenBase]] = {
    "or": _infix(6),
    "and": _infix(7),
    "not": _prefix(8),
    "in": _infix(9),
    "not in": _infix(9),
    "is": _infix(10),
    "is not": _infix(10),
    "==": _infix(10),
    "!=": _infix(10),
    ">": _infix(10),
    ">=": _infix(10),
    "<": _infix(10),
    "<=": _infix(10),
}

for _op, _cls in _OPERATORS.items():
    _cls.id = _op


class _IfExpressionParser:
    error_class = ValueError

    def __init__(self, tokens: list[str]) -> None:
        mapped: list[_TokenBase] = []
        i = 0
        while i < len(tokens):
            token = tokens[i]
            if token == "is" and i + 1 < len(tokens) and tokens[i + 1] == "not":
                token = "is not"
                i += 1
            elif token == "not" and i + 1 < len(tokens) and tokens[i + 1] == "in":
                token = "not in"
                i += 1
            mapped.append(self._translate_token(token))
            i += 1

        self.tokens = mapped
        self.pos = 0
        self.current_token = self._next_token()

    def _translate_token(self, token: str) -> _TokenBase:
        op = _OPERATORS.get(token)
        if op is None:
            return _Literal(token)
        return op()

    def _next_token(self) -> _TokenBase:
        if self.pos >= len(self.tokens):
            return _END
        tok = self.tokens[self.pos]
        self.pos += 1
        return tok

    def parse(self) -> None:
        self.expression()
        if self.current_token is not _END:
            raise self.error_class(
                f"Unused '{self.current_token.display()}' at end of if expression."
            )

    def expression(self, rbp: int = 0) -> _TokenBase:
        t = self.current_token
        self.current_token = self._next_token()
        left = t.nud(self)
        while rbp < self.current_token.lbp:
            t = self.current_token
            self.current_token = self._next_token()
            left = t.led(left, self)
        return left


def validate_if_expression(tokens: list[str]) -> str | None:
    """
    Validate expression tokens for `{% if %}` / `{% elif %}`.

    Returns an error message matching Django's style, or None if valid.
    """
    try:
        _IfExpressionParser(tokens).parse()
    except ValueError as e:
        return str(e)
    return None
