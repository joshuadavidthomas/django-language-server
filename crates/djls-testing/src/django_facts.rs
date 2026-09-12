use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;

use camino::Utf8PathBuf;
use djls_project::Project;
use djls_project::TemplateSymbolKind;
use serde::Deserialize;

use crate::db::OsTestDatabase;
use crate::fixtures::corpus_project_database;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DjangoFactsGolden {
    pub template_dirs: Vec<String>,
    pub template_library_catalog: GoldenTemplateLibraryCatalog,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompilationGolden {
    pub django_compilation: BTreeMap<String, DjangoCompilation>,
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
#[serde(tag = "result", rename_all = "lowercase", deny_unknown_fields)]
pub enum DjangoCompilation {
    Compiled,
    Failed { error: String },
}

type DjangoFactsProject = (OsTestDatabase, Project, Utf8PathBuf, Utf8PathBuf);

pub fn django_facts_project(
    project_dir: &str,
    settings_module: &str,
) -> Result<DjangoFactsProject, Box<dyn std::error::Error>> {
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
    let (db, project, django_source_root) = corpus_project_database(
        project_root.clone(),
        [project_root.clone()],
        settings_module,
    )?;

    Ok((db, project, project_root, django_source_root))
}
