use djls_semantic::TemplateFoldKind;
use djls_source::File;
use tower_lsp_server::ls_types;

use crate::ext::SpanExt;

#[must_use]
pub fn collect_folding_ranges(
    db: &dyn djls_semantic::Db,
    file: File,
) -> Vec<ls_types::FoldingRange> {
    let Some(nodelist) = djls_templates::parse_template(db, file) else {
        return Vec::new();
    };

    let line_index = file.line_index(db);
    let mut ranges: Vec<_> = djls_semantic::collect_template_folds(db, nodelist)
        .into_iter()
        .filter_map(|fold| {
            let range = fold.span.to_lsp_range(line_index);

            if range.start.line >= range.end.line {
                return None;
            }

            Some(ls_types::FoldingRange {
                start_line: range.start.line,
                start_character: Some(range.start.character),
                end_line: range.end.line,
                end_character: Some(range.end.character),
                kind: Some(to_lsp_kind(fold.kind)),
                collapsed_text: None,
            })
        })
        .collect();

    ranges.sort_by_key(|range| {
        (
            range.start_line,
            range.start_character,
            range.end_line,
            range.end_character,
        )
    });
    ranges
}

fn to_lsp_kind(kind: TemplateFoldKind) -> ls_types::FoldingRangeKind {
    match kind {
        TemplateFoldKind::Region => ls_types::FoldingRangeKind::Region,
        TemplateFoldKind::Comment => ls_types::FoldingRangeKind::Comment,
    }
}
