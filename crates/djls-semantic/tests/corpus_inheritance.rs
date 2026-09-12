use camino::Utf8Component;
use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_semantic::template_inheritance;
use djls_templates::parse_template;
use djls_testing::Corpus;
use djls_testing::ProjectFixture;
use djls_testing::ProjectSettings;
use djls_testing::TestDatabase;
use libtest_mimic::Arguments;
use libtest_mimic::Trial;

#[expect(
    clippy::expect_used,
    reason = "corpus fixture failures should fail their named trial"
)]
fn inheritance_terminates(entry_dir: Utf8PathBuf, templates: Vec<Utf8PathBuf>) {
    let template_roots = template_roots(&templates);
    let fixture = ProjectFixture::new(entry_dir).settings(&ProjectSettings {
        dirs: template_roots.iter().map(ToString::to_string).collect(),
        ..ProjectSettings::default()
    });
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
    let mut parsed_count = 0usize;
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

        parsed_count += 1;
        let inheritance = template_inheritance(&db, project, file);
        let _chain_end = inheritance.end(&db);
    }

    assert!(parsed_count > 0, "No corpus templates parsed.");
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

fn main() -> anyhow::Result<()> {
    let args = Arguments::from_args();
    let corpus = Corpus::require()?;
    let trials = corpus
        .locked_repos()
        .filter_map(|(repo_name, entry_dir)| {
            let templates = corpus.templates_in(&entry_dir);
            (!templates.is_empty()).then(|| {
                Trial::test(repo_name, move || {
                    inheritance_terminates(entry_dir, templates);
                    Ok(())
                })
            })
        })
        .collect();

    libtest_mimic::run(&args, trials).exit()
}
