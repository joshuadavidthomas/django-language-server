//! Markdown diagnostic snapshot tests.
//!
//! See `resources/mdtest/README.md` for the authoring format.

use std::ops::Range;
use std::path::Path;

use anyhow::Context as _;
use camino::Utf8Path;
use pulldown_cmark::CodeBlockKind;
use pulldown_cmark::Event;
use pulldown_cmark::HeadingLevel;
use pulldown_cmark::Parser as MarkdownParser;
use pulldown_cmark::Tag;
use pulldown_cmark::TagEnd;

use crate::OsTestDatabase;
use crate::ProjectSettings;
use crate::fixtures::snapshot_validate_files;
use crate::fixtures::standard_validation_db;

const UPDATE_ENV: &str = "DJLS_UPDATE_MDTEST_SNAPSHOTS";
const NO_DIAGNOSTICS_SNAPSHOT: &str = "✓ no diagnostics";
const VALIDATION_PROJECT_ROOT: &str = "/fixture";
const VALIDATION_SETTINGS_PATH: &str = "/fixture/settings.py";
const VALIDATION_TEMPLATE_ROOT: &str = "/templates";

#[derive(Debug)]
pub struct Scenario {
    name: String,
    pub files: Vec<ScenarioFile>,
    primary_file_index: usize,
    snapshot: Option<String>,
    snapshot_start: Option<usize>,
    snapshot_end: Option<usize>,
    snapshot_insert_at: usize,
    settings_source: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScenarioFileKind {
    Template,
    Python,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScenarioFile {
    pub kind: ScenarioFileKind,
    pub path: String,
    pub source: String,
}

impl Scenario {
    /// Return the primary file for this scenario.
    pub fn primary_file(&self) -> anyhow::Result<&ScenarioFile> {
        self.files.get(self.primary_file_index).ok_or_else(|| {
            anyhow::anyhow!(
                "scenario `{}` has no primary source file at index {}",
                self.name,
                self.primary_file_index
            )
        })
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
                output.push_str(line);
                output.push('\n');
            }
            if !update.replacement.is_empty() {
                for line in update.replacement.lines() {
                    output.push_str(line);
                    output.push('\n');
                }
            }
            cursor = update.end;
        }

        for line in &lines[cursor..] {
            output.push_str(line);
            output.push('\n');
        }

        output
    }
}

/// Render one validation scenario against a caller-supplied database.
pub fn render_validation_scenario(
    db: &mut OsTestDatabase,
    scenario: &Scenario,
) -> anyhow::Result<String> {
    let primary = scenario.primary_file()?;
    let python_files = scenario
        .files
        .iter()
        .filter(|file| file.kind == ScenarioFileKind::Python)
        .map(|file| {
            djls_source::safe_join(Utf8Path::new(VALIDATION_PROJECT_ROOT), &file.path)
                .map(|path| (path, file.source.as_str()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let template_files = scenario
        .files
        .iter()
        .filter(|file| file.kind == ScenarioFileKind::Template)
        .map(|file| {
            djls_source::safe_join(Utf8Path::new(VALIDATION_TEMPLATE_ROOT), &file.path)
                .map(|path| (path, file.source.as_str()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let primary_database_path =
        djls_source::safe_join(Utf8Path::new(VALIDATION_TEMPLATE_ROOT), &primary.path)?;
    for (path, source) in &python_files {
        db.add_file(path.as_str(), source)?;
    }
    if let Some(settings_source) = &scenario.settings_source {
        db.add_file(VALIDATION_SETTINGS_PATH, settings_source)?;
    }

    let rendered = snapshot_validate_files(
        db,
        primary_database_path.as_str(),
        primary.path.as_str(),
        primary.source.as_str(),
        template_files
            .iter()
            .map(|(path, source)| (path.as_str(), *source)),
    );
    for (path, _) in &template_files {
        db.remove_file(path.as_str())?;
    }
    for (path, _) in &python_files {
        db.remove_file(path.as_str())?;
    }
    if scenario.settings_source.is_some() {
        db.add_file(
            VALIDATION_SETTINGS_PATH,
            &ProjectSettings::default().settings_py(),
        )?;
    }

    let rendered = rendered?;
    Ok(if rendered.trim().is_empty() {
        NO_DIAGNOSTICS_SNAPSHOT.to_string()
    } else {
        rendered
    })
}

pub fn run_suite(dir: &Path) -> anyhow::Result<()> {
    MdtestRun::new(dir.to_path_buf(), Renderer::Validation).run()
}

pub fn run_suite_with(
    dir: &Path,
    render: fn(&Scenario) -> anyhow::Result<String>,
) -> anyhow::Result<()> {
    MdtestRun::new(dir.to_path_buf(), Renderer::Scenario(render)).run()
}

#[derive(Clone, Copy)]
enum Renderer {
    Validation,
    Scenario(fn(&Scenario) -> anyhow::Result<String>),
}

struct MdtestRun {
    root: std::path::PathBuf,
    renderer: Renderer,
    update: bool,
    failures: Vec<String>,
}

impl MdtestRun {
    fn new(root: std::path::PathBuf, renderer: Renderer) -> Self {
        Self {
            root,
            renderer,
            update: std::env::var_os(UPDATE_ENV).is_some_and(|value| value != "0"),
            failures: Vec::new(),
        }
    }

    fn run(mut self) -> anyhow::Result<()> {
        let files = self.files()?;
        if files.is_empty() {
            anyhow::bail!(
                "expected at least one mdtest file in `{}`",
                self.root.display()
            );
        }

        match self.renderer {
            Renderer::Validation => {
                let mut db = standard_validation_db()?;
                let mut render =
                    |scenario: &Scenario| render_validation_scenario(&mut db, scenario);
                for path in files {
                    self.run_file(&path, &mut render)?;
                }
            }
            Renderer::Scenario(mut render) => {
                for path in files {
                    self.run_file(&path, &mut render)?;
                }
            }
        }

        if !self.failures.is_empty() {
            anyhow::bail!("mdtest failures:\n\n{}", self.failures.join("\n\n"));
        }
        Ok(())
    }

    fn files(&self) -> anyhow::Result<Vec<std::path::PathBuf>> {
        let mut dirs = vec![self.root.clone()];
        let mut files = Vec::new();

        while let Some(dir) = dirs.pop() {
            let entries = std::fs::read_dir(&dir)
                .with_context(|| format!("failed to read mdtest directory `{}`", dir.display()))?;
            for entry in entries {
                let path = entry
                    .with_context(|| {
                        format!(
                            "failed to read an entry in mdtest directory `{}`",
                            dir.display()
                        )
                    })?
                    .path();
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
        Ok(files)
    }

    fn run_file(
        &mut self,
        path: &Path,
        render: &mut impl FnMut(&Scenario) -> anyhow::Result<String>,
    ) -> anyhow::Result<()> {
        let markdown = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read mdtest file `{}`", path.display()))?;
        let scenarios = match ScenarioCollector::new(&markdown).collect() {
            Ok(scenarios) => scenarios,
            Err(err) => {
                self.failures
                    .push(format!("failed to parse {}: {err}", path.display()));
                return Ok(());
            }
        };

        if scenarios.is_empty() {
            self.failures
                .push(format!("{} did not contain any scenarios", path.display()));
            return Ok(());
        }

        let updates = self.render_scenarios(path, &scenarios, render)?;

        if self.update {
            let rewritten_markdown = SnapshotUpdate::apply_all(&markdown, &updates);
            std::fs::write(path, rewritten_markdown).with_context(|| {
                format!(
                    "failed to update snapshots in mdtest file `{}`",
                    path.display()
                )
            })?;
        }
        Ok(())
    }

    fn render_scenarios(
        &mut self,
        path: &Path,
        scenarios: &[Scenario],
        mut render: impl FnMut(&Scenario) -> anyhow::Result<String>,
    ) -> anyhow::Result<Vec<SnapshotUpdate>> {
        let mut updates = Vec::new();
        for scenario in scenarios {
            let actual = render(scenario)
                .with_context(|| format!("failed to render mdtest scenario `{}`", scenario.name))?;
            if self.update {
                updates.push(scenario.snapshot_update(actual));
            } else {
                self.check_snapshot(path, scenario, &actual)?;
            }
        }
        Ok(updates)
    }

    fn check_snapshot(
        &mut self,
        path: &Path,
        scenario: &Scenario,
        actual: &str,
    ) -> anyhow::Result<()> {
        let primary_path = &scenario.primary_file()?.path;
        let Some(expected) = scenario.snapshot.as_deref() else {
            self.failures.push(format!(
                "mdtest scenario missing snapshot: {} ({}) in {}. Set {UPDATE_ENV}=1 to insert snapshots.",
                scenario.name,
                primary_path,
                path.display(),
            ));
            return Ok(());
        };

        if expected.trim_end() != actual.trim_end() {
            self.failures.push(format!(
                "mdtest scenario failed: {} ({}) in {}\n\nexpected:\n{}\n\nactual:\n{}\n\nSet {UPDATE_ENV}=1 to update snapshots.",
                scenario.name,
                primary_path,
                path.display(),
                expected.trim_end(),
                actual.trim_end(),
            ));
        }
        Ok(())
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
    settings: Option<String>,
}

#[derive(Debug)]
struct PartialScenario {
    name: String,
    level: usize,
    files: Vec<ScenarioFile>,
    primary_file_index: Option<usize>,
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
            files: Vec::new(),
            primary_file_index: None,
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
                Event::Start(_)
                | Event::End(_)
                | Event::InlineMath(_)
                | Event::DisplayMath(_)
                | Event::Html(_)
                | Event::InlineHtml(_)
                | Event::FootnoteReference(_)
                | Event::SoftBreak
                | Event::HardBreak
                | Event::Rule
                | Event::TaskListMarker(_) => {}
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
            paragraph.push('`');
            paragraph.push_str(text);
            paragraph.push('`');
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
            && heading.level > current.level
        {
            if current
                .files
                .iter()
                .any(|file| file.kind == ScenarioFileKind::Template)
            {
                return Err(format!(
                    "scenario '{}' has child heading '{}' after its Django code block",
                    current.name, name
                ));
            }
            if current
                .files
                .iter()
                .any(|file| file.kind == ScenarioFileKind::Python)
            {
                return Err(format!(
                    "heading '{}' has a py code block but no template block before child heading '{}'",
                    current.name, name
                ));
            }
        }

        self.finish_current()?;
        self.headings
            .retain(|current| current.level < heading.level);
        self.headings.push(Heading {
            level: heading.level,
            name: name.to_string(),
            settings: None,
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
            self.pending_file_path = None;
            return Ok(());
        };
        if language == "ignore" {
            self.pending_file_path = None;
            return Ok(());
        }
        if !matches!(
            language.as_str(),
            "htmldjango" | "django" | "html" | "py" | "snapshot"
        ) {
            let heading = self
                .current
                .as_ref()
                .map_or("<document>", |current| current.name.as_str());
            return Err(format!(
                "heading '{heading}' has unknown code block language '{language}'"
            ));
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
            "htmldjango" | "django" | "html" => self.set_template(block),
            "py" => self.set_python(block),
            "snapshot" => self.set_snapshot(block),
            _ => Ok(()),
        }
    }

    fn set_template(&mut self, block: FencedBlock) -> Result<(), String> {
        let current = self.current.as_mut().ok_or_else(|| {
            "htmldjango code block must appear under a scenario heading".to_string()
        })?;

        let file_path = if let Some(file_path) = self.pending_file_path.take() {
            file_path
        } else {
            if current.primary_file_index.is_some() {
                return Err(format!(
                    "scenario '{}' has more than one unlabeled htmldjango code block",
                    current.name
                ));
            }
            current.primary_file_index = Some(current.files.len());
            "test.html".to_string()
        };

        if Utf8Path::new(&file_path).is_absolute() {
            return Err(format!(
                "scenario '{}' template path '{}' must be relative",
                current.name, file_path
            ));
        }
        if current
            .files
            .iter()
            .any(|file| file.kind == ScenarioFileKind::Template && file.path == file_path)
        {
            return Err(format!(
                "scenario '{}' has more than one template block for path '{}'",
                current.name, file_path
            ));
        }

        current.files.push(ScenarioFile {
            kind: ScenarioFileKind::Template,
            path: file_path,
            source: block.content,
        });
        current.snapshot_insert_at = Some(block.fence_end);
        Ok(())
    }

    fn set_python(&mut self, block: FencedBlock) -> Result<(), String> {
        let current = self
            .current
            .as_mut()
            .ok_or_else(|| "py code block must appear under a heading".to_string())?;
        let file_path = self
            .pending_file_path
            .take()
            .ok_or_else(|| format!("heading '{}' has an unlabeled py code block", current.name))?;

        if Utf8Path::new(&file_path).is_absolute() {
            return Err(format!(
                "heading '{}' Python path '{}' must be relative",
                current.name, file_path
            ));
        }
        if file_path == "settings.py" {
            let heading = self
                .headings
                .last_mut()
                .ok_or_else(|| "settings.py must appear under a heading".to_string())?;
            if heading.settings.is_some() {
                return Err(format!(
                    "heading '{}' has more than one py block for path 'settings.py'",
                    current.name
                ));
            }
            heading.settings = Some(block.content);
            return Ok(());
        }
        if current
            .files
            .iter()
            .any(|file| file.kind == ScenarioFileKind::Python && file.path == file_path)
        {
            return Err(format!(
                "heading '{}' has more than one py block for path '{}'",
                current.name, file_path
            ));
        }

        current.files.push(ScenarioFile {
            kind: ScenarioFileKind::Python,
            path: file_path,
            source: block.content,
        });
        Ok(())
    }

    fn set_snapshot(&mut self, block: FencedBlock) -> Result<(), String> {
        self.pending_file_path = None;
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
        let template_count = current
            .files
            .iter()
            .filter(|file| file.kind == ScenarioFileKind::Template)
            .count();

        if template_count > 0 {
            let local_primary_file_index = if template_count == 1 {
                current
                    .primary_file_index
                    .or_else(|| {
                        current
                            .files
                            .iter()
                            .position(|file| file.kind == ScenarioFileKind::Template)
                    })
                    .ok_or_else(|| format!("scenario '{}' has no template file", current.name))?
            } else {
                current.primary_file_index.ok_or_else(|| {
                    format!(
                        "scenario '{}' has multiple template blocks but no unlabeled file under test",
                        current.name
                    )
                })?
            };
            let snapshot_insert_at = current.snapshot_insert_at.ok_or_else(|| {
                format!(
                    "scenario '{}' source block has no snapshot insertion point",
                    current.name
                )
            })?;
            let settings_source = self
                .headings
                .iter()
                .rev()
                .find_map(|heading| heading.settings.clone());
            self.scenarios.push(Scenario {
                name: current.name,
                files: current.files,
                primary_file_index: local_primary_file_index,
                snapshot: current.snapshot,
                snapshot_start: current.snapshot_start,
                snapshot_end: current.snapshot_end,
                snapshot_insert_at,
                settings_source,
            });
            Ok(())
        } else if current.snapshot.is_none() {
            Ok(())
        } else {
            Err(format!(
                "scenario '{}' has a snapshot block but no Django code block",
                current.name
            ))
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

        let scenarios = ScenarioCollector::new(markdown)
            .collect()
            .expect("heading-path scenario should parse");

        assert_eq!(scenarios.len(), 1);
        assert_eq!(scenarios[0].name, "Diagnostics / else outside if");
        assert_eq!(
            scenarios[0].files,
            vec![ScenarioFile {
                kind: ScenarioFileKind::Template,
                path: "templates/test.html".to_string(),
                source: "{% else %}".to_string(),
            }]
        );
        assert_eq!(
            scenarios[0].snapshot.as_deref(),
            Some("error[S102]: Orphaned tag")
        );
        assert_eq!(scenarios[0].snapshot_start, Some(11));
        assert_eq!(scenarios[0].snapshot_end, Some(12));
        assert_eq!(scenarios[0].snapshot_insert_at, 9);
    }

    #[test]
    fn parses_unlabeled_single_file_scenario_with_default_path() {
        let markdown = r"# Diagnostics

## else outside if

```htmldjango
{% else %}
```

```snapshot
error[S102]: Orphaned tag
```
";

        let scenarios = ScenarioCollector::new(markdown)
            .collect()
            .expect("single-file scenario should parse");

        assert_eq!(scenarios.len(), 1);
        assert_eq!(
            scenarios[0].files,
            vec![ScenarioFile {
                kind: ScenarioFileKind::Template,
                path: "test.html".to_string(),
                source: "{% else %}".to_string(),
            }]
        );
        assert_eq!(
            scenarios[0].snapshot.as_deref(),
            Some("error[S102]: Orphaned tag")
        );
    }

    #[test]
    fn parses_multi_file_scenario_with_unlabeled_primary_first() {
        let markdown = r#"# Inheritance

## child and parent

```htmldjango
{% extends "parent.html" %}
```

`parent.html`:

```html
{% block content %}{% endblock %}
```

```snapshot
✓ no diagnostics
```
"#;

        let scenarios = ScenarioCollector::new(markdown)
            .collect()
            .expect("primary-first multi-file scenario should parse");

        assert_eq!(scenarios.len(), 1);
        assert_eq!(scenarios[0].name, "Inheritance / child and parent");
        assert_eq!(
            scenarios[0].files,
            vec![
                ScenarioFile {
                    kind: ScenarioFileKind::Template,
                    path: "test.html".to_string(),
                    source: "{% extends \"parent.html\" %}".to_string(),
                },
                ScenarioFile {
                    kind: ScenarioFileKind::Template,
                    path: "parent.html".to_string(),
                    source: "{% block content %}{% endblock %}".to_string(),
                },
            ]
        );
        assert_eq!(
            scenarios[0]
                .primary_file()
                .expect("scenario should have a primary file")
                .path,
            "test.html"
        );
        assert_eq!(scenarios[0].snapshot.as_deref(), Some("✓ no diagnostics"));
    }

    #[test]
    fn parses_multi_file_scenario_with_unlabeled_primary_after_support() {
        let markdown = r#"# Inheritance

## child and parent

`parent.html`:

```html
{% block content %}{% endblock %}
```

```htmldjango
{% extends "parent.html" %}
```

```snapshot
✓ no diagnostics
```
"#;

        let scenarios = ScenarioCollector::new(markdown)
            .collect()
            .expect("support-first multi-file scenario should parse");

        assert_eq!(scenarios.len(), 1);
        assert_eq!(scenarios[0].name, "Inheritance / child and parent");
        assert_eq!(
            scenarios[0].files,
            vec![
                ScenarioFile {
                    kind: ScenarioFileKind::Template,
                    path: "parent.html".to_string(),
                    source: "{% block content %}{% endblock %}".to_string(),
                },
                ScenarioFile {
                    kind: ScenarioFileKind::Template,
                    path: "test.html".to_string(),
                    source: "{% extends \"parent.html\" %}".to_string(),
                },
            ]
        );
        assert_eq!(
            scenarios[0]
                .primary_file()
                .expect("scenario should have a primary file")
                .path,
            "test.html"
        );
    }

    #[test]
    fn rejects_multi_file_scenario_with_only_labeled_blocks() {
        let markdown = r#"# Inheritance

## child and parent

`child.html`:

```htmldjango
{% extends "parent.html" %}
```

`parent.html`:

```html
{% block content %}{% endblock %}
```
"#;

        let error = ScenarioCollector::new(markdown)
            .collect()
            .expect_err("multi-file scenario with no primary file should be rejected");

        assert_eq!(
            error,
            "scenario 'Inheritance / child and parent' has multiple template blocks but no unlabeled file under test"
        );
    }

    #[test]
    fn rejects_duplicate_template_paths() {
        let markdown = r"## duplicate paths

`test.html`:

```html
support
```

```htmldjango
primary
```
";

        let error = ScenarioCollector::new(markdown)
            .collect()
            .expect_err("duplicate template paths should be rejected");

        assert_eq!(
            error,
            "scenario 'duplicate paths' has more than one template block for path 'test.html'"
        );
    }

    #[test]
    fn rejects_absolute_template_paths() {
        let markdown = r"## absolute path

`/templates/test.html`:

```html
source
```
";

        let error = ScenarioCollector::new(markdown)
            .collect()
            .expect_err("absolute template paths should be rejected");

        assert_eq!(
            error,
            "scenario 'absolute path' template path '/templates/test.html' must be relative"
        );
    }

    #[test]
    fn rejects_two_settings_python_blocks_in_one_section() {
        let markdown = r"# Shape

`settings.py`:

```py
FIRST = 1
```

`settings.py`:

```py
SECOND = 2
```
";

        let error = ScenarioCollector::new(markdown)
            .collect()
            .expect_err("two settings.py blocks should be rejected");

        assert_eq!(
            error,
            "heading 'Shape' has more than one py block for path 'settings.py'"
        );
    }

    #[test]
    fn rejects_unlabeled_python_blocks() {
        let markdown = r"# Shape

```py
VALUE = 1
```
";

        let error = ScenarioCollector::new(markdown)
            .collect()
            .expect_err("unlabeled py block should be rejected");

        assert_eq!(error, "heading 'Shape' has an unlabeled py code block");
    }

    #[test]
    fn rejects_unknown_code_block_languages() {
        let markdown = r"# Scenario

```rust
fn main() {}
```
";

        let error = ScenarioCollector::new(markdown)
            .collect()
            .expect_err("unknown fence language should be rejected");

        assert_eq!(
            error,
            "heading 'Scenario' has unknown code block language 'rust'"
        );
    }

    #[test]
    fn rejects_duplicate_python_labels_in_one_section() {
        let markdown = r"# Scenario

`custom_tags.py`:

```py
FIRST = 1
```

`custom_tags.py`:

```py
SECOND = 2
```
";

        let error = ScenarioCollector::new(markdown)
            .collect()
            .expect_err("duplicate py labels should be rejected");

        assert_eq!(
            error,
            "heading 'Scenario' has more than one py block for path 'custom_tags.py'"
        );
    }

    #[test]
    fn inherits_settings_python_from_an_ancestor_heading() {
        let markdown = r#"# Shape

`settings.py`:

```py
BUILTINS = ["local_tags"]
```

## Group

### scenario

`local_tags.py`:

```py
LOCAL = 1
```

```htmldjango
{% local_tag %}
```

```snapshot
snapshot
```
"#;

        let scenarios = ScenarioCollector::new(markdown)
            .collect()
            .expect("inherited settings should parse");

        assert_eq!(scenarios.len(), 1);
        assert_eq!(
            scenarios[0].files,
            vec![
                ScenarioFile {
                    kind: ScenarioFileKind::Python,
                    path: "local_tags.py".to_string(),
                    source: "LOCAL = 1".to_string(),
                },
                ScenarioFile {
                    kind: ScenarioFileKind::Template,
                    path: "test.html".to_string(),
                    source: "{% local_tag %}".to_string(),
                },
            ]
        );
        assert_eq!(
            scenarios[0].settings_source.as_deref(),
            Some("BUILTINS = [\"local_tags\"]")
        );
    }

    #[test]
    fn allows_settings_python_on_a_grouping_heading() {
        let markdown = r"# Shape

## Group

`settings.py`:

```py
VALUE = 1
```

### scenario

```htmldjango
content
```
";

        let scenarios = ScenarioCollector::new(markdown)
            .collect()
            .expect("settings.py on a grouping heading should parse");

        assert_eq!(scenarios.len(), 1);
        assert_eq!(scenarios[0].settings_source.as_deref(), Some("VALUE = 1"));
    }

    #[test]
    fn python_files_belong_only_to_their_section() {
        let markdown = r"# Shape

## first

`local_tags.py`:

```py
FIRST = 1
```

```htmldjango
first
```

```snapshot
snapshot
```

## second

`local_tags.py`:

```py
SECOND = 2
```

```htmldjango
second
```

```snapshot
snapshot
```
";

        let scenarios = ScenarioCollector::new(markdown)
            .collect()
            .expect("section-local Python files should parse");

        assert_eq!(scenarios.len(), 2);
        assert_eq!(scenarios[0].files[0].source, "FIRST = 1");
        assert_eq!(scenarios[1].files[0].source, "SECOND = 2");
        assert_eq!(
            scenarios
                .iter()
                .map(|scenario| scenario.files.len())
                .collect::<Vec<_>>(),
            [2, 2]
        );
    }

    #[test]
    fn rejects_python_on_a_grouping_heading() {
        let markdown = r"# Shape

`unused_tags.py`:

```py
UNUSED = 1
```

## scenario

```htmldjango
content
```
";

        let error = ScenarioCollector::new(markdown)
            .collect()
            .expect_err("Python on a grouping heading should be rejected");

        assert_eq!(
            error,
            "heading 'Shape' has a py code block but no template block before child heading 'scenario'"
        );
    }

    #[test]
    fn ignore_blocks_do_not_relabel_templates() {
        let markdown = r"# Group

`ignored.py`:

```ignore
ignored
```

## scenario

```htmldjango
content
```

```snapshot
snapshot
```
";

        let scenarios = ScenarioCollector::new(markdown)
            .collect()
            .expect("ignore block should be skipped");

        assert_eq!(scenarios.len(), 1);
        assert_eq!(
            scenarios[0]
                .primary_file()
                .expect("scenario should have a primary template")
                .path,
            "test.html"
        );
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

        let scenarios = ScenarioCollector::new(markdown)
            .collect()
            .expect("group-heading scenarios should parse");

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

        let error = ScenarioCollector::new(markdown)
            .collect()
            .expect_err("child heading after a source block should be rejected");

        assert_eq!(
            error,
            "scenario 'i18n / Valid' has child heading 'translates a literal after load' after its Django code block"
        );
    }

    #[test]
    fn rejects_second_unlabeled_source_block_in_one_heading() {
        let markdown = r#"# i18n

## translates a literal after load

`templates/greeting.html`:

```htmldjango
{% load i18n %}
{% trans "Hello" %}
```

```htmldjango
{% load i18n %}
{% trans "Goodbye" %}
```

```htmldjango
{% load i18n %}
{% trans "Later" %}
```
"#;

        let error = ScenarioCollector::new(markdown)
            .collect()
            .expect_err("second unlabeled source block should be rejected");

        assert_eq!(
            error,
            "scenario 'i18n / translates a literal after load' has more than one unlabeled htmldjango code block"
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
