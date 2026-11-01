use djls_conf::DiagnosticsConfig;
use djls_project::Db as ProjectDb;
use djls_project::ScopedTemplateLibraries;
use djls_project::TemplateLibraryCatalog;
use djls_project::TemplateTagRegex;
use djls_project::scoped_template_libraries;
use djls_project::template_tag_regex;
use djls_source::File;
use djls_templates::Db as TemplatesDb;
use djls_templates::LexerMode;

use crate::errors::ValidationError;
use crate::filters::FilterAritySpecs;
use crate::tags::TagSpecs;

#[salsa::db]
pub trait Db: ProjectDb + TemplatesDb {
    /// Tag specs for analysis without a Project.
    ///
    /// Production returns [`crate::bundled_django_tag_specs`]; fixtures may substitute their own.
    /// Project-backed paths must derive meaning through per-library products.
    fn projectless_tag_specs(&self) -> &TagSpecs;

    /// Get the diagnostics configuration.
    fn diagnostics_config(&self) -> DiagnosticsConfig;

    /// Explicit fixture seam for Filter validation without a Project.
    fn projectless_filter_arity_specs(&self) -> &FilterAritySpecs;
}

/// How to lex the templates of `db`'s Project.
///
/// The `multiline_tags` setting wins because extraction cannot prove either
/// answer: a patch can live in code extraction does not read, and a patch it
/// finds can sit behind a runtime condition. Without a Project or a known
/// extracted fact, tags may span lines. Lexing a single-line project that way
/// only misreads stray openers that a later line closes, while lexing a patched
/// project as single-line would break every multi-line tag into errors.
pub fn project_lexer_mode(db: &dyn ProjectDb) -> LexerMode {
    let Some(project) = db.project() else {
        return LexerMode::MultiLine;
    };
    let multiline =
        project
            .multiline_tags(db)
            .unwrap_or_else(|| match template_tag_regex(db, project) {
                TemplateTagRegex::Default => false,
                TemplateTagRegex::DotAll | TemplateTagRegex::Unknown => true,
            });
    if multiline {
        LexerMode::MultiLine
    } else {
        LexerMode::SingleLine
    }
}

pub fn scoped_template_libraries_for_file(
    db: &dyn Db,
    file: File,
) -> ScopedTemplateLibraries<'_, '_> {
    db.project().map_or_else(
        || ScopedTemplateLibraries::from_project_inventory(TemplateLibraryCatalog::empty_ref()),
        |project| scoped_template_libraries(db, project, file),
    )
}

#[salsa::accumulator]
pub struct ValidationErrorAccumulator(pub ValidationError);
