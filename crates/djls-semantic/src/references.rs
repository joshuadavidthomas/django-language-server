use djls_project::Project;
use djls_project::TemplateName;
use djls_project::TemplateOrigin;
use djls_project::template_origins;
use djls_source::File;
use djls_source::Span;
use djls_templates::Node;
use djls_templates::TagBit;
use djls_templates::parse_template;
use rustc_hash::FxHashMap;

use crate::db::Db as SemanticDb;
use crate::tags::TagRole;
use crate::tags::TagSpecs;
use crate::tags::compute_tag_specs;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub enum TemplateReferenceKind {
    Extends,
    Include,
}

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

#[salsa::tracked]
pub(crate) struct TemplateReferences<'db> {
    #[tracked]
    #[returns(ref)]
    by_template_name: FxHashMap<TemplateName<'db>, Vec<TemplateReference<'db>>>,
}

impl<'db> TemplateReferences<'db> {
    fn to_template_name(
        self,
        db: &'db dyn SemanticDb,
        template_name: TemplateName<'db>,
    ) -> &'db [TemplateReference<'db>] {
        self.by_template_name(db)
            .get(&template_name)
            .map_or(&[], Vec::as_slice)
    }
}

#[salsa::tracked]
pub(crate) fn template_references(db: &dyn SemanticDb, project: Project) -> TemplateReferences<'_> {
    let mut by_template_name = FxHashMap::default();
    let origins = template_origins(db, project);
    let tag_specs = compute_tag_specs(db, project);

    for source in origins.iter(db) {
        for tag in template_origin_tags(db, source) {
            let Some(reference) =
                LiteralTemplateReference::from_tag(tag_specs, tag.name(), tag.bits())
            else {
                continue;
            };

            let target_template_name = TemplateName::new(db, reference.template_name.to_string());
            let reference = TemplateReference::new(
                db,
                source,
                target_template_name,
                reference.kind,
                tag.span(),
            );

            by_template_name
                .entry(target_template_name)
                .or_insert_with(Vec::new)
                .push(reference);
        }
    }

    TemplateReferences::new(db, by_template_name)
}

pub fn references_to_template_name<'db>(
    db: &'db dyn SemanticDb,
    project: Project,
    template_name: TemplateName<'db>,
) -> &'db [TemplateReference<'db>] {
    template_references(db, project).to_template_name(db, template_name)
}

#[salsa::tracked]
pub struct TemplateReference<'db> {
    source_origin: TemplateOrigin<'db>,
    target_name: TemplateName<'db>,
    reference_kind: TemplateReferenceKind,
    reference_span: Span,
}

impl<'db> TemplateReference<'db> {
    pub fn source(self, db: &'db dyn SemanticDb) -> TemplateOrigin<'db> {
        self.source_origin(db)
    }

    pub fn source_file(self, db: &'db dyn SemanticDb) -> File {
        self.source(db).file(db)
    }

    pub fn target_template_name(self, db: &'db dyn SemanticDb) -> TemplateName<'db> {
        self.target_name(db)
    }

    pub fn kind(self, db: &dyn SemanticDb) -> TemplateReferenceKind {
        self.reference_kind(db)
    }

    pub fn span(self, db: &dyn SemanticDb) -> Span {
        self.reference_span(db)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LiteralTemplateReference<'bits> {
    pub(crate) kind: TemplateReferenceKind,
    pub(crate) template_name: &'bits str,
    pub(crate) span: Span,
}

impl<'bits> LiteralTemplateReference<'bits> {
    #[must_use]
    pub(crate) fn from_tag(
        tag_specs: &TagSpecs,
        tag_name: &str,
        bits: &'bits [TagBit],
    ) -> Option<Self> {
        let spec = tag_specs.get(tag_name)?;
        let Some(TagRole::TemplateReference(kind)) = spec.role() else {
            return None;
        };
        let bit = bits.first()?;
        let template_name = bit.template_string().quoted_value()?;

        Some(Self {
            kind,
            template_name,
            span: bit.span,
        })
    }
}
