use camino::Utf8PathBuf;
use djls_source::safe_join;
use djls_source::Span;
use djls_source::Utf8PathClean;
use djls_templates::parse_template;
use djls_templates::Node;
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

#[derive(Clone, PartialEq, Eq)]
pub struct TemplateReference {
    pub referencing_path: Utf8PathBuf,
    pub referenced_name: String,
    pub tag_span: Span,
    pub tag_name: String,
}

pub fn find_references_to_template(db: &dyn SemanticDb, name: &str) -> Vec<TemplateReference> {
    let template_name = TemplateName::new(db, name.to_string());
    find_references_to_template_tracked(db, template_name)
}

#[salsa::tracked]
fn find_references_to_template_tracked<'db>(
    db: &'db dyn SemanticDb,
    template_name: TemplateName<'db>,
) -> Vec<TemplateReference> {
    let all_refs = index_template_references(db);
    let name_str = template_name.name(db);

    let matches: Vec<_> = all_refs
        .into_iter()
        .filter(|r| r.referenced_name == *name_str)
        .collect();

    tracing::debug!("Found {} references to '{}'", matches.len(), name_str);
    matches
}

#[salsa::tracked]
fn index_template_references(db: &dyn SemanticDb) -> Vec<TemplateReference> {
    let mut references = Vec::new();
    let templates = discover_templates(db);

    for template in templates {
        let path = template.path_buf(db);
        let file = djls_source::File::new(db, path.clone(), 0);

        let Some(nodelist) = parse_template(db, file) else {
            continue;
        };

        for extracted in collect_template_references(nodelist.nodelist(db)) {
            references.push(TemplateReference {
                referencing_path: path.clone(),
                referenced_name: extracted.referenced_name,
                tag_span: extracted.span,
                tag_name: extracted.tag_name,
            });
        }
    }

    references
}

struct ExtractedReference {
    referenced_name: String,
    span: Span,
    tag_name: String,
}

fn collect_template_references(nodes: &[Node]) -> Vec<ExtractedReference> {
    let mut references = Vec::new();

    for node in nodes {
        match node {
            Node::Tag {
                name, bits, span, ..
            } if name == "extends" || name == "include" => {
                if let Some(template_str) = bits.first() {
                    let template_name = template_str
                        .trim()
                        .trim_start_matches('"')
                        .trim_end_matches('"')
                        .trim_start_matches('\'')
                        .trim_end_matches('\'')
                        .to_string();

                    references.push(ExtractedReference {
                        referenced_name: template_name,
                        span: *span,
                        tag_name: name.clone(),
                    });
                }
            }
            _ => {}
        }
    }

    references
}
