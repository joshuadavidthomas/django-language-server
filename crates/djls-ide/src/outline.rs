use djls_project::TagRole;
use djls_semantic::BlockRole;
use djls_semantic::Db;
use djls_semantic::LoadKind;
use djls_semantic::RegionId;
use djls_semantic::SourceBits;
use djls_semantic::SourceNode;
use djls_semantic::SourceStructure;
use djls_semantic::template_structure_with_roles;
use djls_source::File;
use djls_source::Span;
use djls_templates::NodeList;
use djls_templates::TemplateString;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct OutlineItem {
    pub label: String,
    pub detail: Option<String>,
    pub kind: OutlineKind,
    pub span: Span,
    pub selection_span: Span,
    pub children: Vec<OutlineItem>,
}

/// Kind of template-domain item represented in the outline.
///
/// The template outline is a navigational projection over template semantics,
/// not the source of truth for every semantic fact in a template.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum OutlineKind {
    TemplateBlock,
    /// A Block Tag without a Tag Role.
    BlockTag,
    TemplateReference,
    TemplateLibrary,
    TemplateLibrarySymbol,
    TemplateTag,
    StaticAssetReference,
    RouteReference,
    Variable,
    Filter,
}

impl From<TagRole> for OutlineKind {
    fn from(role: TagRole) -> Self {
        match role {
            TagRole::TemplateReference(_) | TagRole::TemplatePartialReference => {
                Self::TemplateReference
            }
            TagRole::TemplateLibraryLoader => Self::TemplateLibrary,
            TagRole::TemplateBlock => Self::TemplateBlock,
            TagRole::TemplatePartial => Self::TemplateTag,
            TagRole::StaticAssetReference => Self::StaticAssetReference,
            TagRole::RouteReference => Self::RouteReference,
        }
    }
}

#[salsa::tracked(returns(ref))]
pub(crate) fn build_template_outline_for_file(
    db: &dyn Db,
    file: File,
    nodelist: NodeList<'_>,
) -> Vec<OutlineItem> {
    let tree = template_structure_with_roles(db, file, nodelist);
    outline_items_for_region(&tree, tree.root())
}

fn outline_items_for_tag(
    role: Option<TagRole>,
    tag: &str,
    name_span: Span,
    bits: SourceBits<'_>,
    span: Span,
    children: Vec<OutlineItem>,
) -> Vec<OutlineItem> {
    let Some(role) = role else {
        let mut label = tag.to_string();
        for bit in bits.iter() {
            label.push(' ');
            label.push_str(bit.as_str());
        }

        return vec![OutlineItem {
            label,
            detail: Some(tag.to_string()),
            kind: OutlineKind::BlockTag,
            span,
            selection_span: name_span,
            children,
        }];
    };
    match role {
        TagRole::TemplateReference(_)
        | TagRole::TemplateBlock
        | TagRole::TemplatePartial
        | TagRole::TemplatePartialReference
        | TagRole::StaticAssetReference
        | TagRole::RouteReference => {
            let item = if let Some(bit) = bits.iter().next() {
                let (label, selection_span) = match bit.template_string() {
                    TemplateString::Quoted { value, span } => (value.to_string(), span),
                    TemplateString::Unquoted(value) => (value.to_string(), bit.span),
                };

                OutlineItem {
                    label,
                    detail: Some(tag.to_string()),
                    kind: role.into(),
                    span,
                    selection_span,
                    children,
                }
            } else {
                OutlineItem {
                    label: tag.to_string(),
                    detail: Some(tag.to_string()),
                    kind: role.into(),
                    span,
                    selection_span: name_span,
                    children,
                }
            };
            vec![item]
        }
        TagRole::TemplateLibraryLoader => {
            match LoadKind::from_loader_bits(&bits.iter().collect::<Vec<_>>()) {
                Some(LoadKind::FullLoad { libraries }) => libraries
                    .into_iter()
                    .map(|library| OutlineItem {
                        label: library.as_str().to_string(),
                        detail: Some(tag.to_string()),
                        kind: role.into(),
                        span,
                        selection_span: library.span(),
                        children: Vec::new(),
                    })
                    .collect(),
                Some(LoadKind::SelectiveImport { symbols, library }) => vec![OutlineItem {
                    label: library.as_str().to_string(),
                    detail: Some(tag.to_string()),
                    kind: role.into(),
                    span,
                    selection_span: library.span(),
                    children: symbols
                        .into_iter()
                        .map(|symbol| OutlineItem {
                            label: symbol.as_str().to_string(),
                            detail: Some(format!("from {}", library.as_str())),
                            kind: OutlineKind::TemplateLibrarySymbol,
                            span,
                            selection_span: symbol.span(),
                            children: Vec::new(),
                        })
                        .collect(),
                }],
                None => Vec::new(),
            }
        }
    }
}

fn outline_items_for_region(tree: &SourceStructure<'_>, region: RegionId) -> Vec<OutlineItem> {
    tree.region(region)
        .nodes()
        .flat_map(|node| outline_items_for_node(tree, &node))
        .collect()
}

#[allow(clippy::too_many_lines)]
fn outline_items_for_node(tree: &SourceStructure<'_>, node: &SourceNode<'_>) -> Vec<OutlineItem> {
    match node {
        SourceNode::Block {
            tag,
            name_span,
            bits,
            full_span: _,
            body,
            block_role: BlockRole::Opener,
            tag_role,
        } => {
            let children = tree
                .region(*body)
                .nodes()
                .flat_map(|node| match &node {
                    SourceNode::Block {
                        tag: segment_tag,
                        body: segment_body,
                        block_role: BlockRole::Segment,
                        ..
                    } if segment_tag == tag => outline_items_for_region(tree, *segment_body),
                    SourceNode::Block { .. }
                    | SourceNode::Opaque { .. }
                    | SourceNode::StandaloneTag { .. }
                    | SourceNode::Variable { .. } => outline_items_for_node(tree, &node),
                })
                .collect();

            outline_items_for_tag(
                *tag_role,
                tag,
                *name_span,
                *bits,
                tree.region(*body).span(),
                children,
            )
        }
        SourceNode::Block {
            tag,
            name_span,
            bits,
            full_span,
            body,
            block_role: BlockRole::Segment,
            ..
        } => {
            let children = outline_items_for_region(tree, *body);
            outline_items_for_tag(None, tag, *name_span, *bits, *full_span, children)
        }
        SourceNode::StandaloneTag {
            tag,
            name_span,
            bits,
            full_span,
            tag_role,
        } => tag_role.map_or_else(Vec::new, |role| {
            outline_items_for_tag(Some(role), tag, *name_span, *bits, *full_span, Vec::new())
        }),
        SourceNode::Variable {
            var,
            var_span,
            filters,
            span,
        } => vec![OutlineItem {
            label: var.to_string(),
            detail: None,
            kind: OutlineKind::Variable,
            span: *span,
            selection_span: *var_span,
            children: filters
                .iter()
                .map(|filter| OutlineItem {
                    label: filter.label(),
                    detail: None,
                    kind: OutlineKind::Filter,
                    span: filter.span,
                    selection_span: filter.span.with_length_usize_saturating(filter.name.len()),
                    children: Vec::new(),
                })
                .collect(),
        }],
        SourceNode::Opaque { .. } => Vec::new(),
    }
}
