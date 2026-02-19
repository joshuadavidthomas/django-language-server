use std::io::Read as _;
use std::io::Write as _;
use std::sync::Arc;

use anyhow::bail;
use anyhow::Context;
use anyhow::Result;
use camino::Utf8Path;
use camino::Utf8PathBuf;
use clap::Parser;
use djls_db::DjangoDatabase;
use djls_fmt::FormatConfig;
use djls_workspace::OsFileSystem;
use djls_workspace::WalkOptions;
use rayon::prelude::*;

use crate::args::Args;
use crate::commands::common::discover_files;
use crate::commands::common::resolve_project_root;
use crate::commands::common::ColorMode;
use crate::commands::Command;
use crate::exit::Exit;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OutputMode {
    WriteInPlace,
    Check,
    /// Diff-only mode always exits successfully; pair with `--check` to fail
    /// when formatting changes are needed.
    Diff,
    CheckWithDiff,
}

impl OutputMode {
    fn from_flags(check: bool, diff: bool) -> Self {
        match (check, diff) {
            (false, false) => Self::WriteInPlace,
            (true, false) => Self::Check,
            (false, true) => Self::Diff,
            (true, true) => Self::CheckWithDiff,
        }
    }

    fn should_check(self) -> bool {
        matches!(self, Self::Check | Self::CheckWithDiff)
    }

    fn should_print_diff(self) -> bool {
        matches!(self, Self::Diff | Self::CheckWithDiff)
    }

    fn should_write(self) -> bool {
        matches!(self, Self::WriteInPlace)
    }
}

#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Parser)]
pub struct Format {
    /// Files or directories to format. Pass `-` to read from stdin. If
    /// omitted, discovers template directories from the Django project.
    paths: Vec<Utf8PathBuf>,

    /// Exit with code 1 if any file would be reformatted.
    #[arg(long, default_value_t = false)]
    check: bool,

    /// Print unified diffs for files that would be reformatted. Does not
    /// change exit status; use `--check` to fail when formatting is needed.
    #[arg(long, default_value_t = false)]
    diff: bool,

    /// Include hidden files and directories (those starting with `.`).
    #[arg(short = '.', long, default_value_t = false)]
    hidden: bool,

    /// Include or exclude files matching a glob pattern. Prefix with `!` to
    /// exclude. May be specified multiple times. Later patterns take
    /// precedence.
    #[arg(short = 'g', long = "glob")]
    globs: Vec<String>,

    /// Don't respect ignore files (.gitignore, .ignore, etc.).
    #[arg(long, default_value_t = false)]
    no_ignore: bool,

    /// Follow symbolic links.
    #[arg(short = 'L', long, default_value_t = false)]
    follow: bool,

    /// Limit directory traversal depth.
    #[arg(short = 'd', long)]
    max_depth: Option<usize>,

    /// When to use colors.
    #[arg(long, value_enum, default_value_t = ColorMode::Auto)]
    color: ColorMode,
}

#[derive(Debug)]
struct FormattedFile {
    path: Utf8PathBuf,
    source: String,
    formatted: String,
}

impl FormattedFile {
    fn changed(&self) -> bool {
        self.source != self.formatted
    }
}

impl Command for Format {
    fn execute(&self, args: &Args) -> Result<Exit> {
        let project_root = resolve_project_root()?;
        let settings =
            djls_conf::Settings::new(&project_root, None).context("Failed to load settings")?;
        let format_config = settings.format().clone();
        let output_mode = OutputMode::from_flags(self.check, self.diff);

        let reading_stdin = self.paths.iter().any(|path| path.as_str() == "-");
        let has_non_stdin_path = self.paths.iter().any(|path| path.as_str() != "-");
        if reading_stdin && has_non_stdin_path {
            bail!("Cannot mix `-` (stdin) with file or directory paths");
        }

        if reading_stdin {
            return format_stdin(&format_config, output_mode, self.color, args.quiet);
        }

        let fs: Arc<dyn djls_workspace::FileSystem> = Arc::new(OsFileSystem);
        let db = DjangoDatabase::new(fs, &settings, Some(&project_root));

        let walk_options = WalkOptions {
            hidden: self.hidden,
            globs: self.globs.clone(),
            no_ignore: self.no_ignore,
            follow_links: self.follow,
            max_depth: self.max_depth,
        };

        let files = discover_files(&self.paths, &db, &project_root, &walk_options);

        if files.is_empty() {
            return Ok(Exit::success());
        }

        let formatted_files = files
            .into_par_iter()
            .map(|path| format_file(&path, &format_config))
            .collect::<Result<Vec<_>>>()?;

        apply_output_mode(output_mode, &formatted_files, self.color, args.quiet)
    }
}

fn apply_output_mode(
    mode: OutputMode,
    files: &[FormattedFile],
    color: ColorMode,
    quiet: bool,
) -> Result<Exit> {
    let changed_files: Vec<&FormattedFile> = files.iter().filter(|file| file.changed()).collect();

    if mode.should_write() {
        for file in &changed_files {
            std::fs::write(&file.path, &file.formatted)
                .with_context(|| format!("Failed to write formatted file: {}", file.path))?;
        }

        if !quiet && !changed_files.is_empty() {
            let count = changed_files.len();
            let noun = if count == 1 { "file" } else { "files" };
            println!("Formatted {count} {noun}.");
        }
    }

    if mode.should_print_diff() && !quiet {
        for file in &changed_files {
            println!("{}", render_diff(file, color));
        }
    }

    if mode.should_check() && !changed_files.is_empty() {
        if quiet {
            Ok(Exit::error())
        } else {
            let count = changed_files.len();
            let noun = if count == 1 {
                "file needs"
            } else {
                "files need"
            };
            Ok(Exit::error().with_message(format!("{count} {noun} formatting.")))
        }
    } else {
        Ok(Exit::success())
    }
}

fn format_file(path: &Utf8Path, format_config: &FormatConfig) -> Result<FormattedFile> {
    let source = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read file for formatting: {path}"))?;
    let formatted = djls_fmt::format_source(&source, format_config);

    Ok(FormattedFile {
        path: path.to_owned(),
        source,
        formatted,
    })
}

fn format_stdin(
    format_config: &FormatConfig,
    output_mode: OutputMode,
    color: ColorMode,
    quiet: bool,
) -> Result<Exit> {
    let mut source = String::new();
    std::io::stdin()
        .read_to_string(&mut source)
        .context("Failed to read stdin")?;

    let formatted = djls_fmt::format_source(&source, format_config);

    let file = FormattedFile {
        path: Utf8PathBuf::from("<stdin>.html"),
        source,
        formatted,
    };

    if output_mode.should_write() {
        std::io::stdout()
            .write_all(file.formatted.as_bytes())
            .context("Failed to write formatted output to stdout")?;
        return Ok(Exit::success());
    }

    apply_output_mode(output_mode, &[file], color, quiet)
}

fn render_diff(file: &FormattedFile, color_mode: ColorMode) -> String {
    let diff = djls_fmt::unified_diff(file.path.as_str(), &file.source, &file.formatted)
        .unwrap_or_default();

    if color_mode.should_use_color() {
        colorize_unified_diff(&diff)
    } else {
        diff
    }
}

fn colorize_unified_diff(diff: &str) -> String {
    let mut output = String::new();

    for line in diff.lines() {
        let colored = if line.starts_with("+++") {
            format!("\u{1b}[32m{line}\u{1b}[0m")
        } else if line.starts_with("---") {
            format!("\u{1b}[31m{line}\u{1b}[0m")
        } else if line.starts_with("@@") {
            format!("\u{1b}[36m{line}\u{1b}[0m")
        } else if line.starts_with('+') {
            format!("\u{1b}[32m{line}\u{1b}[0m")
        } else if line.starts_with('-') {
            format!("\u{1b}[31m{line}\u{1b}[0m")
        } else {
            line.to_owned()
        };
        output.push_str(&colored);
        output.push('\n');
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discover_files_finds_template_extensions() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let templates = root.join("templates");

        std::fs::create_dir_all(&templates).unwrap();
        std::fs::write(templates.join("base.html"), "<h1>hi</h1>").unwrap();
        std::fs::write(templates.join("partial.htm"), "<h1>hi</h1>").unwrap();
        std::fs::write(templates.join("email.djhtml"), "<h1>hi</h1>").unwrap();
        std::fs::write(templates.join("notes.txt"), "not a template").unwrap();

        let settings = djls_conf::Settings::new(&root, None).unwrap();
        let fs: Arc<dyn djls_workspace::FileSystem> = Arc::new(OsFileSystem);
        let db = DjangoDatabase::new(fs, &settings, Some(&root));

        let files = discover_files(
            &[root.join("templates")],
            &db,
            &root,
            &WalkOptions::default(),
        );

        let names: Vec<&str> = files.iter().filter_map(|path| path.file_name()).collect();
        assert!(names.contains(&"base.html"));
        assert!(names.contains(&"partial.htm"));
        assert!(names.contains(&"email.djhtml"));
        assert!(!names.contains(&"notes.txt"));
    }

    #[test]
    fn render_diff_outputs_unified_diff() {
        let file = FormattedFile {
            path: Utf8PathBuf::from("templates/page.html"),
            source: "<p>before</p>\n".to_owned(),
            formatted: "<p>after</p>\n".to_owned(),
        };

        let diff = render_diff(&file, ColorMode::Never);

        assert!(diff.contains("--- a/templates/page.html"));
        assert!(diff.contains("+++ b/templates/page.html"));
        assert!(diff.contains("-<p>before</p>"));
        assert!(diff.contains("+<p>after</p>"));
    }

    #[test]
    fn diff_mode_exits_zero_when_files_change() {
        let files = vec![FormattedFile {
            path: Utf8PathBuf::from("templates/page.html"),
            source: "before\n".to_owned(),
            formatted: "after\n".to_owned(),
        }];

        let exit = apply_output_mode(OutputMode::Diff, &files, ColorMode::Never, true).unwrap();

        assert_eq!(exit.as_raw(), 0);
    }

    #[test]
    fn check_mode_exits_one_when_files_change() {
        let files = vec![FormattedFile {
            path: Utf8PathBuf::from("templates/page.html"),
            source: "before\n".to_owned(),
            formatted: "after\n".to_owned(),
        }];

        let exit = apply_output_mode(OutputMode::Check, &files, ColorMode::Never, true).unwrap();

        assert_eq!(exit.as_raw(), 1);
    }
}
