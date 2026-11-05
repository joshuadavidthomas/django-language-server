use djls_semantic::BlockRole;
use djls_semantic::BodyAnalysis;
use djls_semantic::RegionId;
use djls_semantic::SourceNode;
use djls_semantic::SourceStructure;
use djls_semantic::template_structure;
use djls_source::File;
use djls_source::Span;
use djls_templates::Node;
use djls_templates::NodeList;
use tower_lsp_server::ls_types;

use crate::ext::FoldSpanExt;
use crate::imports;

#[must_use]
pub fn collect_folding_ranges(
    db: &dyn djls_semantic::Db,
    file: File,
) -> Vec<ls_types::FoldingRange> {
    let djls_templates::TemplateParseResult::Parsed(nodelist) =
        djls_templates::parse_template(db, file)
    else {
        return Vec::new();
    };

    let Ok(source) = file.try_source(db) else {
        return Vec::new();
    };
    let structure_folds = build_template_folds_for_file(db, file, nodelist);
    let role_at = |node: &Node| imports::tag_role_at(db, file, nodelist, node);
    let folds = FoldSpans::collect(
        structure_folds,
        nodelist.nodelist(db),
        source.as_str(),
        &role_at,
    )
    .into_vec();

    let line_index = file.line_index(db);
    folds
        .into_iter()
        .filter_map(|fold| fold.to_lsp_folding_range(line_index))
        .collect()
}

#[salsa::tracked(returns(ref))]
fn build_template_folds_for_file(
    db: &dyn djls_semantic::Db,
    file: File,
    nodelist: NodeList<'_>,
) -> Vec<FoldSpan> {
    let tree = template_structure(db, file, nodelist);
    let mut folds = Vec::new();
    collect_folds_for_region(&tree, tree.root(), &mut folds);
    folds
}

fn collect_folds_for_region(
    tree: &SourceStructure<'_>,
    region: RegionId,
    folds: &mut Vec<FoldSpan>,
) {
    for node in tree.region(region).nodes() {
        match node {
            SourceNode::Block {
                full_span,
                body,
                block_role: BlockRole::Opener,
                ..
            } => {
                let end = tree.region(body).span().end();
                if end > full_span.end() {
                    folds.push(FoldSpan {
                        span: Span::saturating_from_bounds_usize(
                            full_span.start_usize(),
                            end as usize,
                        ),
                        kind: FoldKind::Region,
                    });
                }
                collect_folds_for_region(tree, body, folds);
            }
            SourceNode::Block {
                body,
                block_role: BlockRole::Segment,
                ..
            } => collect_folds_for_region(tree, body, folds),
            SourceNode::Opaque {
                full_span,
                body_analysis,
            } => folds.push(FoldSpan {
                span: full_span,
                kind: match body_analysis {
                    BodyAnalysis::Opaque => FoldKind::Comment,
                    BodyAnalysis::Analyze | BodyAnalysis::Verbatim => FoldKind::Region,
                },
            }),
            SourceNode::StandaloneTag { .. } | SourceNode::Variable { .. } => {}
        }
    }
}

#[derive(Default)]
struct FoldSpans(Vec<FoldSpan>);

impl FoldSpans {
    fn collect(
        structure_folds: &[FoldSpan],
        nodes: &[Node],
        source: &str,
        role_at: imports::TagRoleAt,
    ) -> Self {
        let mut folds = Self::default();
        for fold in structure_folds {
            folds.push(*fold);
        }

        for node in nodes {
            if let Node::Comment { .. } = node {
                folds.push(FoldSpan::comment(node.full_span()));
            }
        }
        for span in imports::fold_spans(nodes, source, role_at) {
            folds.push_imports(FoldSpan::imports(span.start(), span.end()));
        }

        folds
    }

    fn push(&mut self, fold: FoldSpan) {
        self.0.push(fold);
    }

    fn push_imports(&mut self, fold: Option<FoldSpan>) {
        if let Some(fold) = fold {
            self.push(fold);
        }
    }

    fn into_vec(mut self) -> Vec<FoldSpan> {
        self.0.sort_by_key(|fold| fold.sort_key());
        self.0.dedup();
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct FoldSpan {
    pub(crate) span: Span,
    pub(crate) kind: FoldKind,
}

impl FoldSpan {
    fn from_bounds(start: u32, end: u32, kind: FoldKind) -> Option<Self> {
        if start >= end {
            return None;
        }

        Some(Self {
            span: Span::saturating_from_bounds_usize(start as usize, end as usize),
            kind,
        })
    }

    fn comment(span: Span) -> Self {
        Self {
            span,
            kind: FoldKind::Comment,
        }
    }

    fn imports(start: u32, end: u32) -> Option<Self> {
        Self::from_bounds(start, end, FoldKind::Imports)
    }

    fn sort_key(self) -> (u32, u32, u8) {
        (self.span.start(), self.span.end(), self.kind.sort_key())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum FoldKind {
    Region,
    Comment,
    Imports,
}

impl FoldKind {
    fn sort_key(self) -> u8 {
        match self {
            Self::Region => 0,
            Self::Comment => 1,
            Self::Imports => 2,
        }
    }
}

#[cfg(test)]
mod tests {
    use camino::Utf8Path;
    use djls_semantic::EndTag;
    use djls_semantic::IntermediateTag;
    use djls_semantic::TagSpec;
    use djls_semantic::bundled_django_tag_specs;
    use djls_templates::parse_template;
    use djls_testing::ProjectFixture;
    use djls_testing::ProjectSettings;
    use djls_testing::SalsaEventLog;
    use djls_testing::TestDatabase;

    use super::*;

    #[test]
    fn opaque_opener_treats_intermediate_as_raw_content() {
        let mut specs = bundled_django_tag_specs().clone();
        specs.insert(
            "opaque_if".to_string(),
            TagSpec::new(
                "test".into(),
                Some(EndTag {
                    name: "endopaque_if".into(),
                    required: true,
                }),
                vec![IntermediateTag {
                    name: "opaque_else".into(),
                }]
                .into(),
                BodyAnalysis::Opaque,
            ),
        );
        let db = TestDatabase::new().with_projectless_tag_specs(specs);
        let source = "{% opaque_if %}first{% opaque_else %}second{% endopaque_if %}";
        db.add_file("test.html", source)
            .expect("fixture should be added");
        let file = db
            .file(Utf8Path::new("test.html"))
            .expect("fixture should exist");
        let nodelist = parse_template(&db, file).expect("fixture should parse");
        assert_eq!(
            build_template_folds_for_file(&db, file, nodelist),
            &[FoldSpan {
                span: Span::saturating_from_bounds_usize(0, 61),
                kind: FoldKind::Comment,
            }]
        );
    }

    #[test]
    #[expect(clippy::too_many_lines, reason = "compare both query routing modes")]
    fn structure_queries_use_correlated_analysis_only_with_a_project() {
        let event_log = SalsaEventLog::default();
        let db = TestDatabase::with_event_log(event_log.clone());
        db.add_file("/tree.html", "{% if value %}body{% endif %}")
            .expect("fixture file should be added to the test database");
        db.add_file(
            "/opaque.html",
            "{% verbatim %}{% if ignored %}{% endverbatim %}",
        )
        .expect("opaque template fixture should be added to the test database");
        let tree_file = db
            .file(Utf8Path::new("/tree.html"))
            .expect("fixture file should exist in the test database");
        let opaque_file = db
            .file(Utf8Path::new("/opaque.html"))
            .expect("fixture file should exist in the test database");
        let tree_nodes = parse_template(&db, tree_file).expect("tree fixture should parse");
        let opaque_nodes = parse_template(&db, opaque_file).expect("opaque fixture should parse");

        event_log
            .take()
            .expect("Salsa event log should be readable before the projectless query");
        let folds = build_template_folds_for_file(&db, tree_file, tree_nodes);
        assert_eq!(folds.len(), 1);
        let executed = event_log
            .take_will_execute_names(&db)
            .expect("projectless folding query events should be readable");
        assert!(
            executed
                .iter()
                .any(|name| name.ends_with("build_projectless_template_tree")),
            "projectless folding should build the tree directly: {executed:?}"
        );
        assert!(
            executed
                .iter()
                .all(|name| !name.ends_with("analyze_template_layout_in_scope")),
            "projectless folding must bypass correlated analysis: {executed:?}"
        );

        let folds = build_template_folds_for_file(&db, opaque_file, opaque_nodes);
        assert_eq!(
            folds,
            &[FoldSpan {
                span: Span::saturating_from_bounds_usize(0, 47),
                kind: FoldKind::Region,
            }]
        );
        let executed = event_log
            .take_will_execute_names(&db)
            .expect("projectless opaque folding events should be readable");
        assert!(
            executed
                .iter()
                .any(|name| name.ends_with("build_projectless_template_tree")),
            "projectless opaque folding should build one direct tree: {executed:?}"
        );
        assert!(
            executed
                .iter()
                .all(|name| !name.ends_with("analyze_template_layout_in_scope")),
            "projectless opaque folding must bypass correlated analysis: {executed:?}"
        );
        let event_log = SalsaEventLog::default();
        let mut db = TestDatabase::with_event_log(event_log.clone());
        ProjectFixture::new("/project")
            .settings(&ProjectSettings {
                dirs: vec!["/project/templates".to_string()],
                ..ProjectSettings::default()
            })
            .file(
                "/project/templates/tree.html",
                "{% if value %}body{% endif %}",
            )
            .file(
                "/project/templates/opaque.html",
                "{% verbatim %}{% if ignored %}{% endverbatim %}",
            )
            .install(&mut db)
            .expect("project fixture should install into the test database");
        let tree_file = db
            .file(Utf8Path::new("/project/templates/tree.html"))
            .expect("fixture file should exist in the test database");
        let opaque_file = db
            .file(Utf8Path::new("/project/templates/opaque.html"))
            .expect("fixture file should exist in the test database");
        let tree_nodes = parse_template(&db, tree_file).expect("tree fixture should parse");
        let opaque_nodes = parse_template(&db, opaque_file).expect("opaque fixture should parse");

        event_log
            .take()
            .expect("Salsa event log should be readable before the project-backed query");
        let folds = build_template_folds_for_file(&db, tree_file, tree_nodes);
        assert_eq!(folds.len(), 1);
        let executed = event_log
            .take_will_execute_names(&db)
            .expect("project-backed folding query events should be readable");
        assert!(
            executed
                .iter()
                .any(|name| name.ends_with("analyze_template_layout_in_scope")),
            "project-backed folding must use correlated analysis: {executed:?}"
        );

        let folds = build_template_folds_for_file(&db, opaque_file, opaque_nodes);
        assert_eq!(
            folds,
            &[FoldSpan {
                span: Span::saturating_from_bounds_usize(0, 47),
                kind: FoldKind::Region,
            }]
        );
        let executed = event_log
            .take_will_execute_names(&db)
            .expect("project-backed opaque folding events should be readable");
        assert!(
            executed
                .iter()
                .any(|name| name.ends_with("analyze_template_layout_in_scope")),
            "project-backed opaque folding must use correlated analysis: {executed:?}"
        );
    }
}
