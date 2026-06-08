use camino::Utf8PathBuf;
use djls_source::File;
use djls_source::Span;
use djls_templates::Node;
use djls_templates::TagBit;
use djls_templates::parse_template;

use crate::db::Db as SemanticDb;

#[salsa::tracked]
pub struct TemplateOrigin<'db> {
    pub template_name: TemplateName<'db>,
    pub file: File,
}

impl<'db> TemplateOrigin<'db> {
    pub fn path_buf(&'db self, db: &'db dyn SemanticDb) -> &'db Utf8PathBuf {
        self.file(db).path(db)
    }

    pub(crate) fn tags(self, db: &'db dyn SemanticDb) -> &'db [Tag] {
        template_origin_tags(db, self)
    }
}

// Tags are a cached projection of a template origin's parsed nodes, not
// independent Salsa identities. Keep the query boundary here and expose it
// through `TemplateOrigin::tags` so callers can still ask an origin for its tags.
#[salsa::tracked(returns(ref))]
fn template_origin_tags(db: &dyn SemanticDb, origin: TemplateOrigin<'_>) -> Vec<Tag> {
    let file = origin.file(db);
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
    pub(crate) fn new(name: String, bits: Vec<TagBit>, span: Span) -> Self {
        Self { name, bits, span }
    }

    #[must_use]
    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub(crate) fn bits(&self) -> &[TagBit] {
        &self.bits
    }

    #[must_use]
    pub(crate) fn span(&self) -> Span {
        self.span
    }
}
