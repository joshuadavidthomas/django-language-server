use similar::ChangeTag;
use similar::TextDiff;

#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
#[error("edit start ({start_line}:{start_char}) is after end ({end_line}:{end_char})")]
pub struct InvalidEditRange {
    start_line: u32,
    start_char: u32,
    end_line: u32,
    end_char: u32,
}

/// A text edit representing a replacement of a range in the original document.
///
/// Uses zero-indexed line/character positions, matching LSP conventions so
/// conversion at the server boundary is a trivial field copy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    start_line: u32,
    start_char: u32,
    end_line: u32,
    end_char: u32,
    new_text: String,
}

impl Edit {
    /// Create a text edit replacing the range `[start, end)` with `new_text`.
    ///
    /// Returns an error if the start position is after the end position.
    pub fn new(
        start_line: u32,
        start_char: u32,
        end_line: u32,
        end_char: u32,
        new_text: String,
    ) -> Result<Self, InvalidEditRange> {
        if start_line > end_line || (start_line == end_line && start_char > end_char) {
            return Err(InvalidEditRange {
                start_line,
                start_char,
                end_line,
                end_char,
            });
        }

        Ok(Self {
            start_line,
            start_char,
            end_line,
            end_char,
            new_text,
        })
    }

    pub fn start_line(&self) -> u32 {
        self.start_line
    }

    pub fn start_char(&self) -> u32 {
        self.start_char
    }

    pub fn end_line(&self) -> u32 {
        self.end_line
    }

    pub fn end_char(&self) -> u32 {
        self.end_char
    }

    pub fn new_text(&self) -> &str {
        &self.new_text
    }
}

/// Quick check: did formatting change anything?
///
/// This is a fast-path that avoids full diff computation.
#[must_use]
pub fn is_changed(original: &str, formatted: &str) -> bool {
    original != formatted
}

/// Compute edits that transform `original` into `formatted`.
///
/// Returns an empty `Vec` when the inputs are identical.  Each returned
/// [`Edit`] replaces a contiguous range in `original` with `new_text`.
/// Adjacent diff hunks are coalesced into a single edit so the result set
/// is minimal.
#[must_use]
pub fn compute_text_edits(original: &str, formatted: &str) -> Vec<Edit> {
    if !is_changed(original, formatted) {
        return Vec::new();
    }

    let diff = TextDiff::from_lines(original, formatted);
    let mut edits: Vec<Edit> = Vec::new();

    // Track current line in the *original* document.
    let mut orig_line: u32 = 0;

    // Accumulate contiguous hunks into one edit.
    let mut hunk_start_line: Option<u32> = None;
    let mut hunk_end_line: u32 = 0;
    let mut hunk_new_text = String::new();

    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Equal => {
                flush_hunk(
                    &mut edits,
                    &mut hunk_start_line,
                    hunk_end_line,
                    &mut hunk_new_text,
                );
                orig_line += 1;
            }
            ChangeTag::Delete => {
                if hunk_start_line.is_none() {
                    hunk_start_line = Some(orig_line);
                }
                hunk_end_line = orig_line + 1;
                orig_line += 1;
            }
            ChangeTag::Insert => {
                if hunk_start_line.is_none() {
                    hunk_start_line = Some(orig_line);
                    hunk_end_line = orig_line;
                }
                hunk_new_text.push_str(change.value());
            }
        }
    }

    flush_hunk(
        &mut edits,
        &mut hunk_start_line,
        hunk_end_line,
        &mut hunk_new_text,
    );

    edits
}

fn flush_hunk(
    edits: &mut Vec<Edit>,
    hunk_start_line: &mut Option<u32>,
    hunk_end_line: u32,
    hunk_new_text: &mut String,
) {
    if let Some(start) = hunk_start_line.take() {
        // Range is constructed from forward iteration over diff output,
        // so start <= end is guaranteed.
        edits.push(
            Edit::new(start, 0, hunk_end_line, 0, std::mem::take(hunk_new_text))
                .expect("hunk produced an invalid edit range"),
        );
    }
}

/// Produce a unified diff string suitable for CLI `--diff` output.
///
/// Returns `None` when the inputs are identical.
#[must_use]
pub fn unified_diff(path: &str, original: &str, formatted: &str) -> Option<String> {
    if !is_changed(original, formatted) {
        return None;
    }

    let old_header = format!("a/{path}");
    let new_header = format!("b/{path}");

    let diff = TextDiff::from_lines(original, formatted)
        .unified_diff()
        .header(&old_header, &new_header)
        .to_string();

    Some(diff)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_input_returns_no_edits() {
        let source = "hello\nworld\n";
        assert!(!is_changed(source, source));
        assert!(compute_text_edits(source, source).is_empty());
        assert!(unified_diff("test.html", source, source).is_none());
    }

    #[test]
    fn single_line_replacement() {
        let original = "aaa\nbbb\nccc\n";
        let formatted = "aaa\nBBB\nccc\n";
        let edits = compute_text_edits(original, formatted);

        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0], Edit::new(1, 0, 2, 0, "BBB\n".to_owned()).unwrap());
    }

    #[test]
    fn multi_line_insertion() {
        let original = "aaa\nccc\n";
        let formatted = "aaa\nbbb\nccc\n";
        let edits = compute_text_edits(original, formatted);

        assert_eq!(edits.len(), 1);
        // Insert before line 1 (where "ccc\n" is), range is empty (start==end)
        assert_eq!(edits[0].start_line, 1);
        assert_eq!(edits[0].end_line, 1);
        assert_eq!(edits[0].new_text, "bbb\n");
    }

    #[test]
    fn deletion() {
        let original = "aaa\nbbb\nccc\n";
        let formatted = "aaa\nccc\n";
        let edits = compute_text_edits(original, formatted);

        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].start_line, 1);
        assert_eq!(edits[0].end_line, 2);
        assert_eq!(edits[0].new_text, "");
    }

    #[test]
    fn mixed_changes() {
        let original = "line1\nline2\nline3\nline4\nline5\n";
        let formatted = "line1\nLINE2\nline4\nnew_line\nline5\n";
        let edits = compute_text_edits(original, formatted);

        // line2 → LINE2, line3 deleted → first hunk
        // new_line inserted before line5 → second hunk
        assert_eq!(edits.len(), 2);

        assert_eq!(edits[0].start_line, 1);
        assert_eq!(edits[0].end_line, 3);
        assert_eq!(edits[0].new_text, "LINE2\n");

        assert_eq!(edits[1].start_line, 4);
        assert_eq!(edits[1].end_line, 4);
        assert_eq!(edits[1].new_text, "new_line\n");
    }

    #[test]
    fn empty_to_content() {
        let original = "";
        let formatted = "hello\nworld\n";
        let edits = compute_text_edits(original, formatted);

        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].start_line, 0);
        assert_eq!(edits[0].end_line, 0);
        assert_eq!(edits[0].new_text, "hello\nworld\n");
    }

    #[test]
    fn content_to_empty() {
        let original = "hello\nworld\n";
        let formatted = "";
        let edits = compute_text_edits(original, formatted);

        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].start_line, 0);
        assert_eq!(edits[0].end_line, 2);
        assert_eq!(edits[0].new_text, "");
    }

    #[test]
    fn unified_diff_produces_standard_format() {
        let original = "aaa\nbbb\nccc\n";
        let formatted = "aaa\nBBB\nccc\n";
        let diff = unified_diff("templates/page.html", original, formatted).unwrap();

        assert!(diff.contains("--- a/templates/page.html"));
        assert!(diff.contains("+++ b/templates/page.html"));
        assert!(diff.contains("-bbb"));
        assert!(diff.contains("+BBB"));
    }

    #[test]
    fn is_changed_detects_difference() {
        assert!(is_changed("a", "b"));
        assert!(!is_changed("a", "a"));
        assert!(is_changed("", "x"));
        assert!(is_changed("x", ""));
    }

    #[test]
    fn edits_apply_correctly() {
        // Verify that applying edits to original produces formatted output.
        // This is a round-trip sanity check.
        let original = "line1\nline2\nline3\nline4\n";
        let formatted = "line1\nLINE2\nline3\nnew_line\nline4\n";
        let edits = compute_text_edits(original, formatted);

        let result = apply_edits(original, &edits);
        assert_eq!(result, formatted);
    }

    #[test]
    fn edits_apply_correctly_on_deletion() {
        let original = "aaa\nbbb\nccc\n";
        let formatted = "aaa\nccc\n";
        let edits = compute_text_edits(original, &formatted);

        let result = apply_edits(original, &edits);
        assert_eq!(result, formatted);
    }

    #[test]
    fn edits_apply_correctly_on_insertion() {
        let original = "aaa\nccc\n";
        let formatted = "aaa\nbbb\nccc\n";
        let edits = compute_text_edits(original, formatted);

        let result = apply_edits(original, &edits);
        assert_eq!(result, formatted);
    }

    #[test]
    fn edit_allows_empty_range() {
        // An insertion: start == end is valid (zero-width range).
        let edit = Edit::new(3, 5, 3, 5, "inserted".to_owned()).unwrap();
        assert_eq!(edit.start_line, 3);
        assert_eq!(edit.end_char, 5);
    }

    #[test]
    fn edit_rejects_backwards_lines() {
        let err = Edit::new(5, 0, 3, 0, String::new()).unwrap_err();
        assert_eq!(
            err,
            InvalidEditRange {
                start_line: 5,
                start_char: 0,
                end_line: 3,
                end_char: 0,
            }
        );
    }

    #[test]
    fn edit_rejects_backwards_chars_on_same_line() {
        let err = Edit::new(2, 10, 2, 5, String::new()).unwrap_err();
        assert_eq!(
            err,
            InvalidEditRange {
                start_line: 2,
                start_char: 10,
                end_line: 2,
                end_char: 5,
            }
        );
    }

    /// Apply a set of [`Edit`]s to source text, producing the transformed
    /// output. Used only in tests to validate round-trip correctness.
    fn apply_edits(source: &str, edits: &[Edit]) -> String {
        let lines: Vec<&str> = source.split_inclusive('\n').collect();
        let mut result = String::new();
        let mut current_line: u32 = 0;

        for edit in edits {
            // Copy unchanged lines before this edit.
            while current_line < edit.start_line {
                if let Some(line) = lines.get(current_line as usize) {
                    result.push_str(line);
                }
                current_line += 1;
            }

            // Insert the replacement text.
            result.push_str(&edit.new_text);

            // Skip over the replaced original lines.
            current_line = edit.end_line;
        }

        // Copy remaining lines after the last edit.
        while (current_line as usize) < lines.len() {
            result.push_str(lines[current_line as usize]);
            current_line += 1;
        }

        result
    }
}
