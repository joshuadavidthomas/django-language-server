use camino::Utf8PathBuf;
use djls_source::safe_join;
use djls_source::Utf8PathClean;
use walkdir::WalkDir;

pub use crate::db::Db as SemanticDb;

#[salsa::tracked]
pub struct Template<'db> {
    name: TemplateName<'db>,
    #[returns(ref)]
    path: Utf8PathBuf,
}

impl<'db> Template<'db> {
    pub fn name_str(&'db self, db: &'db dyn SemanticDb) -> &'db str {
        self.name(db).name(db)
    }

    pub fn path_buf(&'db self, db: &'db dyn SemanticDb) -> &'db Utf8PathBuf {
        self.path(db)
    }
}

#[salsa::interned]
pub struct TemplateName {
    #[returns(ref)]
    name: String,
}

#[salsa::tracked]
pub fn discover_templates(db: &dyn SemanticDb) -> Vec<Template<'_>> {
    let mut templates = Vec::new();

    if let Some(search_dirs) = db.template_dirs() {
        tracing::debug!("Discovering templates in {} directories", search_dirs.len());

        for dir in &search_dirs {
            if !dir.exists() {
                tracing::warn!("Template directory does not exist: {}", dir);
                continue;
            }

            for entry in WalkDir::new(dir)
                .into_iter()
                .filter_map(std::result::Result::ok)
                .filter(|e| e.file_type().is_file())
            {
                let Ok(path) = Utf8PathBuf::from_path_buf(entry.path().to_path_buf()) else {
                    continue;
                };

                let name = match path.strip_prefix(dir) {
                    Ok(rel) => rel.clean().to_string(),
                    Err(_) => continue,
                };

                templates.push(Template::new(db, TemplateName::new(db, name), path));
            }
        }
    } else {
        tracing::warn!("No template directories configured");
    }

    tracing::debug!("Discovered {} total templates", templates.len());
    templates
}

#[salsa::tracked]
pub fn find_template<'db>(
    db: &'db dyn SemanticDb,
    template_name: TemplateName<'db>,
) -> Option<Template<'db>> {
    let templates = discover_templates(db);

    templates
        .iter()
        .find(|t| t.name(db) == template_name)
        .copied()
}

#[derive(Clone, PartialEq, salsa::Update)]
pub enum ResolveResult<'db> {
    Found(Template<'db>),
    NotFound {
        name: String,
        tried: Vec<Utf8PathBuf>,
    },
}

impl<'db> ResolveResult<'db> {
    #[must_use] 
    pub fn ok(self) -> Option<Template<'db>> {
        match self {
            Self::Found(t) => Some(t),
            Self::NotFound { .. } => None,
        }
    }

    #[must_use] 
    pub fn is_found(&self) -> bool {
        matches!(self, Self::Found(_))
    }
}

pub fn resolve_template<'db>(db: &'db dyn SemanticDb, name: &str) -> ResolveResult<'db> {
    let template_name = TemplateName::new(db, name.to_string());
    if let Some(template) = find_template(db, template_name) {
        return ResolveResult::Found(template);
    }

    let tried = db
        .template_dirs()
        .map(|dirs| {
            dirs.iter()
                .filter_map(|d| safe_join(d, name).ok())
                .collect()
        })
        .unwrap_or_default();

    ResolveResult::NotFound {
        name: name.to_string(),
        tried,
    }
}
