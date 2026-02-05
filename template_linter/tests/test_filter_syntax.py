"""Unit tests for filter expression parsing."""

from __future__ import annotations

from template_linter.template_syntax.filter_syntax import (
    _extract_filter_exprs_from_token,
)
from template_linter.template_syntax.filter_syntax import _parse_filter_chain
from template_linter.template_syntax.filter_syntax import _split_first_unquoted
from template_linter.template_syntax.filter_syntax import _split_unquoted


class TestSplitUnquoted:
    """Tests for _split_unquoted - splits on separator outside quotes."""

    def test_simple_split(self):
        assert _split_unquoted("a|b|c", "|") == ["a", "b", "c"]

    def test_no_separator(self):
        assert _split_unquoted("abc", "|") == ["abc"]

    def test_empty_string(self):
        assert _split_unquoted("", "|") == [""]

    def test_preserves_quoted_separator_single(self):
        assert _split_unquoted("a|'b|c'|d", "|") == ["a", "'b|c'", "d"]

    def test_preserves_quoted_separator_double(self):
        assert _split_unquoted('a|"b|c"|d', "|") == ["a", '"b|c"', "d"]

    def test_nested_quotes(self):
        # Single quotes inside double quotes
        assert _split_unquoted("""a|"b'c"|d""", "|") == ["a", """"b'c\"""", "d"]

    def test_empty_parts(self):
        assert _split_unquoted("a||b", "|") == ["a", "", "b"]

    def test_trailing_separator(self):
        assert _split_unquoted("a|b|", "|") == ["a", "b", ""]


class TestSplitFirstUnquoted:
    """Tests for _split_first_unquoted - splits on first unquoted occurrence."""

    def test_simple_split(self):
        assert _split_first_unquoted("key:value", ":") == ("key", "value")

    def test_no_separator(self):
        assert _split_first_unquoted("keyvalue", ":") == ("keyvalue", None)

    def test_multiple_separators(self):
        # Only splits on first
        assert _split_first_unquoted("a:b:c", ":") == ("a", "b:c")

    def test_preserves_quoted_separator(self):
        assert _split_first_unquoted("'a:b':c", ":") == ("'a:b'", "c")

    def test_quoted_after_separator(self):
        assert _split_first_unquoted("key:'val:ue'", ":") == ("key", "'val:ue'")

    def test_empty_value(self):
        assert _split_first_unquoted("key:", ":") == ("key", "")


class TestParseFilterChain:
    """Tests for _parse_filter_chain - parses filter expressions."""

    def test_no_filters(self):
        assert _parse_filter_chain("value") == []

    def test_single_filter_no_arg(self):
        assert _parse_filter_chain("value|upper") == [("upper", False)]

    def test_single_filter_with_arg(self):
        assert _parse_filter_chain('value|add:"1"') == [("add", True)]

    def test_filter_chain(self):
        assert _parse_filter_chain('value|add:"1"|upper') == [
            ("add", True),
            ("upper", False),
        ]

    def test_filter_arg_with_pipe(self):
        # Pipe inside quoted arg should not split
        assert _parse_filter_chain('value|default:"a|b"') == [("default", True)]

    def test_filter_arg_with_colon(self):
        assert _parse_filter_chain('value|date:"H:i:s"') == [("date", True)]

    def test_empty_filter_name_skipped(self):
        # Malformed but shouldn't crash
        assert _parse_filter_chain("value||upper") == [("upper", False)]

    def test_whitespace_handling(self):
        assert _parse_filter_chain("value | upper | lower") == [
            ("upper", False),
            ("lower", False),
        ]

    def test_arg_with_empty_value(self):
        # `filter:` with nothing after colon - has_arg should be False
        assert _parse_filter_chain("value|default:") == [("default", False)]

    def test_arg_with_whitespace_only(self):
        assert _parse_filter_chain("value|default:   ") == [("default", False)]

    def test_complex_chain(self):
        expr = 'items|first|default:"none"|truncatewords:30|upper'
        assert _parse_filter_chain(expr) == [
            ("first", False),
            ("default", True),
            ("truncatewords", True),
            ("upper", False),
        ]


class TestExtractFilterExprsFromToken:
    """Tests for _extract_filter_exprs_from_token - extracts filter expressions from tag tokens."""

    def test_no_pipe_returns_empty(self):
        assert _extract_filter_exprs_from_token("value") == []

    def test_simple_filter_returns_token(self):
        assert _extract_filter_exprs_from_token("value|upper") == ["value|upper"]

    def test_kwarg_extracts_value(self):
        # key=value|filter should return just the value part
        assert _extract_filter_exprs_from_token('foo=bar|default:"x"') == [
            'bar|default:"x"'
        ]

    def test_equality_operator_not_kwarg(self):
        # == should not be treated as kwarg assignment
        assert _extract_filter_exprs_from_token("x==y|upper") == ["x==y|upper"]

    def test_inequality_operator_not_kwarg(self):
        assert _extract_filter_exprs_from_token("x!=y|upper") == ["x!=y|upper"]

    def test_gte_operator_not_kwarg(self):
        assert _extract_filter_exprs_from_token("x>=y|upper") == ["x>=y|upper"]

    def test_lte_operator_not_kwarg(self):
        assert _extract_filter_exprs_from_token("x<=y|upper") == ["x<=y|upper"]

    def test_kwarg_with_no_identifier_key(self):
        # If key isn't a valid identifier, return full token
        assert _extract_filter_exprs_from_token("123=value|upper") == [
            "123=value|upper"
        ]

    def test_double_equals_after_value(self):
        # key=value==something - the ==something part stays
        token = "foo=bar==baz|upper"
        result = _extract_filter_exprs_from_token(token)
        # Should not split on = since value starts looking like comparison
        assert result == [token]
