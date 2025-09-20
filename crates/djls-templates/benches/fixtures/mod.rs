#![allow(dead_code)]

use std::env;
use std::fs;
use std::io;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

use camino::Utf8PathBuf;

#[derive(Clone)]
pub struct Fixture {
    pub name: String,
    pub slug: String,
    pub contents: String,
}

impl Fixture {
    pub fn new(
        name: impl Into<String>,
        slug: impl Into<String>,
        contents: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            slug: slug.into(),
            contents: contents.into(),
        }
    }

    #[must_use]
    pub fn file_path(&self) -> Utf8PathBuf {
        Utf8PathBuf::from(format!("bench_{}.html", self.slug))
    }
}

macro_rules! fixture {
    ($name:literal, $slug:literal, $file:literal) => {
        Fixture::new(
            $name,
            $slug,
            include_str!(concat!(
                env!("CARGO_WORKSPACE_DIR"),
                "/tests/project/djls_app/templates/bench/",
                $file
            )),
        )
    };
}

pub fn lex_parse_fixtures() -> Vec<Fixture> {
    let mut fixtures = synthetic_lex_parse_fixtures();
    fixtures.extend(load_django_admin_fixtures());
    fixtures
}

pub fn validation_fixtures() -> Vec<Fixture> {
    let mut fixtures = synthetic_validation_fixtures();
    fixtures.extend(load_django_admin_fixtures());
    fixtures
}

fn synthetic_lex_parse_fixtures() -> Vec<Fixture> {
    vec![
        fixture!("Simple Dashboard", "simple", "simple.html"),
        fixture!("Kanban Dashboard", "dashboard", "dashboard.html"),
        fixture!("Knowledge Base", "knowledge_base", "knowledge_base.html"),
        fixture!(
            "Completion Hotspots",
            "completion_hotspots",
            "completion_hotspots.html"
        ),
        fixture!("Micro Blocks", "micro_blocks", "micro_blocks.html"),
        fixture!("Custom Tags", "custom_tags", "custom_tags.html"),
        fixture!(
            "Translation Heavy",
            "translation_heavy",
            "translation_heavy.html"
        ),
        fixture!("Minified Stream", "minified_stream", "minified_stream.html"),
        fixture!("Filter Chains", "filter_chains", "filter_chains.html"),
    ]
}

fn synthetic_validation_fixtures() -> Vec<Fixture> {
    let mut fixtures = synthetic_lex_parse_fixtures();
    fixtures.push(fixture!("Error Storm", "error_storm", "error_storm.html"));
    fixtures.push(fixture!(
        "Invalid Block",
        "invalid_block",
        "invalid_block.html"
    ));
    fixtures
}

static DJANGO_ADMIN_FIXTURES: OnceLock<Vec<Fixture>> = OnceLock::new();

fn load_django_admin_fixtures() -> Vec<Fixture> {
    DJANGO_ADMIN_FIXTURES
        .get_or_init(|| {
            let Some(root) = discover_django_admin_templates() else {
                return Vec::new();
            };

            let limit = env::var("DJLS_BENCH_MAX_DJANGO_TEMPLATES")
                .ok()
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(30);

            let mut collected = Vec::new();
            if let Err(err) = collect_templates(&root, limit, &mut collected) {
                eprintln!(
                    "djls bench: failed to load Django templates from {}: {}",
                    root.display(),
                    err
                );
                return Vec::new();
            }

            collected
        })
        .clone()
}

fn discover_django_admin_templates() -> Option<PathBuf> {
    if let Ok(path) = env::var("DJLS_BENCH_DJANGO_TEMPLATE_ROOT") {
        return Some(PathBuf::from(path));
    }

    let python = env::var("DJLS_BENCH_PYTHON").unwrap_or_else(|_| "python".to_string());
    let script = "import django, os, sys; path = os.path.join(os.path.dirname(django.__file__), 'contrib', 'admin', 'templates'); sys.stdout.write(path)";

    match Command::new(python).arg("-c").arg(script).output() {
        Ok(output) if output.status.success() => {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if path.is_empty() {
                None
            } else {
                Some(PathBuf::from(path))
            }
        }
        Ok(output) => {
            eprintln!(
                "djls bench: python reported an error discovering Django templates: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            None
        }
        Err(err) => {
            eprintln!("djls bench: unable to invoke python to locate Django templates: {err}");
            None
        }
    }
}

fn collect_templates(root: &Path, limit: usize, fixtures: &mut Vec<Fixture>) -> io::Result<()> {
    let mut stack = vec![root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                stack.push(path);
                continue;
            }

            if path.extension().and_then(|ext| ext.to_str()) != Some("html") {
                continue;
            }

            let rel = path.strip_prefix(root).unwrap_or(&path);
            let slug = format!("django_admin_{}", slug_from_rel(rel));
            let name = format!("django admin: {}", rel.display());
            let contents = fs::read_to_string(&path)?;

            fixtures.push(Fixture::new(name, slug, contents));

            if fixtures.len() >= limit {
                return Ok(());
            }
        }
    }

    Ok(())
}

fn slug_from_rel(path: &Path) -> String {
    let raw = path.to_string_lossy();
    raw.chars()
        .map(|ch| match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' => ch.to_ascii_lowercase(),
            _ => '_',
        })
        .collect()
}
