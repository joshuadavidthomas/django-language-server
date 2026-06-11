use camino::Utf8PathBuf;
use djls_source::File;
use djls_source::Span;
use djls_source::safe_join;
use djls_templates::Node;
use djls_templates::TagBit;
use djls_templates::parse_template;
use rustc_hash::FxHashMap;

use crate::db::Db as SemanticDb;
use crate::project::Project;
use crate::project::TemplateDirs;
use crate::tags::TagRole;
use crate::tags::TagSpecs;
use crate::tags::compute_tag_specs;

#[salsa::interned]
#[derive(Debug)]
pub struct TemplateName {
    #[returns(ref)]
    pub name: String,
}

#[salsa::tracked]
pub struct TemplateOrigin<'db> {
    resolved_template_name: TemplateName<'db>,
    template_file: File,
}

impl<'db> TemplateOrigin<'db> {
    pub fn template_name(self, db: &'db dyn SemanticDb) -> TemplateName<'db> {
        self.resolved_template_name(db)
    }

    pub fn file(self, db: &'db dyn SemanticDb) -> File {
        self.template_file(db)
    }

    pub fn path_buf(self, db: &'db dyn SemanticDb) -> &'db Utf8PathBuf {
        self.file(db).path(db)
    }

    pub(crate) fn tags(self, db: &'db dyn SemanticDb) -> &'db [Tag] {
        template_origin_tags(db, self)
    }
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub enum TemplateReferenceKind {
    Extends,
    Include,
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
pub(crate) struct TemplateOrigins<'db> {
    #[tracked]
    #[returns(ref)]
    ordered: Vec<TemplateOrigin<'db>>,
    #[tracked]
    #[returns(ref)]
    first_by_template_name: FxHashMap<TemplateName<'db>, TemplateOrigin<'db>>,
    #[tracked]
    #[returns(ref)]
    template_dirs: TemplateDirs,
}

impl<'db> TemplateOrigins<'db> {
    pub fn iter(self, db: &'db dyn SemanticDb) -> impl Iterator<Item = TemplateOrigin<'db>> + 'db {
        self.ordered(db).iter().copied()
    }

    #[must_use]
    pub fn find_template(
        self,
        db: &'db dyn SemanticDb,
        template_name: TemplateName<'db>,
    ) -> FindTemplateResult<'db> {
        if let Some(origin) = self.first_by_template_name(db).get(&template_name) {
            return FindTemplateResult::Found(*origin);
        }

        let name = template_name.name(db);
        let tried = self
            .template_dirs(db)
            .as_known()
            .map(|dirs| {
                dirs.iter()
                    .filter_map(|dir| safe_join(dir, name).ok())
                    .map(|path| TriedTemplateSource { path })
                    .collect()
            })
            .unwrap_or_default();

        FindTemplateResult::DoesNotExist(TemplateDoesNotExist {
            template_name,
            tried,
        })
    }
}

#[salsa::tracked]
pub(crate) fn template_origins(db: &dyn SemanticDb, project: Project) -> TemplateOrigins<'_> {
    let mut ordered = Vec::new();
    let mut first_by_template_name = FxHashMap::default();

    for template in project.template_files(db).iter() {
        let template_name = TemplateName::new(db, template.name().to_string());
        let origin = TemplateOrigin::new(db, template_name, template.file());

        first_by_template_name
            .entry(template_name)
            .or_insert(origin);
        ordered.push(origin);
    }

    tracing::debug!("Discovered {} total template origins", ordered.len());

    TemplateOrigins::new(
        db,
        ordered,
        first_by_template_name,
        project.template_dirs(db).clone(),
    )
}

pub fn find_template<'db>(
    db: &'db dyn SemanticDb,
    project: Project,
    template_name: TemplateName<'db>,
) -> FindTemplateResult<'db> {
    template_origins(db, project).find_template(db, template_name)
}

#[derive(Clone, PartialEq)]
pub enum FindTemplateResult<'db> {
    Found(TemplateOrigin<'db>),
    DoesNotExist(TemplateDoesNotExist<'db>),
}

impl<'db> FindTemplateResult<'db> {
    #[must_use]
    pub fn ok(self) -> Option<TemplateOrigin<'db>> {
        match self {
            Self::Found(origin) => Some(origin),
            Self::DoesNotExist(_) => None,
        }
    }

    #[must_use]
    pub fn is_found(&self) -> bool {
        matches!(self, Self::Found(_))
    }
}

#[derive(Clone, PartialEq)]
pub struct TemplateDoesNotExist<'db> {
    pub template_name: TemplateName<'db>,
    pub tried: Vec<TriedTemplateSource>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TriedTemplateSource {
    pub path: Utf8PathBuf,
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
        for tag in source.tags(db) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::TemplateDirs;
    use crate::testing::ProjectFixture;
    use crate::testing::TestDatabase;

    fn project_with_templates(
        db: &mut TestDatabase,
        template_dirs: Vec<&str>,
        templates: Vec<(&str, &str, &str)>,
    ) -> Project {
        let template_dirs =
            TemplateDirs::Known(template_dirs.into_iter().map(Into::into).collect());
        templates
            .into_iter()
            .fold(
                ProjectFixture::new("/test/project").template_dirs(template_dirs),
                |fixture, (name, path, source)| fixture.template_file(name, path, source),
            )
            .build(db)
    }

    #[test]
    fn template_origins_preserve_django_search_order() {
        let mut db = TestDatabase::new();
        let project = project_with_templates(
            &mut db,
            vec!["/test/project/templates", "/test/project/app/templates"],
            vec![
                (
                    "base.html",
                    "/test/project/templates/base.html",
                    "project base",
                ),
                (
                    "base.html",
                    "/test/project/app/templates/base.html",
                    "app base",
                ),
                (
                    "account/detail.html",
                    "/test/project/app/templates/account/detail.html",
                    "detail",
                ),
            ],
        );

        let names: Vec<_> = template_origins(&db, project)
            .iter(&db)
            .map(|origin| origin.template_name(&db).name(&db).clone())
            .collect();

        assert_eq!(names, ["base.html", "base.html", "account/detail.html"]);
    }

    #[test]
    fn find_template_returns_first_origin_for_duplicate_template_names() {
        let mut db = TestDatabase::new();
        let project = project_with_templates(
            &mut db,
            vec!["/test/project/templates", "/test/project/app/templates"],
            vec![
                (
                    "base.html",
                    "/test/project/templates/base.html",
                    "project base",
                ),
                (
                    "base.html",
                    "/test/project/app/templates/base.html",
                    "app base",
                ),
            ],
        );

        let name = TemplateName::new(&db, "base.html".to_string());
        let result = template_origins(&db, project).find_template(&db, name);
        let FindTemplateResult::Found(origin) = result else {
            panic!("expected base.html to resolve");
        };

        assert_eq!(
            origin.file(&db).path(&db).as_str(),
            "/test/project/templates/base.html"
        );
    }

    #[test]
    fn find_template_reports_tried_sources_for_missing_template() {
        let mut db = TestDatabase::new();
        let project = project_with_templates(
            &mut db,
            vec!["/test/project/templates", "/test/project/app/templates"],
            Vec::new(),
        );

        let name = TemplateName::new(&db, "missing.html".to_string());
        let result = template_origins(&db, project).find_template(&db, name);
        let FindTemplateResult::DoesNotExist(error) = result else {
            panic!("expected missing.html to be missing");
        };
        let tried: Vec<_> = error
            .tried
            .iter()
            .map(|source| source.path.as_str())
            .collect();

        assert_eq!(
            tried,
            [
                "/test/project/templates/missing.html",
                "/test/project/app/templates/missing.html"
            ]
        );
    }

    #[test]
    fn template_references_record_extends_and_include_kinds() {
        let mut db = TestDatabase::new();
        let project = project_with_templates(
            &mut db,
            vec!["/test/project/templates"],
            vec![
                (
                    "child.html",
                    "/test/project/templates/child.html",
                    "{% extends \"base.html\" %}\n{% include \"partial.html\" %}",
                ),
                ("base.html", "/test/project/templates/base.html", "base"),
                (
                    "partial.html",
                    "/test/project/templates/partial.html",
                    "partial",
                ),
            ],
        );

        let base = TemplateName::new(&db, "base.html".to_string());
        let partial = TemplateName::new(&db, "partial.html".to_string());

        let base_refs = references_to_template_name(&db, project, base);
        let partial_refs = references_to_template_name(&db, project, partial);

        assert_eq!(base_refs.len(), 1);
        assert_eq!(base_refs[0].kind(&db), TemplateReferenceKind::Extends);
        assert_eq!(
            base_refs[0].span(&db),
            Span::saturating_from_parts_usize(2, 21)
        );
        assert_eq!(partial_refs.len(), 1);
        assert_eq!(partial_refs[0].kind(&db), TemplateReferenceKind::Include);
    }

    #[test]
    fn template_references_ignore_dynamic_template_names() {
        let mut db = TestDatabase::new();
        let project = project_with_templates(
            &mut db,
            vec!["/test/project/templates"],
            vec![
                (
                    "child.html",
                    "/test/project/templates/child.html",
                    "{% include partial_name %}\n{% include \"partial.html\" %}",
                ),
                (
                    "partial.html",
                    "/test/project/templates/partial.html",
                    "partial",
                ),
            ],
        );

        let partial = TemplateName::new(&db, "partial.html".to_string());
        let references = references_to_template_name(&db, project, partial);

        assert_eq!(references.len(), 1);
        assert_eq!(references[0].kind(&db), TemplateReferenceKind::Include);
    }

    #[test]
    fn template_references_to_template_name_include_all_sources() {
        let mut db = TestDatabase::new();
        let project = project_with_templates(
            &mut db,
            vec!["/test/project/templates"],
            vec![
                (
                    "first.html",
                    "/test/project/templates/first.html",
                    "{% include \"partial.html\" %}",
                ),
                (
                    "second.html",
                    "/test/project/templates/second.html",
                    "{% include \"partial.html\" %}",
                ),
                (
                    "partial.html",
                    "/test/project/templates/partial.html",
                    "partial",
                ),
            ],
        );

        let partial = TemplateName::new(&db, "partial.html".to_string());
        let references = references_to_template_name(&db, project, partial);
        let source_paths: Vec<_> = references
            .iter()
            .map(|reference| reference.source_file(&db).path(&db).as_str())
            .collect();

        assert_eq!(
            source_paths,
            [
                "/test/project/templates/first.html",
                "/test/project/templates/second.html"
            ]
        );
    }
}
