//! Parse-time checks from Django's `FilterExpression` and `Variable`.
//!
//! Django compiles each expression-taking tag argument with `parser.compile_filter()`, which
//! rejects malformed text before rendering. Django 6.0 changed the variable grammar for `+` and
//! `-`, so an expression is only rejected when both grammars reject it with the same message.

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

const CONSTANT: &str = r#"(?:_\("[^"\\]*(?:\\.[^"\\]*)*"\)|_\('[^'\\]*(?:\\.[^'\\]*)*'\)|"[^"\\]*(?:\\.[^"\\]*)*"|'[^'\\]*(?:\\.[^'\\]*)*')"#;
// Python's Unicode `\w`.
const WORD: &str = r"[\p{Letter}\p{Number}_]";

fn filter_regex(var: &str) -> Option<Regex> {
    let pattern = format!(
        r"^(?P<constant>{CONSTANT})|^(?P<var>{var})|(?:\s*\|\s*(?P<filter_name>{WORD}+)(?::(?:(?P<constant_arg>{CONSTANT})|(?P<var_arg>{var})))?)"
    );
    Regex::new(&pattern).ok()
}

static LEGACY_REGEX: LazyLock<Option<Regex>> =
    LazyLock::new(|| filter_regex(r"[\p{Letter}\p{Number}_.]+|[-+.]?\d[\d.e]*"));
static MODERN_REGEX: LazyLock<Option<Regex>> =
    LazyLock::new(|| filter_regex(r"[\p{Letter}\p{Number}_.+-]+"));

impl Grammar {
    fn regex(self) -> Option<&'static Regex> {
        match self {
            Self::Legacy => LEGACY_REGEX.as_ref(),
            Self::Modern => MODERN_REGEX.as_ref(),
        }
    }

    fn parse(self, token: &str) -> Result<Vec<ParsedFilter>, String> {
        let Some(regex) = self.regex() else {
            return Ok(Vec::new());
        };
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
