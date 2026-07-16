use djls_project::FindTemplateResult;
use djls_project::InconclusiveTemplateSearch;
use djls_project::LibraryName;
use djls_project::Project;
use djls_project::ScopedTemplateReferenceResolution;
use djls_project::TemplateName;
use djls_project::TemplateOrigin;
use djls_project::TemplateResolution;
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
use crate::tags::TagRole;

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
        for reference in template_references_in_file(db, project, source.file(db)).as_slice(db) {
            let Some(scoped) = reference.kind.resolve_from_origin(
                db,
                resolution,
                source,
                reference.target_template_name,
            ) else {
                continue;
            };
            match scoped.result {
                FindTemplateResult::Found(_) => {}
                // Possible origins surviving an incomplete scoped search are still real reference
                // targets worth indexing; an inconclusive miss with no candidates indexes
                // nothing rather than guessing across backends.
                FindTemplateResult::Inconclusive(search) if !search.possible_origins.is_empty() => {
                }
                FindTemplateResult::DoesNotExist(_) | FindTemplateResult::Inconclusive(_) => {
                    continue;
                }
            }

            let reference = TemplateReference::new(
                db,
                source,
                scoped.target_name,
                reference.kind,
                reference.span,
            );

            by_template_name
                .entry(scoped.target_name)
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

    fn resolve_from_origin<'db>(
        self,
        db: &'db dyn SemanticDb,
        resolution: TemplateResolution<'db>,
        source: TemplateOrigin<'db>,
        raw_name: TemplateName<'db>,
    ) -> Option<ScopedTemplateReferenceResolution<'db>> {
        let excluded = match self {
            Self::Extends => std::slice::from_ref(&source),
            Self::Include => &[],
        };
        resolution.resolve_reference_from_origin(db, source, raw_name, excluded, self.allow_self())
    }
}

/// Per-origin normalization and backend-scoped resolution of one raw file reference.
pub fn resolve_reference_origins<'db>(
    db: &'db dyn SemanticDb,
    resolution: TemplateResolution<'db>,
    file: File,
    raw_name: TemplateName<'db>,
    kind: TemplateReferenceKind,
) -> Vec<ScopedTemplateReferenceResolution<'db>> {
    resolution
        .template_names_for_file(db, file)
        .iter()
        .flat_map(|name| resolution.origins_for_name(db, *name))
        .filter(|origin| origin.file(db) == file)
        .filter_map(|origin| kind.resolve_from_origin(db, resolution, *origin, raw_name))
        .collect()
}

/// Joined resolution for an IDE operation on a physical file.
///
/// Relative references are normalized independently for every source origin. A definitive target
/// is returned only when all origin/backend-scoped outcomes select the same physical file.
pub fn resolve_reference_for_file<'db>(
    db: &'db dyn SemanticDb,
    resolution: TemplateResolution<'db>,
    file: File,
    raw_name: TemplateName<'db>,
    kind: TemplateReferenceKind,
) -> Option<FindTemplateResult<'db>> {
    let outcomes = resolve_reference_origins(db, resolution, file, raw_name, kind);
    let Some(first) = outcomes.first() else {
        // Files outside the template inventory have no origin from which to derive a backend
        // scope or normalize a relative name. Absolute names can still use the project-wide
        // resolution, which keeps feasible settings alternatives separate when selecting a
        // winner; relative names deliberately remain unresolved.
        djls_project::resolve_relative_name(None, raw_name.name(db), kind.allow_self())?;
        return Some(resolution.resolve(db, raw_name));
    };
    let found_file = match first.result {
        FindTemplateResult::Found(origin) => Some(origin.file(db)),
        FindTemplateResult::DoesNotExist(_) | FindTemplateResult::Inconclusive(_) => None,
    };
    if let Some(file) = found_file
        && outcomes.iter().all(
            |outcome| matches!(outcome.result, FindTemplateResult::Found(origin) if origin.file(db) == file),
        )
    {
        let FindTemplateResult::Found(origin) = first.result else {
            unreachable!("the joined reference outcome was checked as found")
        };
        return Some(FindTemplateResult::Found(origin));
    }

    if outcomes
        .iter()
        .all(|outcome| matches!(outcome.result, FindTemplateResult::DoesNotExist(_)))
    {
        return Some(first.result.clone());
    }

    let mut possible_origins = Vec::new();
    for outcome in &outcomes {
        match &outcome.result {
            FindTemplateResult::Found(origin) => {
                if !possible_origins.iter().any(|possible| possible == origin) {
                    possible_origins.push(*origin);
                }
            }
            FindTemplateResult::Inconclusive(search) => {
                for origin in &search.possible_origins {
                    if !possible_origins.iter().any(|possible| possible == origin) {
                        possible_origins.push(*origin);
                    }
                }
            }
            FindTemplateResult::DoesNotExist(_) => {}
        }
    }
    Some(FindTemplateResult::Inconclusive(
        InconclusiveTemplateSearch {
            name: first.target_name,
            possible_origins,
        },
    ))
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
    _project: Project,
    file: File,
) -> TemplateReferencesInFile<'_> {
    let djls_templates::TemplateParseResult::Parsed(nodelist) = parse_template(db, file) else {
        return TemplateReferencesInFile::new(db, Vec::new());
    };
    let projection = crate::scoping::template_analysis_projection_for_file(db, file, nodelist);
    let tree = projection.tree(db);
    let tag_facts = projection.scoped_tag_facts(db);

    let references = active_template_tags(tree.regions(db), tree.root(db))
        .into_iter()
        .filter_map(|tag| {
            let spec = tag_facts.for_tag(tag)?.spec.as_ref()?;
            let reference = LiteralTemplateReference::from_spec(spec, tag.bits)?;
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
    let djls_templates::TemplateParseResult::Parsed(nodelist) = parse_template(db, file) else {
        return TemplateLibraryReferencesInFile::new(db, Vec::new());
    };
    let projection = crate::scoping::template_analysis_projection_for_file(db, file, nodelist);
    let tree = projection.tree(db);
    let tag_facts = projection.scoped_tag_facts(db);

    let references = active_template_tags(tree.regions(db), tree.root(db))
        .into_iter()
        .filter(|tag| {
            tag_facts
                .for_tag(*tag)
                .and_then(|facts| facts.spec.as_ref())
                .and_then(crate::TagSpec::role)
                == Some(TagRole::TemplateLibraryLoader)
        })
        .flat_map(|tag| literal_load_references_from_tag(tag.bits))
        .collect();

    TemplateLibraryReferencesInFile::new(db, references)
}

fn literal_load_references_from_tag(bits: &[TagBit]) -> Vec<TemplateLibraryReferenceInFile> {
    let Some(kind) = LoadKind::from_loader_bits(bits) else {
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

    fn target_template_name(self, db: &'db dyn SemanticDb) -> TemplateName<'db> {
        self.target_name(db)
    }

    pub fn kind(self, db: &dyn SemanticDb) -> TemplateReferenceKind {
        self.reference_kind(db)
    }

    pub fn resolve(
        self,
        db: &'db dyn SemanticDb,
        resolution: TemplateResolution<'db>,
    ) -> Option<ScopedTemplateReferenceResolution<'db>> {
        self.kind(db).resolve_from_origin(
            db,
            resolution,
            self.source(db),
            self.target_template_name(db),
        )
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
    pub(crate) fn from_spec(spec: &crate::tags::TagSpec, bits: &'bits [TagBit]) -> Option<Self> {
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
