/// Tracks quote state while iterating through characters.
///
/// Handles single and double quotes, with optional backslash escaping.
/// Used to split strings on delimiters while respecting quoted sections.
pub(crate) struct QuoteTracker {
    quote: Option<char>,
    escape: bool,
}

impl QuoteTracker {
    pub(crate) fn new() -> Self {
        Self {
            quote: None,
            escape: false,
        }
    }

    /// Update state for the given character.
    ///
    /// Returns `true` if the character is outside quotes and not part of
    /// quote or escape syntax — i.e., the character is "actionable" for
    /// delimiter checking by the caller.
    ///
    /// When `handle_escapes` is true, a `\` inside quotes starts an escape
    /// sequence and the following character is consumed.
    pub(crate) fn process(&mut self, ch: char, handle_escapes: bool) -> bool {
        if self.escape {
            self.escape = false;
            return false;
        }
        match ch {
            '\\' if handle_escapes && self.quote.is_some() => {
                self.escape = true;
                false
            }
            '"' | '\'' if self.quote == Some(ch) => {
                self.quote = None;
                false
            }
            '"' | '\'' if self.quote.is_none() => {
                self.quote = Some(ch);
                false
            }
            _ if self.quote.is_some() => false,
            _ => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unquoted_characters_are_actionable() {
        let mut qt = QuoteTracker::new();
        assert!(qt.process('a', false));
        assert!(qt.process(' ', false));
        assert!(qt.process('|', false));
    }

    #[test]
    fn test_quoted_characters_not_actionable() {
        let mut qt = QuoteTracker::new();
        assert!(!qt.process('\'', false)); // open quote
        assert!(!qt.process('a', false)); // inside
        assert!(!qt.process('|', false)); // inside, delimiter not actionable
        assert!(!qt.process('\'', false)); // close quote
        assert!(qt.process('b', false)); // outside again
    }

    #[test]
    fn test_double_quotes() {
        let mut qt = QuoteTracker::new();
        assert!(!qt.process('"', false));
        assert!(!qt.process('x', false));
        assert!(!qt.process('"', false));
        assert!(qt.process('y', false));
    }

    #[test]
    fn test_escape_handling() {
        let mut qt = QuoteTracker::new();
        assert!(!qt.process('"', true)); // open
        assert!(!qt.process('\\', true)); // escape start
        assert!(!qt.process('"', true)); // escaped, NOT a close
        assert!(!qt.process('a', true)); // still inside
        assert!(!qt.process('"', true)); // actual close
        assert!(qt.process('b', true)); // outside
    }

    #[test]
    fn test_escape_ignored_without_flag() {
        let mut qt = QuoteTracker::new();
        assert!(!qt.process('"', false)); // open
        assert!(!qt.process('\\', false)); // NOT an escape without flag
        assert!(!qt.process('"', false)); // closes the quote
        assert!(qt.process('a', false)); // outside
    }

    #[test]
    fn test_escape_outside_quotes_is_actionable() {
        let mut qt = QuoteTracker::new();
        assert!(qt.process('\\', true)); // outside quotes, backslash is just a char
    }

    #[test]
    fn test_mismatched_quotes() {
        let mut qt = QuoteTracker::new();
        assert!(!qt.process('\'', false)); // open single
        assert!(!qt.process('"', false)); // double inside single, not actionable
        assert!(!qt.process('\'', false)); // close single
        assert!(qt.process('x', false)); // outside
    }
}
