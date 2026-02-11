/// Find positions of a delimiter character in `s`, skipping occurrences inside
/// single- or double-quoted regions.
///
/// When `handle_escapes` is true, `\` inside a quoted region escapes the next
/// character (so `\"` does not close the quote).
///
/// The callback receives the byte index of each unquoted delimiter found.
/// Return `true` from the callback to stop early.
pub(crate) fn for_each_unquoted(
    s: &str,
    delimiter: impl Fn(char) -> bool,
    handle_escapes: bool,
    mut cb: impl FnMut(usize) -> bool,
) {
    let mut quote: Option<char> = None;
    let mut escape = false;

    for (idx, ch) in s.char_indices() {
        if escape {
            escape = false;
            continue;
        }
        match ch {
            '\\' if handle_escapes && quote.is_some() => {
                escape = true;
            }
            '"' | '\'' if quote == Some(ch) => {
                quote = None;
            }
            '"' | '\'' if quote.is_none() => {
                quote = Some(ch);
            }
            _ if quote.is_some() => {}
            _ if delimiter(ch) => {
                if cb(idx) {
                    return;
                }
            }
            _ => {}
        }
    }
}

/// Split `s` on whitespace while respecting quoted regions (with escape handling).
///
/// Returns owned strings for each whitespace-delimited token.
pub(crate) fn split_on_whitespace(s: &str) -> Vec<String> {
    let mut pieces = Vec::with_capacity((s.len() / 8).clamp(2, 8));
    let mut start = None;
    let mut quote: Option<char> = None;
    let mut escape = false;

    for (idx, ch) in s.char_indices() {
        if escape {
            escape = false;
            if start.is_none() {
                start = Some(idx.saturating_sub(1));
            }
            continue;
        }
        match ch {
            '\\' if quote.is_some() => {
                escape = true;
                if start.is_none() {
                    start = Some(idx);
                }
            }
            '"' | '\'' if quote == Some(ch) => {
                quote = None;
                if start.is_none() {
                    start = Some(idx);
                }
            }
            '"' | '\'' if quote.is_none() => {
                quote = Some(ch);
                if start.is_none() {
                    start = Some(idx);
                }
            }
            _ if quote.is_some() => {
                if start.is_none() {
                    start = Some(idx);
                }
            }
            _ if ch.is_whitespace() => {
                if let Some(s_start) = start.take() {
                    pieces.push(s[s_start..idx].to_owned());
                }
            }
            _ => {
                if start.is_none() {
                    start = Some(idx);
                }
            }
        }
    }
    if let Some(s_start) = start {
        pieces.push(s[s_start..].to_owned());
    }
    pieces
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unquoted_delimiters_found() {
        let mut positions = Vec::new();
        for_each_unquoted(
            "a|b|c",
            |ch| ch == '|',
            false,
            |idx| {
                positions.push(idx);
                false
            },
        );
        assert_eq!(positions, vec![1, 3]);
    }

    #[test]
    fn quoted_delimiters_skipped() {
        let mut positions = Vec::new();
        for_each_unquoted(
            "a|'b|c'|d",
            |ch| ch == '|',
            false,
            |idx| {
                positions.push(idx);
                false
            },
        );
        assert_eq!(positions, vec![1, 7]);
    }

    #[test]
    fn double_quotes() {
        let mut positions = Vec::new();
        for_each_unquoted(
            r#"a|"b|c"|d"#,
            |ch| ch == '|',
            false,
            |idx| {
                positions.push(idx);
                false
            },
        );
        assert_eq!(positions, vec![1, 7]);
    }

    #[test]
    fn escape_handling() {
        let mut positions = Vec::new();
        for_each_unquoted(
            r#""a\"b"|c"#,
            |ch| ch == '|',
            true,
            |idx| {
                positions.push(idx);
                false
            },
        );
        // The \" is escaped, so the quote doesn't close until the real "
        assert_eq!(positions, vec![6]);
    }

    #[test]
    fn escape_ignored_without_flag() {
        let mut positions = Vec::new();
        for_each_unquoted(
            r#""a\"b"|c"#,
            |ch| ch == '|',
            false,
            |idx| {
                positions.push(idx);
                false
            },
        );
        // Without escape handling, \" closes the quote, then b" opens a new one
        // "a\" -> quote closed at \", then b" opens, |c is outside... actually:
        // char-by-char: " opens, a inside, \ inside, " closes, b outside, " opens, | inside, c inside
        assert!(positions.is_empty());
    }

    #[test]
    fn early_stop() {
        let mut positions = Vec::new();
        for_each_unquoted(
            "a|b|c|d",
            |ch| ch == '|',
            false,
            |idx| {
                positions.push(idx);
                positions.len() >= 2
            },
        );
        assert_eq!(positions, vec![1, 3]);
    }

    #[test]
    fn split_whitespace_simple() {
        assert_eq!(
            split_on_whitespace("load i18n l10n"),
            vec!["load", "i18n", "l10n"]
        );
    }

    #[test]
    fn split_whitespace_quoted() {
        assert_eq!(
            split_on_whitespace(r#"if x == "hello world""#),
            vec!["if", "x", "==", r#""hello world""#]
        );
    }

    #[test]
    fn split_whitespace_escaped() {
        assert_eq!(
            split_on_whitespace(r#"blocktrans "it\"s fine""#),
            vec!["blocktrans", r#""it\"s fine""#]
        );
    }

    #[test]
    fn split_whitespace_empty() {
        assert!(split_on_whitespace("").is_empty());
        assert!(split_on_whitespace("   ").is_empty());
    }
}
