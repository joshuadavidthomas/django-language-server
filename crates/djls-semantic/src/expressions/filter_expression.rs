//! Parse-time checks from Django's `FilterExpression` and `Variable`.
//!
//! This is a pinned port of a language grammar, not extracted Django knowledge: the
//! `filter_raw_string` regex and the `Variable.__init__` checks in `django/template/base.py`
//! decide which text `parser.compile_filter()` accepts, and Django changes them rarely. Which
//! tag arguments reach this grammar is extracted Argument Consumption, never a table here.
//!
//! Django 6.0 changed the variable grammar for `+` and `-`: Django 5.2 matches signed numbers
//! with a separate pattern and rejects the remainder after a variable, while Django 6.0 accepts
//! `+` and `-` as variable characters that `Variable` then rejects with its own message. An
//! expression is only rejected when both grammars reject it with the same message, so a
//! diagnostic never depends on which supported Django version the project runs.

use std::ops::Range;
use std::sync::LazyLock;

use regex::Captures;
use regex::Match;
use regex::Regex;

/// One filter application in a syntactically valid filter expression.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ParsedFilter {
    pub(crate) name: String,
    /// Byte range of the filter name and argument within the expression text.
    pub(crate) range: Range<usize>,
    /// Byte range of the argument within the expression text.
    pub(crate) argument: Option<Range<usize>>,
}

/// Parse `token` as Django does at template compile time.
///
/// Returns the filters on success, `Err(message)` when every supported Django version rejects
/// the expression with the same message, and `Ok` with no filters when versions disagree.
pub(crate) fn parse_filter_expression(token: &str) -> Result<Vec<ParsedFilter>, String> {
    // Most operands are a bare variable, number, or string. Both grammars match one whole, so
    // only the `Variable` checks the grammars share apply, and the regexes are not needed.
    if !token.is_empty()
        && token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'.')
    {
        return Grammar::Legacy.check_variable(token).map(|()| Vec::new());
    }
    if is_plain_string(token) {
        return Ok(Vec::new());
    }
    let modern = Grammar::Modern.parse(token);
    let legacy = Grammar::Legacy.parse(token);
    match (modern, legacy) {
        (Ok(filters), Ok(_)) => Ok(filters),
        (Err(modern), Err(legacy)) if modern == legacy => Err(modern),
        _ => Ok(Vec::new()),
    }
}

#[derive(Clone, Copy)]
enum Grammar {
    /// Django 5.2 and earlier: `[\w.]+` variables plus a separate signed-number pattern.
    Legacy,
    /// Django 6.0 and later: `+` and `-` are variable characters that `Variable` then rejects.
    Modern,
}

/// Django's `constant_string`, given its classes for a character other than a quote or backslash
/// (`[^"\\]` and `[^'\\]`) and for any character but a newline (`.`).
fn constant(not_double: &str, not_single: &str, any: &str) -> String {
    format!(
        r#"(?:_\("{not_double}*(?:\\{any}{not_double}*)*"\)|_\('{not_single}*(?:\\{any}{not_single}*)*'\)|"{not_double}*(?:\\{any}{not_double}*)*"|'{not_single}*(?:\\{any}{not_single}*)*')"#
    )
}

/// The character classes of Python's Unicode `\w`, `\d`, and the `\s` this port uses.
struct Classes {
    letter_number: &'static str,
    digit: &'static str,
    space: &'static str,
    not_double: &'static str,
    not_single: &'static str,
    any: &'static str,
}

const UNICODE: Classes = Classes {
    letter_number: r"\p{Letter}\p{Number}",
    digit: r"\d",
    space: r"\s",
    not_double: r#"[^"\\]"#,
    not_single: r"[^'\\]",
    any: ".",
};

// On ASCII text the Unicode classes match exactly these, and compiling them is far cheaper. Most
// templates are ASCII, so the Unicode regexes are only built when a token needs them.
const ASCII: Classes = Classes {
    letter_number: "A-Za-z0-9",
    digit: "[0-9]",
    space: r"[\t\n\x0B\x0C\r ]",
    not_double: r"[\x00-\x21\x23-\x5B\x5D-\x7F]",
    not_single: r"[\x00-\x26\x28-\x5B\x5D-\x7F]",
    any: r"[\x00-\x09\x0B-\x7F]",
};

fn filter_regex(grammar: Grammar, classes: &Classes) -> Option<Regex> {
    let Classes {
        letter_number,
        digit,
        space,
        not_double,
        not_single,
        any,
    } = classes;
    let constant = constant(not_double, not_single, any);
    let var = match grammar {
        Grammar::Legacy => format!(r"[{letter_number}_.]+|[-+.]?{digit}[{digit}.e]*"),
        Grammar::Modern => format!(r"[{letter_number}_.+-]+"),
    };
    let pattern = format!(
        r"^(?P<constant>{constant})|^(?P<var>{var})|(?:{space}*\|{space}*(?P<filter_name>[{letter_number}_]+)(?::(?:(?P<constant_arg>{constant})|(?P<var_arg>{var})))?)"
    );
    Regex::new(&pattern).ok()
}

static LEGACY_REGEX: LazyLock<Option<Regex>> =
    LazyLock::new(|| filter_regex(Grammar::Legacy, &UNICODE));
static MODERN_REGEX: LazyLock<Option<Regex>> =
    LazyLock::new(|| filter_regex(Grammar::Modern, &UNICODE));
static LEGACY_ASCII_REGEX: LazyLock<Option<Regex>> =
    LazyLock::new(|| filter_regex(Grammar::Legacy, &ASCII));
static MODERN_ASCII_REGEX: LazyLock<Option<Regex>> =
    LazyLock::new(|| filter_regex(Grammar::Modern, &ASCII));

impl Grammar {
    fn regex(self, token: &str) -> Option<&'static Regex> {
        match (self, token.is_ascii()) {
            (Self::Legacy, false) => LEGACY_REGEX.as_ref(),
            (Self::Modern, false) => MODERN_REGEX.as_ref(),
            (Self::Legacy, true) => LEGACY_ASCII_REGEX.as_ref(),
            (Self::Modern, true) => MODERN_ASCII_REGEX.as_ref(),
        }
    }

    fn parse(self, token: &str) -> Result<Vec<ParsedFilter>, String> {
        let Some(regex) = self.regex(token) else {
            return Ok(Vec::new());
        };
        self.parse_with(regex, token)
    }

    fn parse_with(self, regex: &Regex, token: &str) -> Result<Vec<ParsedFilter>, String> {
        let mut filters = Vec::new();
        let mut has_var = false;
        let mut upto = 0;
        while upto < token.len() {
            let Some(captures) = regex.captures_at(token, upto) else {
                break;
            };
            let Some(matched) = captures.get(0) else {
                break;
            };
            let start = matched.start();
            if start != upto {
                return Err(format!(
                    "Could not parse some characters: {}|{}|{}",
                    &token[..upto],
                    &token[upto..start],
                    &token[start..]
                ));
            }
            if has_var {
                let Some(name) = captures.name("filter_name") else {
                    return Ok(Vec::new());
                };
                filters.push(self.filter(name, &captures)?);
            } else {
                if captures.name("constant").is_none() {
                    let Some(var) = captures.name("var") else {
                        return Err(format!("Could not find variable at start of {token}."));
                    };
                    self.check_variable(var.as_str())?;
                }
                has_var = true;
            }
            upto = matched.end();
        }
        if upto != token.len() {
            return Err(format!(
                "Could not parse the remainder: '{}' from '{token}'",
                &token[upto..]
            ));
        }
        Ok(filters)
    }

    fn filter(self, name: Match<'_>, captures: &Captures<'_>) -> Result<ParsedFilter, String> {
        let argument = captures
            .name("constant_arg")
            .or_else(|| captures.name("var_arg"));
        if let Some(var_arg) = captures.name("var_arg") {
            self.check_variable(var_arg.as_str())?;
        }
        let end = argument.map_or(name.end(), |argument| argument.end());
        Ok(ParsedFilter {
            name: name.as_str().to_string(),
            range: name.start()..end,
            argument: argument.map(|argument| argument.range()),
        })
    }

    /// The checks `Variable.__init__` applies to non-literal text.
    fn check_variable(self, var: &str) -> Result<(), String> {
        if is_python_number(var) {
            return Ok(());
        }
        if var.contains("._") || var.starts_with('_') {
            return Err(format!(
                "Variables and attributes may not begin with underscores: '{var}'"
            ));
        }
        if matches!(self, Self::Modern) {
            for c in ['+', '-'] {
                if var.contains(c) {
                    return Err(format!(
                        "Invalid character ('{c}') in variable name: '{var}'"
                    ));
                }
            }
        }
        Ok(())
    }
}

/// A quoted string without escapes or filters, which `constant_string` matches whole.
fn is_plain_string(token: &str) -> bool {
    b"\"'".iter().any(|&quote| {
        token.len() >= 2
            && token.as_bytes()[0] == quote
            && token.as_bytes()[token.len() - 1] == quote
            && !token.as_bytes()[1..token.len() - 1]
                .iter()
                .any(|&byte| byte == quote || byte == b'\\')
    })
}

/// Whether `Variable` treats `var` as a numeric literal.
fn is_python_number(var: &str) -> bool {
    if var.contains('.') || var.contains(['e', 'E']) {
        is_python_float(var) && !var.ends_with('.')
    } else {
        is_python_int(var)
    }
}

fn strip_sign(value: &str) -> &str {
    value
        .strip_prefix('+')
        .or_else(|| value.strip_prefix('-'))
        .unwrap_or(value)
}

/// Python numeric digits with single underscores between them, as `int()` and `float()` accept.
fn is_digit_part(value: &str) -> bool {
    !value.is_empty()
        && value
            .split('_')
            .all(|group| !group.is_empty() && group.chars().all(char::is_numeric))
}

fn is_python_int(value: &str) -> bool {
    is_digit_part(strip_sign(value))
}

fn is_python_float(value: &str) -> bool {
    let value = strip_sign(value);
    if ["inf", "infinity", "nan"]
        .iter()
        .any(|special| value.eq_ignore_ascii_case(special))
    {
        return true;
    }
    let (mantissa, exponent) = match value.find(['e', 'E']) {
        Some(index) => (&value[..index], Some(&value[index + 1..])),
        None => (value, None),
    };
    if exponent.is_some_and(|exponent| !is_digit_part(strip_sign(exponent))) {
        return false;
    }
    match mantissa.split_once('.') {
        Some((whole, fraction)) => {
            (whole.is_empty() || is_digit_part(whole))
                && (fraction.is_empty() || is_digit_part(fraction))
                && !(whole.is_empty() && fraction.is_empty())
        }
        None => is_digit_part(mantissa),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_variables_literals_and_filters() {
        for token in [
            "user",
            "user.name",
            "items.0",
            "\"text\"",
            "'text'",
            "_(\"text\")",
            "1.",
            "1_000",
            "-1",
            "+2",
            "1e5",
            "value|default:\"x\"|lower",
            "value | lower",
            "value|default:other.attr",
            "and",
        ] {
            assert_eq!(
                parse_filter_expression(token).map(|_| ()),
                Ok(()),
                "{token}"
            );
        }
    }

    #[test]
    fn plain_operands_skip_the_regexes_without_changing_the_result() {
        for token in [
            "user",
            "user.name",
            "items.0",
            "1",
            "1.5",
            "1.",
            "1_000",
            "1__0",
            "1e5",
            "inf",
            "_secret",
            "x._y",
            "x.",
            ".x",
            "x..y",
            "_",
            "0x10",
            "''",
            "'a b'",
            "\"it's\"",
            "'a'b'",
            "'a\\'b'",
            "'a'|lower",
            "'a",
            "'",
            "\"a'",
        ] {
            let modern = Grammar::Modern.parse(token);
            let legacy = Grammar::Legacy.parse(token);
            assert_eq!(modern, legacy, "{token}");
            assert_eq!(parse_filter_expression(token), modern, "{token}");
        }
    }

    #[test]
    fn ascii_regexes_match_the_unicode_ones_on_ascii_text() {
        for token in [
            "user|lower",
            "value | default:\"x\" |lower",
            "value|default:other.attr",
            "value|date:_(\"Y\")",
            "x|f:1.5e3",
            "x|f:-1",
            "-1.5",
            "+2|add:3",
            "a-b",
            "a+b|f",
            "x|f\t|g",
            "x |\x0Bf",
            "x\x1F|f",
            "x|9f",
            "'a\\'b'|f",
            "user|",
            "x|lower:",
            "x._y|f",
            "x|f:_y",
            "|f",
            "x||f",
            "x|f:'a'b",
        ] {
            for grammar in [Grammar::Legacy, Grammar::Modern] {
                let unicode = match grammar {
                    Grammar::Legacy => LEGACY_REGEX.as_ref(),
                    Grammar::Modern => MODERN_REGEX.as_ref(),
                }
                .expect("Unicode regex compiles");
                assert_eq!(
                    grammar.parse(token),
                    grammar.parse_with(unicode, token),
                    "{token}"
                );
            }
        }
    }

    #[test]
    fn reports_django_messages() {
        for (token, message) in [
            ("user|", "Could not parse the remainder: '|' from 'user|'"),
            (
                "x|lower:",
                "Could not parse the remainder: ':' from 'x|lower:'",
            ),
            (
                "_secret",
                "Variables and attributes may not begin with underscores: '_secret'",
            ),
            (
                "x._y",
                "Variables and attributes may not begin with underscores: 'x._y'",
            ),
            (
                "x|default:_y",
                "Variables and attributes may not begin with underscores: '_y'",
            ),
            ("\"a\"b", "Could not parse the remainder: 'b' from '\"a\"b'"),
            ("|lower", "Could not find variable at start of |lower."),
            ("x||lower", "Could not parse some characters: x||||lower"),
            ("page=", "Could not parse the remainder: '=' from 'page='"),
        ] {
            assert_eq!(parse_filter_expression(token), Err(message.to_string()));
        }
    }

    #[test]
    fn ignores_expressions_whose_validity_depends_on_django_version() {
        // Django 5.2 rejects the remainder; Django 6.0 reports an invalid character.
        assert_eq!(parse_filter_expression("a-b"), Ok(Vec::new()));
        // Django 5.2 rejects the remainder; Django 6.0 parses a float.
        assert_eq!(parse_filter_expression("1.5e-3"), Ok(Vec::new()));
    }

    #[test]
    fn records_filter_ranges() {
        assert_eq!(
            parse_filter_expression("value|default:\"x\"|lower"),
            Ok(vec![
                ParsedFilter {
                    name: "default".to_string(),
                    range: 6..17,
                    argument: Some(14..17),
                },
                ParsedFilter {
                    name: "lower".to_string(),
                    range: 18..23,
                    argument: None,
                },
            ])
        );
    }
}
