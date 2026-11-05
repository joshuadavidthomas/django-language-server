use djls_project::TagRole;
use djls_source::File;
use djls_source::Span;
use djls_templates::Filter;
use djls_templates::NodeList;
use djls_templates::TagBit;

use crate::db::Db;
use crate::layout::SourcePositions;
use crate::layout::template_layout;
use crate::scoping::ScopedTagFacts;
use crate::scoping::analyze_template_layout_in_scope;
use crate::structure::BlockRole;
use crate::structure::RegionId;
use crate::structure::TemplateNode;
use crate::structure::TemplateRegion;
use crate::structure::TemplateTree;
use crate::structure::build_projectless_template_tree;
use crate::tags::BodyAnalysis;
use crate::tags::TagSpec;

/// A borrowed view of template structure. Every exposed span is in source positions.
pub struct SourceStructure<'db> {
    db: &'db dyn Db,
    tree: TemplateTree<'db>,
    positions: Option<&'db SourcePositions>,
    facts: Option<&'db ScopedTagFacts>,
}

/// Read structure without requesting occurrence roles. Without a Project this uses
/// the direct projectless grammar instead of correlated template analysis.
#[must_use]
pub fn template_structure<'db>(
    db: &'db dyn Db,
    file: File,
    nodelist: NodeList<'db>,
) -> SourceStructure<'db> {
    if db.project().is_none() {
        return SourceStructure {
            db,
            tree: build_projectless_template_tree(db, nodelist),
            positions: None,
            facts: None,
        };
    }
    let layout = template_layout(db, nodelist);
    let analysis = analyze_template_layout_in_scope(db, file, layout, file);
    SourceStructure {
        db,
        tree: analysis.tree(db),
        positions: Some(layout.positions(db)),
        facts: None,
    }
}

/// Read structure and occurrence-specific tag roles from the correlated analysis.
#[must_use]
pub fn template_structure_with_roles<'db>(
    db: &'db dyn Db,
    file: File,
    nodelist: NodeList<'db>,
) -> SourceStructure<'db> {
    let layout = template_layout(db, nodelist);
    let analysis = analyze_template_layout_in_scope(db, file, layout, file);
    SourceStructure {
        db,
        tree: analysis.tree(db),
        positions: Some(layout.positions(db)),
        facts: Some(analysis.scoped_tag_facts(db)),
    }
}

impl<'db> SourceStructure<'db> {
    #[must_use]
    pub fn root(&self) -> RegionId {
        self.tree.root(self.db)
    }

    #[must_use]
    pub fn region(&self, id: RegionId) -> SourceRegion<'db> {
        SourceRegion {
            region: self.tree.regions(self.db).get(id),
            positions: self.positions,
            facts: self.facts,
        }
    }
}

#[derive(Clone, Copy)]
pub struct SourceRegion<'db> {
    region: &'db TemplateRegion,
    positions: Option<&'db SourcePositions>,
    facts: Option<&'db ScopedTagFacts>,
}

impl<'db> SourceRegion<'db> {
    #[must_use]
    pub fn span(self) -> Span {
        source_span(self.positions, *self.region.span())
    }

    pub fn nodes(self) -> impl Iterator<Item = SourceNode<'db>> {
        self.region.nodes().iter().filter_map(move |node| {
            let role = |span| {
                self.facts
                    .and_then(|facts| facts.for_name_span(span))
                    .and_then(|fact| fact.spec.as_deref())
                    .and_then(TagSpec::role)
            };
            let bits = |bits| SourceBits {
                bits,
                positions: self.positions,
            };
            match node {
                TemplateNode::Block {
                    tag,
                    name_span,
                    bits: tag_bits,
                    full_span,
                    body,
                    role: block_role,
                } => Some(SourceNode::Block {
                    tag,
                    name_span: source_span(self.positions, *name_span),
                    bits: bits(tag_bits),
                    full_span: source_span(self.positions, *full_span),
                    body: *body,
                    block_role: block_role.clone(),
                    tag_role: role(*name_span),
                }),
                TemplateNode::Opaque {
                    full_span,
                    body_analysis,
                    ..
                } => Some(SourceNode::Opaque {
                    full_span: source_span(self.positions, *full_span),
                    body_analysis: *body_analysis,
                }),
                TemplateNode::StandaloneTag {
                    tag,
                    name_span,
                    bits: tag_bits,
                    full_span,
                } => Some(SourceNode::StandaloneTag {
                    tag,
                    name_span: source_span(self.positions, *name_span),
                    bits: bits(tag_bits),
                    full_span: source_span(self.positions, *full_span),
                    tag_role: role(*name_span),
                }),
                TemplateNode::Variable {
                    var,
                    var_span,
                    filters,
                    span,
                } => Some(SourceNode::Variable {
                    var,
                    var_span: source_span(self.positions, *var_span),
                    filters: SourceFilters {
                        filters,
                        positions: self.positions,
                    },
                    span: source_span(self.positions, *span),
                }),
                TemplateNode::Comment { .. } | TemplateNode::Error { .. } => None,
            }
        })
    }
}

/// Nodes used by structure-based features. Tag and filter arguments are converted
/// on demand, so reading folds does not copy an entire source-position tree.
pub enum SourceNode<'db> {
    Block {
        tag: &'db str,
        name_span: Span,
        bits: SourceBits<'db>,
        full_span: Span,
        body: RegionId,
        block_role: BlockRole,
        tag_role: Option<TagRole>,
    },
    Opaque {
        full_span: Span,
        body_analysis: BodyAnalysis,
    },
    StandaloneTag {
        tag: &'db str,
        name_span: Span,
        bits: SourceBits<'db>,
        full_span: Span,
        tag_role: Option<TagRole>,
    },
    Variable {
        var: &'db str,
        var_span: Span,
        filters: SourceFilters<'db>,
        span: Span,
    },
}

#[derive(Clone, Copy)]
pub struct SourceBits<'db> {
    bits: &'db [TagBit],
    positions: Option<&'db SourcePositions>,
}

impl<'db> SourceBits<'db> {
    pub fn iter(self) -> impl Iterator<Item = TagBit> + 'db {
        self.bits.iter().map(move |bit| {
            let mut bit = bit.clone();
            bit.span = source_span(self.positions, bit.span);
            bit
        })
    }
}

#[derive(Clone, Copy)]
pub struct SourceFilters<'db> {
    filters: &'db [Filter],
    positions: Option<&'db SourcePositions>,
}

impl<'db> SourceFilters<'db> {
    pub fn iter(self) -> impl Iterator<Item = Filter> + 'db {
        self.filters.iter().map(move |filter| {
            let mut filter = filter.clone();
            filter.span = source_span(self.positions, filter.span);
            if let Some(argument) = &mut filter.arg {
                argument.span = source_span(self.positions, argument.span);
            }
            filter
        })
    }
}

fn source_span(positions: Option<&SourcePositions>, span: Span) -> Span {
    positions.map_or(span, |positions| positions.resolve(span))
}
