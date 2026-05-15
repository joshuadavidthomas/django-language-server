use camino::Utf8PathBuf;
use djls_source::File;
use djls_source::Span;
use djls_templates::parse_template;
use djls_templates::Node;
use djls_templates::TagBit;

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

    pub(crate) fn tags(self, db: &'db dyn SemanticDb) -> &'db [Tag] {
        template_tags(db, self)
    }
}

// Tags are a cached projection of a template's parsed nodes, not independent
// Salsa identities. Keep the query boundary here and expose it through
// `Template::tags` so callers can still ask a template for its tags.
#[salsa::tracked(returns(ref))]
fn template_tags(db: &dyn SemanticDb, template: Template<'_>) -> Vec<Tag> {
    let file = template.file(db);
    let Some(nodelist) = parse_template(db, file) else {
        return Vec::new();
    };

    nodelist
        .nodelist(db)
        .iter()
        .filter_map(|node| match node {
            Node::Tag {
                name, bits, span, ..
            } => Some(Tag::new(name.clone(), bits.clone(), *span)),
            _ => None,
        })
        .collect()
}

#[salsa::interned]
pub struct TemplateName {
    #[returns(ref)]
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Tag {
    name: String,
    bits: Vec<TagBit>,
    span: Span,
}

impl Tag {
    #[must_use]
    pub fn new(name: String, bits: Vec<TagBit>, span: Span) -> Self {
        Self { name, bits, span }
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn bits(&self) -> &[TagBit] {
        &self.bits
    }

    #[must_use]
    pub fn span(&self) -> Span {
        self.span
    }
}
