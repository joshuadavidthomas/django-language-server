use std::collections::BTreeMap;

use camino::Utf8Component;
use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_semantic::ChainEnd;
use djls_semantic::template_inheritance;
use djls_templates::parse_template;
use djls_testing::Corpus;
use djls_testing::ProjectFixture;
use djls_testing::TestDatabase;

#[test]
fn corpus_template_inheritance_terminates() {
    let corpus = Corpus::require().expect("synced corpus should be available for corpus tests");
    let mut by_entry: BTreeMap<Utf8PathBuf, Vec<Utf8PathBuf>> = BTreeMap::new();

    for template_path in corpus.templates_in(corpus.root()) {
        let Some(entry_dir) = corpus.entry_dir_for_path(&template_path) else {
            continue;
        };
        by_entry.entry(entry_dir).or_default().push(template_path);
    }

    let mut distribution = ChainEndDistribution::default();
    let mut template_count = 0usize;

    for (entry_dir, mut templates) in by_entry {
        templates.sort();
        let template_roots = template_roots(&templates);
        if template_roots.is_empty() {
            continue;
        }

        let settings_source = format!(
            "INSTALLED_APPS = []\nTEMPLATES = [{{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [{}], 'APP_DIRS': False}}]\n",
            template_roots
                .iter()
                .map(|root| format!("'{root}'"))
                .collect::<Vec<_>>()
                .join(", ")
        );
        let settings_path = entry_dir.join("djls_corpus_settings.py");
        let fixture = ProjectFixture::new(entry_dir.clone())
            .django_settings_module("djls_corpus_settings")
            .file(settings_path, settings_source);
        let db = TestDatabase::new();

        let mut fixture = fixture;
        for template_path in &templates {
            let Ok(source) = std::fs::read_to_string(template_path.as_std_path()) else {
                continue;
            };
            fixture = fixture.file(template_path.clone(), source);
        }

        let project = fixture
            .build(&db)
            .expect("corpus project fixture should build in the test database");
        for template_path in templates {
            let file = db
                .file(&template_path)
                .expect("corpus template should exist in the test database");
            if !matches!(
                parse_template(&db, file),
                djls_templates::TemplateParseResult::Parsed(_)
            ) {
                continue;
            }

            let inheritance = template_inheritance(&db, project, file);
            distribution.record(&inheritance.end(&db));
            template_count += 1;
        }
    }

    assert!(template_count > 0, "No corpus templates discovered.");
    println!("ChainEnd distribution across {template_count} corpus templates:");
    println!("  Root: {}", distribution.root);
    println!("  Dynamic: {}", distribution.dynamic);
    println!("  Unresolved: {}", distribution.unresolved);
    println!("  InconclusiveParent: {}", distribution.inconclusive_parent);
    println!("  Cycle: {}", distribution.cycle);
}

#[derive(Default)]
struct ChainEndDistribution {
    root: usize,
    dynamic: usize,
    unresolved: usize,
    inconclusive_parent: usize,
    cycle: usize,
}

impl ChainEndDistribution {
    fn record(&mut self, end: &ChainEnd) {
        match *end {
            ChainEnd::Root => self.root += 1,
            ChainEnd::Dynamic { .. } => self.dynamic += 1,
            ChainEnd::Unresolved { .. } => self.unresolved += 1,
            ChainEnd::InconclusiveParent { .. } => self.inconclusive_parent += 1,
            ChainEnd::Cycle => self.cycle += 1,
        }
    }
}

fn template_roots(templates: &[Utf8PathBuf]) -> Vec<Utf8PathBuf> {
    let mut roots = templates
        .iter()
        .filter_map(|template| template_root(template))
        .collect::<Vec<_>>();
    roots.sort();
    roots.dedup();
    roots
}

fn template_root(path: &Utf8Path) -> Option<Utf8PathBuf> {
    let mut root = Utf8PathBuf::new();
    for component in path.components() {
        match component {
            Utf8Component::RootDir => root.push("/"),
            Utf8Component::CurDir => {}
            Utf8Component::ParentDir | Utf8Component::Prefix(_) => return None,
            Utf8Component::Normal(part) => {
                root.push(part);
                if part == "templates" {
                    return Some(root);
                }
            }
        }
    }
    None
}
