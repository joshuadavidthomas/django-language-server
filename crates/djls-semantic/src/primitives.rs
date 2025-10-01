use camino::Utf8PathBuf;
use djls_source::File;
use djls_source::Span;
use djls_templates::parse_template;
use djls_templates::Node;

use crate::db::Db as SemanticDb;

#[salsa::tracked]
pub struct Template<'db> {
    pub name: TemplateName<'db>,
    pub file: File,
}

impl<'db> Template<'db> {
    pub fn path_buf(&'db self, db: &'db dyn SemanticDb) -> &'db Utf8PathBuf {
        self.file(db).path(db)
    }

    pub fn tags(&self, db: &'db dyn SemanticDb) -> Vec<Tag<'db>> {
        let file = self.file(db);
        let Some(nodelist) = parse_template(db, file) else {
            return Vec::new();
        };

        nodelist
            .nodelist(db)
            .iter()
            .filter_map(|node| match node {
                Node::Tag { name, bits, span } => {
                    Some(Tag::new(db, name.clone(), bits.clone(), *span))
                }
                _ => None,
            })
            .collect()
    }
}

#[salsa::interned]
pub struct TemplateName {
    #[returns(ref)]
    pub name: String,
}

#[salsa::tracked]
pub struct Tag<'db> {
    #[returns(ref)]
    pub name: String,
    #[returns(ref)]
    pub arguments: Vec<String>,
    pub span: Span,
}
