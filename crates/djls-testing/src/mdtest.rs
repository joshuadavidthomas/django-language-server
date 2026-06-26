//! Markdown diagnostic snapshot tests.
//!
//! See `resources/mdtest/README.md` for the authoring format.

use std::fmt::Write as _;
use std::ops::Range;
use std::path::Path;

use pulldown_cmark::CodeBlockKind;
use pulldown_cmark::Event;
use pulldown_cmark::HeadingLevel;
use pulldown_cmark::Parser as MarkdownParser;
use pulldown_cmark::Tag;
use pulldown_cmark::TagEnd;

use crate::fixtures::snapshot_validate;
use crate::fixtures::snapshot_validate_file;

const UPDATE_ENV: &str = "DJLS_UPDATE_MDTEST_SNAPSHOTS";
const NO_DIAGNOSTICS_SNAPSHOT: &str = "✓ no diagnostics";

#[derive(Debug)]
struct Scenario {
    name: String,
    file_path: String,
    source: String,
    snapshot: Option<String>,
    snapshot_start: Option<usize>,
    snapshot_end: Option<usize>,
    snapshot_insert_at: usize,
}

impl Scenario {
    fn render_snapshot(&self) -> String {
        let rendered = if self.file_path == "test.html" {
            snapshot_validate(&self.source)
        } else {
            snapshot_validate_file(&self.file_path, &self.source)
        };

        if rendered.trim().is_empty() {
            NO_DIAGNOSTICS_SNAPSHOT.to_string()
        } else {
            rendered
        }
    }

    fn snapshot_update(&self, actual: String) -> SnapshotUpdate {
        if let (Some(start), Some(end)) = (self.snapshot_start, self.snapshot_end) {
            SnapshotUpdate {
                start,
                end,
                replacement: actual,
            }
        } else {
            SnapshotUpdate {
                start: self.snapshot_insert_at,
                end: self.snapshot_insert_at,
                replacement: format!("\n```snapshot\n{actual}\n```"),
            }
        }
    }
}

#[derive(Debug)]
struct SnapshotUpdate {
    start: usize,
    end: usize,
    replacement: String,
}

impl SnapshotUpdate {
    fn apply_all(markdown: &str, updates: &[Self]) -> String {
        let lines = markdown.lines().collect::<Vec<_>>();
        let mut output = String::new();
        let mut cursor = 0;

        for update in updates {
            for line in &lines[cursor..update.start] {
                writeln!(&mut output, "{line}").unwrap();
            }
            if !update.replacement.is_empty() {
                for line in update.replacement.lines() {
                    writeln!(&mut output, "{line}").unwrap();
                }
            }
            cursor = update.end;
        }

        for line in &lines[cursor..] {
            writeln!(&mut output, "{line}").unwrap();
        }

        output
    }
}

pub fn run_suite(dir: &Path) {
    MdtestRun::new(dir.to_path_buf()).run();
}

struct MdtestRun {
    root: std::path::PathBuf,
    update: bool,
    failures: Vec<String>,
}

impl MdtestRun {
    fn new(root: std::path::PathBuf) -> Self {
        Self {
            root,
            update: std::env::var_os(UPDATE_ENV).is_some_and(|value| value != "0"),
            failures: Vec::new(),
        }
    }

    fn run(mut self) {
        let files = self.files();
        assert!(!files.is_empty(), "expected at least one mdtest file");

        for path in files {
            self.run_file(&path);
        }

        assert!(
            self.failures.is_empty(),
            "mdtest failures:\n\n{}",
            self.failures.join("\n\n")
        );
    }

    fn files(&self) -> Vec<std::path::PathBuf> {
        let mut dirs = vec![self.root.clone()];
        let mut files = Vec::new();

        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).expect("failed to read mdtest directory") {
                let path = entry.expect("failed to read mdtest directory entry").path();
                if path.is_dir() {
                    dirs.push(path);
                } else if path.extension().is_some_and(|ext| ext == "md")
                    && path.file_name().is_none_or(|name| name != "README.md")
                {
                    files.push(path);
                }
            }
        }

        files.sort();
        files
    }

    fn run_file(&mut self, path: &Path) {
        let markdown = std::fs::read_to_string(path).expect("failed to read mdtest file");
        let scenarios = match ScenarioCollector::new(&markdown).collect() {
            Ok(scenarios) => scenarios,
            Err(err) => {
                self.failures
                    .push(format!("failed to parse {}: {err}", path.display()));
                return;
            }
        };

        if scenarios.is_empty() {
            self.failures
                .push(format!("{} did not contain any scenarios", path.display()));
            return;
        }

        let mut updates = Vec::new();
        for scenario in scenarios {
            let actual = scenario.render_snapshot();
            if self.update {
                updates.push(scenario.snapshot_update(actual));
            } else {
                self.check_snapshot(path, scenario, &actual);
            }
        }

        if self.update {
            let rewritten_markdown = SnapshotUpdate::apply_all(&markdown, &updates);
            std::fs::write(path, rewritten_markdown).expect("failed to update mdtest snapshots");
        }
    }

    fn check_snapshot(&mut self, path: &Path, scenario: Scenario, actual: &str) {
        let Some(expected) = scenario.snapshot else {
            self.failures.push(format!(
                "mdtest scenario missing snapshot: {} ({}) in {}. Set {UPDATE_ENV}=1 to insert snapshots.",
                scenario.name,
                scenario.file_path,
                path.display(),
            ));
            return;
        };

        if expected.trim_end() != actual.trim_end() {
            self.failures.push(format!(
                "mdtest scenario failed: {} ({}) in {}\n\nexpected:\n{}\n\nactual:\n{}\n\nSet {UPDATE_ENV}=1 to update snapshots.",
                scenario.name,
                scenario.file_path,
                path.display(),
                expected.trim_end(),
                actual.trim_end(),
            ));
        }
    }
}

struct ScenarioCollector<'a> {
    markdown: &'a str,
    line_starts: Vec<usize>,
    current: Option<PartialScenario>,
    scenarios: Vec<Scenario>,
    pending_file_path: Option<String>,
    headings: Vec<Heading>,
    active_heading: Option<ActiveHeading>,
    active_code_block: Option<ActiveCodeBlock>,
    active_paragraph: Option<String>,
}

#[derive(Debug)]
struct Heading {
    level: usize,
    name: String,
}

#[derive(Debug)]
struct PartialScenario {
    name: String,
    level: usize,
    file_path: Option<String>,
    source: Option<String>,
    snapshot: Option<String>,
    snapshot_start: Option<usize>,
    snapshot_end: Option<usize>,
    snapshot_insert_at: Option<usize>,
}

#[derive(Debug)]
struct ActiveHeading {
    level: usize,
    name: String,
}

#[derive(Debug)]
struct ActiveCodeBlock {
    language: Option<String>,
    content: String,
    content_start: Option<usize>,
    content_end: Option<usize>,
}

#[derive(Debug)]
struct FencedBlock {
    language: String,
    content: String,
    content_start: usize,
    content_end: usize,
    fence_end: usize,
}

impl PartialScenario {
    fn new(name: String, level: usize) -> Self {
        Self {
            name,
            level,
            file_path: None,
            source: None,
            snapshot: None,
            snapshot_start: None,
            snapshot_end: None,
            snapshot_insert_at: None,
        }
    }
}

impl ActiveCodeBlock {
    fn new(language: Option<String>) -> Self {
        Self {
            language,
            content: String::new(),
            content_start: None,
            content_end: None,
        }
    }
}

impl<'a> ScenarioCollector<'a> {
    fn new(markdown: &'a str) -> Self {
        let mut line_starts = vec![0];
        for (index, byte) in markdown.bytes().enumerate() {
            if byte == b'\n' {
                line_starts.push(index + 1);
            }
        }

        Self {
            markdown,
            line_starts,
            current: None,
            scenarios: Vec::new(),
            pending_file_path: None,
            headings: Vec::new(),
            active_heading: None,
            active_code_block: None,
            active_paragraph: None,
        }
    }

    fn collect(mut self) -> Result<Vec<Scenario>, String> {
        for (event, range) in MarkdownParser::new(self.markdown).into_offset_iter() {
            match event {
                Event::Start(Tag::Heading { level, .. }) => {
                    let level = match level {
                        HeadingLevel::H1 => 1,
                        HeadingLevel::H2 => 2,
                        HeadingLevel::H3 => 3,
                        HeadingLevel::H4 => 4,
                        HeadingLevel::H5 => 5,
                        HeadingLevel::H6 => 6,
                    };
                    self.active_heading = Some(ActiveHeading {
                        level,
                        name: String::new(),
                    });
                }
                Event::End(TagEnd::Heading(_)) => self.finish_heading()?,
                Event::Start(Tag::Paragraph) => {
                    self.active_paragraph = Some(String::new());
                }
                Event::End(TagEnd::Paragraph) => {
                    if let Some(paragraph) = self.active_paragraph.take() {
                        let trimmed = paragraph.trim();
                        if let Some(label) = trimmed
                            .strip_prefix('`')
                            .and_then(|value| value.strip_suffix("`:"))
                            && !label.is_empty()
                        {
                            self.pending_file_path = Some(label.to_string());
                        }
                    }
                }
                Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(info))) => {
                    let language = info.split_whitespace().next().map(str::to_string);
                    self.active_code_block = Some(ActiveCodeBlock::new(language));
                }
                Event::Start(Tag::CodeBlock(CodeBlockKind::Indented)) => {
                    self.active_code_block = Some(ActiveCodeBlock::new(None));
                }
                Event::End(TagEnd::CodeBlock) => self.finish_code_block(range)?,
                Event::Text(text) => self.push_text(&text, range),
                Event::Code(text) => self.push_inline_code(&text),
                _ => {}
            }
        }

        self.finish_current()?;
        Ok(self.scenarios)
    }

    fn push_text(&mut self, text: &str, range: Range<usize>) {
        let content_start = self.line_at(range.start);
        let content_end = self.line_after(range.end);
        if let Some(code_block) = &mut self.active_code_block {
            if code_block.content_start.is_none() {
                code_block.content_start = Some(content_start);
            }
            code_block.content_end = Some(content_end);
            code_block.content.push_str(text);
        } else if let Some(heading) = &mut self.active_heading {
            heading.name.push_str(text);
        } else if let Some(paragraph) = &mut self.active_paragraph {
            paragraph.push_str(text);
        }
    }

    fn push_inline_code(&mut self, text: &str) {
        if let Some(heading) = &mut self.active_heading {
            heading.name.push_str(text);
        } else if let Some(paragraph) = &mut self.active_paragraph {
            write!(paragraph, "`{text}`").unwrap();
        }
    }

    fn finish_heading(&mut self) -> Result<(), String> {
        let Some(heading) = self.active_heading.take() else {
            return Ok(());
        };
        let name = heading.name.trim();
        if name.is_empty() {
            return Ok(());
        }

        if let Some(current) = &self.current
            && current.source.is_some()
            && heading.level > current.level
        {
            return Err(format!(
                "scenario '{}' has child heading '{}' after its Django code block",
                current.name, name
            ));
        }

        self.finish_current()?;
        self.headings
            .retain(|current| current.level < heading.level);
        self.headings.push(Heading {
            level: heading.level,
            name: name.to_string(),
        });
        self.current = Some(PartialScenario::new(self.scenario_name(), heading.level));
        self.pending_file_path = None;
        Ok(())
    }

    fn finish_code_block(&mut self, range: Range<usize>) -> Result<(), String> {
        let Some(code_block) = self.active_code_block.take() else {
            return Ok(());
        };
        let Some(language) = code_block.language else {
            return Ok(());
        };
        if !matches!(
            language.as_str(),
            "htmldjango" | "django" | "html" | "snapshot"
        ) {
            return Ok(());
        }

        let closing_fence_line = self.line_at(range.start);
        let block = FencedBlock {
            language,
            content: code_block.content.trim_end_matches('\n').to_string(),
            content_start: code_block.content_start.unwrap_or(closing_fence_line),
            content_end: code_block.content_end.unwrap_or(closing_fence_line),
            fence_end: self.line_after(range.end),
        };

        match block.language.as_str() {
            "htmldjango" | "django" | "html" => self.set_source(block),
            "snapshot" => self.set_snapshot(block),
            _ => Ok(()),
        }
    }

    fn set_source(&mut self, block: FencedBlock) -> Result<(), String> {
        let current = self.current.as_mut().ok_or_else(|| {
            "htmldjango code block must appear under a scenario heading".to_string()
        })?;

        if current.source.is_some() {
            return Err(format!(
                "scenario '{}' has more than one htmldjango code block",
                current.name
            ));
        }

        current.source = Some(block.content);
        current.snapshot_insert_at = Some(block.fence_end);
        current.file_path = Some(
            self.pending_file_path
                .take()
                .unwrap_or_else(|| "test.html".to_string()),
        );
        Ok(())
    }

    fn set_snapshot(&mut self, block: FencedBlock) -> Result<(), String> {
        let current = self
            .current
            .as_mut()
            .ok_or_else(|| "snapshot block must appear under a scenario heading".to_string())?;

        if current.snapshot.is_some() {
            return Err(format!(
                "scenario '{}' has more than one snapshot block",
                current.name
            ));
        }

        current.snapshot = Some(block.content);
        current.snapshot_start = Some(block.content_start);
        current.snapshot_end = Some(block.content_end);
        Ok(())
    }

    fn finish_current(&mut self) -> Result<(), String> {
        let Some(current) = self.current.take() else {
            return Ok(());
        };

        match current.source {
            Some(source) => {
                self.scenarios.push(Scenario {
                    name: current.name,
                    file_path: current.file_path.unwrap_or_else(|| "test.html".to_string()),
                    source,
                    snapshot: current.snapshot,
                    snapshot_start: current.snapshot_start,
                    snapshot_end: current.snapshot_end,
                    snapshot_insert_at: current
                        .snapshot_insert_at
                        .expect("source block should have an insertion point"),
                });
                Ok(())
            }
            None if current.snapshot.is_none() => Ok(()),
            None => Err(format!(
                "scenario '{}' has a snapshot block but no Django code block",
                current.name
            )),
        }
    }

    fn scenario_name(&self) -> String {
        self.headings
            .iter()
            .map(|heading| heading.name.as_str())
            .collect::<Vec<_>>()
            .join(" / ")
    }

    fn line_at(&self, byte: usize) -> usize {
        self.line_starts
            .partition_point(|line_start| *line_start <= byte)
            .saturating_sub(1)
    }

    fn line_after(&self, byte: usize) -> usize {
        if byte > 0 && self.markdown.as_bytes().get(byte - 1) == Some(&b'\n') {
            self.line_at(byte)
        } else {
            self.line_at(byte) + 1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_heading_path_for_scenario_name() {
        let markdown = r"# Diagnostics

## else outside if

`templates/test.html`:

```htmldjango
{% else %}
```

```snapshot
error[S102]: Orphaned tag
```
";

        let scenarios = ScenarioCollector::new(markdown).collect().unwrap();

        assert_eq!(scenarios.len(), 1);
        assert_eq!(scenarios[0].name, "Diagnostics / else outside if");
        assert_eq!(scenarios[0].file_path, "templates/test.html");
        assert_eq!(scenarios[0].source, "{% else %}");
        assert_eq!(
            scenarios[0].snapshot.as_deref(),
            Some("error[S102]: Orphaned tag")
        );
        assert_eq!(scenarios[0].snapshot_start, Some(11));
        assert_eq!(scenarios[0].snapshot_end, Some(12));
        assert_eq!(scenarios[0].snapshot_insert_at, 9);
    }

    #[test]
    fn ignores_group_headings_without_code_blocks() {
        let markdown = r"# for

## Valid

### iterates over a sequence

```htmldjango
{% for item in items %}{% endfor %}
```

## Invalid

### reports empty outside for

```htmldjango
{% empty %}
```
";

        let scenarios = ScenarioCollector::new(markdown).collect().unwrap();

        assert_eq!(scenarios.len(), 2);
        assert_eq!(scenarios[0].name, "for / Valid / iterates over a sequence");
        assert_eq!(
            scenarios[1].name,
            "for / Invalid / reports empty outside for"
        );
    }

    #[test]
    fn rejects_child_heading_after_source_block() {
        let markdown = r#"# i18n

## Valid

```htmldjango
{% load i18n %}
{% trans "Hello" %}
```

### translates a literal after load

```htmldjango
{% load i18n %}
{% trans "Hello" %}
```
"#;

        let error = ScenarioCollector::new(markdown).collect().unwrap_err();

        assert_eq!(
            error,
            "scenario 'i18n / Valid' has child heading 'translates a literal after load' after its Django code block"
        );
    }

    #[test]
    fn rejects_multiple_source_blocks_in_one_heading() {
        let markdown = r#"# i18n

## translates a literal after load

```htmldjango
{% load i18n %}
{% trans "Hello" %}
```

```htmldjango
{% load i18n %}
{% trans "Goodbye" %}
```
"#;

        let error = ScenarioCollector::new(markdown).collect().unwrap_err();

        assert_eq!(
            error,
            "scenario 'i18n / translates a literal after load' has more than one htmldjango code block"
        );
    }

    #[test]
    fn rewrites_snapshot_contents() {
        let markdown = r"## scenario

```htmldjango
{% else %}
```

```snapshot
old snapshot
```
";
        let rewritten = SnapshotUpdate::apply_all(
            markdown,
            &[SnapshotUpdate {
                start: 7,
                end: 8,
                replacement: "new snapshot".to_string(),
            }],
        );

        assert_eq!(
            rewritten,
            r"## scenario

```htmldjango
{% else %}
```

```snapshot
new snapshot
```
"
        );
    }

    #[test]
    fn inserts_missing_snapshot_block() {
        let markdown = r"## scenario

```htmldjango
{% else %}
```
";
        let rewritten = SnapshotUpdate::apply_all(
            markdown,
            &[SnapshotUpdate {
                start: 5,
                end: 5,
                replacement: "\n```snapshot\nnew snapshot\n```".to_string(),
            }],
        );

        assert_eq!(
            rewritten,
            r"## scenario

```htmldjango
{% else %}
```

```snapshot
new snapshot
```
"
        );
    }
}
