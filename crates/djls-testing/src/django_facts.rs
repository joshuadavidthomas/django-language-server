use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;

use camino::Utf8PathBuf;
use djls_project::Interpreter;
use djls_project::Project;
use djls_project::PythonModuleName;
use djls_project::SearchPath;
use djls_project::SearchPaths;
use djls_project::TemplateSymbolKind;
use serde::Deserialize;

use crate::corpus::Corpus;
use crate::db::OsTestDatabase;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DjangoFactsGolden {
    pub template_dirs: Vec<String>,
    pub template_library_catalog: GoldenTemplateLibraryCatalog,
    #[serde(default)]
    pub template_verdicts: BTreeMap<String, TemplateVerdict>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoldenTemplateLibraryCatalog {
    pub builtins: Vec<String>,
    pub libraries: BTreeMap<String, String>,
    pub symbols: Vec<GoldenTemplateSymbol>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoldenTemplateSymbol {
    pub kind: TemplateSymbolKind,
    pub name: String,
    pub load_name: Option<String>,
    pub library_module: String,
    pub module: String,
}

#[derive(Deserialize)]
#[serde(tag = "verdict", rename_all = "lowercase", deny_unknown_fields)]
pub enum TemplateVerdict {
    Accepted,
    Rejected { error: String },
}

type DjangoFactsGoldenFixture = (
    OsTestDatabase,
    Project,
    Utf8PathBuf,
    Utf8PathBuf,
    DjangoFactsGolden,
);

pub fn django_facts_project(
    project_dir: &str,
    golden_file: &str,
    settings_module: &str,
) -> Result<DjangoFactsGoldenFixture, Box<dyn std::error::Error>> {
    let corpus = Corpus::require()?;
    let django_source_root = corpus.root().join("repos/django-5.2");
    if !django_source_root.join("django/__init__.py").is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "pinned Django 5.2 corpus source is missing",
        )
        .into());
    }

    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()?;
    let workspace = Utf8PathBuf::from_path_buf(workspace).map_err(|path| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("workspace path should be UTF-8: {}", path.display()),
        )
    })?;
    let project_root = workspace.join(project_dir);
    let golden_path = workspace.join(golden_file);
    let golden_source = std::fs::read_to_string(golden_path.as_std_path())?;
    let golden = serde_json::from_str(&golden_source)?;

    let mut db = OsTestDatabase::new();
    let interpreter = Interpreter::VenvPath(corpus.root().join("hermetic-no-venv"));
    let pythonpath = vec![django_source_root.clone()];
    let search_paths = SearchPaths::from_paths(vec![
        SearchPath::FirstParty(project_root.clone()),
        SearchPath::SitePackages(django_source_root.clone()),
    ]);
    search_paths.register_roots(&db);
    let project = Project::new(
        &db,
        project_root.clone(),
        search_paths,
        interpreter,
        Some(PythonModuleName::parse(settings_module)?),
        pythonpath,
        Vec::new(),
        djls_conf::Settings::default().tagspecs().clone(),
    );
    db.set_project(project);

    Ok((db, project, project_root, django_source_root, golden))
}
