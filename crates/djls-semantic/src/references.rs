use djls_project::LibraryName;
use djls_project::Project;
use djls_project::TemplateName;
use djls_project::TemplateOrigin;
use djls_project::TemplateResolution;
use djls_project::resolve_relative_name;
use djls_project::template_resolution;
use djls_source::File;
use djls_source::Span;
use djls_templates::TagBit;
use djls_templates::TemplateString;
use djls_templates::parse_template;
use rustc_hash::FxHashMap;

use crate::db::Db as SemanticDb;
use crate::scoping::LoadKind;
use crate::structure::active_template_tags;
use crate::structure::build_template_tree;
use crate::tags::TagRole;
use crate::tags::TagSpecs;
use crate::tags::compute_tag_specs;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub enum TemplateReferenceKind {
    Extends,
    Include,
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
    let resolution = template_resolution(db, project);

    for source in resolution.origins(db) {
        let source_name = source.template_name(db).name(db);
        for reference in template_references_in_file(db, project, source.file(db)).as_slice(db) {
            let raw_target_template_name = reference.target_template_name;
            let raw_target = raw_target_template_name.name(db);
            let Some(resolved_target) =
                resolve_relative_name(Some(source_name), raw_target, reference.kind.allow_self())
            else {
                continue;
            };
            let target_template_name = match resolved_target {
                std::borrow::Cow::Borrowed(_) => raw_target_template_name,
                std::borrow::Cow::Owned(name) => TemplateName::new(db, name),
            };
            let reference = TemplateReference::new(
                db,
                source,
                target_template_name,
                reference.kind,
                reference.span,
            );

            by_template_name
                .entry(target_template_name)
                .or_insert_with(Vec::new)
                .push(reference);
        }
    }

    TemplateReferences::new(db, by_template_name)
}

#[salsa::tracked]
pub struct TemplateReferencesInFile<'db> {
    #[tracked]
    #[returns(ref)]
    references: Vec<TemplateReferenceInFile<'db>>,
}

impl<'db> TemplateReferencesInFile<'db> {
    pub fn as_slice(self, db: &'db dyn SemanticDb) -> &'db [TemplateReferenceInFile<'db>] {
        self.references(db)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub struct TemplateReferenceInFile<'db> {
    target_template_name: TemplateName<'db>,
    kind: TemplateReferenceKind,
    span: Span,
}

impl<'db> TemplateReferenceInFile<'db> {
    #[must_use]
    pub fn target_template_name(self) -> TemplateName<'db> {
        self.target_template_name
    }

    #[must_use]
    pub fn span(self) -> Span {
        self.span
    }

    #[must_use]
    pub fn kind(self) -> TemplateReferenceKind {
        self.kind
    }
}

#[salsa::tracked]
pub struct TemplateLibraryReferencesInFile<'db> {
    #[tracked]
    #[returns(ref)]
    references: Vec<TemplateLibraryReferenceInFile>,
}

impl<'db> TemplateLibraryReferencesInFile<'db> {
    pub fn as_slice(self, db: &'db dyn SemanticDb) -> &'db [TemplateLibraryReferenceInFile] {
        self.references(db)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub struct TemplateLibraryReferenceInFile {
    load_name: LibraryName,
    span: Span,
}

impl TemplateReferenceKind {
    #[must_use]
    const fn allow_self(self) -> bool {
        match self {
            Self::Extends => false,
            Self::Include => true,
        }
    }
}

/// Resolve a single template reference written in `file`, anchored at the file's
/// primary template name (the template name Django binds the file to, first in
/// discovery order), honoring the reference kind's self-reference rule.
///
/// This anchor differs from `template_references` aggregation above, which
/// normalizes per source origin name; a file shadowed under multiple names
/// resolves per name there.
pub fn resolve_reference_name<'db>(
    db: &'db dyn SemanticDb,
    resolution: TemplateResolution<'db>,
    file: File,
    raw_name: TemplateName<'db>,
    kind: TemplateReferenceKind,
) -> Option<TemplateName<'db>> {
    let raw_name_text = raw_name.name(db);
    let current_template_name = resolution
        .primary_template_name(db, file)
        .map(|name| name.name(db).as_str());

    match resolve_relative_name(current_template_name, raw_name_text, kind.allow_self())? {
        std::borrow::Cow::Borrowed(_) => Some(raw_name),
        std::borrow::Cow::Owned(name) => Some(TemplateName::new(db, name)),
    }
}

impl TemplateLibraryReferenceInFile {
    #[must_use]
    pub fn load_name(&self) -> &LibraryName {
        &self.load_name
    }

    #[must_use]
    pub fn span(&self) -> Span {
        self.span
    }
}

#[salsa::tracked]
pub fn template_references_in_file(
    db: &dyn SemanticDb,
    project: Project,
    file: File,
) -> TemplateReferencesInFile<'_> {
    let tag_specs = compute_tag_specs(db, project);
    let Some(nodelist) = parse_template(db, file) else {
        return TemplateReferencesInFile::new(db, Vec::new());
    };
    let tree = build_template_tree(db, nodelist);

    let references = active_template_tags(tree.regions(db), tree.root(db))
        .into_iter()
        .filter_map(|tag| {
            let reference = LiteralTemplateReference::from_tag(tag_specs, tag.tag, tag.bits)?;
            Some(TemplateReferenceInFile {
                target_template_name: TemplateName::new(db, reference.template_name.to_string()),
                kind: reference.kind,
                span: reference.span,
            })
        })
        .collect();

    TemplateReferencesInFile::new(db, references)
}

#[salsa::tracked]
pub fn template_library_references_in_file(
    db: &dyn SemanticDb,
    file: File,
) -> TemplateLibraryReferencesInFile<'_> {
    let Some(nodelist) = parse_template(db, file) else {
        return TemplateLibraryReferencesInFile::new(db, Vec::new());
    };
    let tree = build_template_tree(db, nodelist);

    let references = active_template_tags(tree.regions(db), tree.root(db))
        .into_iter()
        .flat_map(|tag| literal_load_references_from_tag(tag.tag, tag.bits))
        .collect();

    TemplateLibraryReferencesInFile::new(db, references)
}

fn literal_load_references_from_tag(
    name: &str,
    bits: &[TagBit],
) -> Vec<TemplateLibraryReferenceInFile> {
    let Some(kind) = LoadKind::from_tag(name, bits) else {
        return Vec::new();
    };

    kind.into_library_arguments()
        .into_iter()
        .filter_map(|argument| {
            Some(TemplateLibraryReferenceInFile {
                load_name: LibraryName::parse(argument.as_str()).ok()?,
                span: argument.span(),
            })
        })
        .collect()
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
    fn source(self, db: &'db dyn SemanticDb) -> TemplateOrigin<'db> {
        self.source_origin(db)
    }

    pub fn source_file(self, db: &'db dyn SemanticDb) -> File {
        self.source(db).file(db)
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
    pub(crate) bit_span: Span,
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
        let TemplateString::Quoted {
            value: template_name,
            span,
        } = bit.template_string()
        else {
            return None;
        };

        Some(Self {
            kind,
            template_name,
            bit_span: bit.span,
            span,
        })
    }
}
