use djls_project::TemplateName;
use djls_project::TemplateResolution;
use djls_project::resolve_relative_name;
use djls_semantic::TemplateReferenceKind;
use djls_source::File;

pub(crate) fn resolve_reference_name<'db>(
    db: &'db dyn djls_semantic::Db,
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
