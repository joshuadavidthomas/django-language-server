use djls_project::FindTemplateResult;
use djls_project::template_resolution;
use djls_semantic::template_references_in_file;
use djls_source::File;
use tower_lsp_server::ls_types;

use crate::ext::SpanExt;
use crate::ext::Utf8PathExt;

pub fn document_links(db: &dyn djls_semantic::Db, file: File) -> Vec<ls_types::DocumentLink> {
    let Some(project) = db.project() else {
        return Vec::new();
    };

    let resolution = template_resolution(db, project);
    let line_index = file.line_index(db);

    template_references_in_file(db, project, file)
        .as_slice(db)
        .iter()
        .filter_map(
            |reference| match resolution.resolve(db, reference.target_template_name) {
                FindTemplateResult::Found(origin) => Some(ls_types::DocumentLink {
                    range: reference.span.to_lsp_range(line_index),
                    target: Some(origin.path_buf(db).to_lsp_uri()?),
                    tooltip: None,
                    data: None,
                }),
                FindTemplateResult::DoesNotExist(error) => {
                    tracing::debug!(
                        "Skipping unresolved template document link for '{}': {:?}",
                        error.template_name.name(db),
                        error.tried
                    );
                    None
                }
            },
        )
        .collect()
}
