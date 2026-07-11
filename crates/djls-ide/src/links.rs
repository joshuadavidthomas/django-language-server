use djls_project::FindTemplateResult;
use djls_project::LoadableLibraryLookup;
use djls_project::template_resolution;
use djls_semantic::resolve_reference_name;
use djls_semantic::template_library_references_in_file;
use djls_semantic::template_references_in_file;
use djls_source::File;
use tower_lsp_server::ls_types;

use crate::ext::SpanExt;
use crate::ext::Utf8PathExt;

pub fn document_links(db: &dyn djls_semantic::Db, file: File) -> Vec<ls_types::DocumentLink> {
    let line_index = file.line_index(db);
    let mut links = Vec::new();

    if let Some(project) = db.project() {
        let resolution = template_resolution(db, project);
        links.extend(
            template_references_in_file(db, project, file)
                .as_slice(db)
                .iter()
                .filter_map(|reference| {
                    let template_name = resolve_reference_name(
                        db,
                        resolution,
                        file,
                        reference.target_template_name(),
                        reference.kind(),
                    )?;
                    match resolution.resolve(db, template_name) {
                        FindTemplateResult::Found(origin) => Some(ls_types::DocumentLink {
                            range: reference.span().to_lsp_range(line_index),
                            target: Some(origin.path_buf(db).to_lsp_uri()?),
                            tooltip: None,
                            data: None,
                        }),
                        FindTemplateResult::DoesNotExist(error) => {
                            tracing::debug!(
                                "Skipping unresolved template document link for '{}': {:?}",
                                error.name.name(db),
                                error.tried
                            );
                            None
                        }
                        FindTemplateResult::Inconclusive(search) => {
                            // Document links render persistently in the buffer, so a link that
                            // might target the wrong shadow is worse than no link; only
                            // definitive resolutions are linked.
                            tracing::debug!(
                                "Skipping inconclusive template document link for '{}'",
                                search.name.name(db)
                            );
                            None
                        }
                    }
                }),
        );
    }

    links.extend(
        template_library_references_in_file(db, file)
            .as_slice(db)
            .iter()
            .filter_map(|reference| {
                let LoadableLibraryLookup::Found(library) = db
                    .template_libraries()
                    .loadable_library(reference.load_name())
                else {
                    return None;
                };
                Some(ls_types::DocumentLink {
                    range: reference.span().to_lsp_range(line_index),
                    target: Some(library.file().path(db).to_lsp_uri()?),
                    tooltip: None,
                    data: None,
                })
            }),
    );

    links.sort_by_key(|link| {
        (
            link.range.start.line,
            link.range.start.character,
            link.range.end.line,
            link.range.end.character,
        )
    });

    links
}
