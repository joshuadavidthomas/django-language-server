use djls_source::Span;
use djls_templates::NodeList;
use djls_templates::TagBit;
use djls_templates::TemplateString;

use crate::db::Db;
use crate::references::TemplateReferenceKind;
use crate::structure::BlockRole;
use crate::structure::RegionId;
use crate::structure::Regions;
use crate::structure::TemplateNode;
use crate::structure::build_template_tree;
use crate::tags::TagRole;
use crate::tags::TagSpec;
use crate::tags::TagSpecs;

#[salsa::tracked(returns(ref))]
pub fn template_symbols<'db>(db: &'db dyn Db, nodelist: NodeList<'db>) -> TemplateSymbols {
    let tree = build_template_tree(db, nodelist);
    let regions = tree.regions(db);
    let mut builder = SymbolBuilder {
        tag_specs: db.tag_specs(),
        regions,
        blocks: Vec::new(),
        partials: Vec::new(),
        extends: None,
    };

    builder.collect_region(tree.root(db));
    builder.finish()
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TemplateSymbols {
    blocks: Vec<BlockDef>,
    partials: Vec<PartialDef>,
    extends: Option<ExtendsTarget>,
}

impl TemplateSymbols {
    #[must_use]
    pub fn blocks(&self) -> &[BlockDef] {
        &self.blocks
    }

    #[must_use]
    pub fn partials(&self) -> &[PartialDef] {
        &self.partials
    }

    #[must_use]
    pub fn extends(&self) -> Option<&ExtendsTarget> {
        self.extends.as_ref()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockDef {
    pub name: String,
    pub name_span: Span,
    pub full_span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PartialDef {
    pub name: String,
    pub name_span: Span,
    pub full_span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExtendsTarget {
    Literal { name: String, span: Span },
    Dynamic { span: Span },
}

struct SymbolBuilder<'a> {
    tag_specs: &'a TagSpecs,
    regions: &'a Regions,
    blocks: Vec<BlockDef>,
    partials: Vec<PartialDef>,
    extends: Option<ExtendsTarget>,
}

impl SymbolBuilder<'_> {
    fn finish(self) -> TemplateSymbols {
        TemplateSymbols {
            blocks: self.blocks,
            partials: self.partials,
            extends: self.extends,
        }
    }

    fn collect_region(&mut self, region: RegionId) {
        for node in self.regions.get(region).nodes() {
            self.collect_node(node);
        }
    }

    fn collect_node(&mut self, node: &TemplateNode) {
        match node {
            TemplateNode::Block {
                tag,
                bits,
                full_span,
                body,
                role: BlockRole::Opener,
                ..
            } => {
                self.collect_definition(tag, bits, *self.regions.get(*body).span());
                self.collect_extends(tag, bits, *full_span);
                self.collect_region(*body);
            }
            TemplateNode::Block {
                body,
                role: BlockRole::Segment,
                ..
            } => {
                self.collect_region(*body);
            }
            TemplateNode::StandaloneTag {
                tag,
                bits,
                full_span,
                ..
            } => {
                self.collect_extends(tag, bits, *full_span);
            }
            TemplateNode::Opaque { .. }
            | TemplateNode::Variable { .. }
            | TemplateNode::Comment { .. }
            | TemplateNode::Text { .. }
            | TemplateNode::Error { .. } => {}
        }
    }

    fn collect_definition(&mut self, tag: &str, bits: &[TagBit], full_span: Span) {
        let Some(role) = self.tag_role(tag) else {
            return;
        };
        let Some(bit) = bits.first() else {
            return;
        };

        match role {
            TagRole::TemplateBlock => self.blocks.push(BlockDef {
                name: bit.as_str().to_string(),
                name_span: bit.span,
                full_span,
            }),
            TagRole::TemplatePartial => self.partials.push(PartialDef {
                name: bit.as_str().to_string(),
                name_span: bit.span,
                full_span,
            }),
            TagRole::TemplateReference(_)
            | TagRole::TemplateLibraryLoader
            | TagRole::ControlTag
            | TagRole::TemplateTag
            | TagRole::StaticAssetReference
            | TagRole::RouteReference => {}
        }
    }

    fn collect_extends(&mut self, tag: &str, bits: &[TagBit], full_span: Span) {
        if self.extends.is_some() {
            return;
        }
        if !matches!(
            self.tag_role(tag),
            Some(TagRole::TemplateReference(TemplateReferenceKind::Extends))
        ) {
            return;
        }

        self.extends = Some(match bits.first() {
            Some(bit) => match bit.template_string() {
                TemplateString::Quoted { value, span } => ExtendsTarget::Literal {
                    name: value.to_string(),
                    span,
                },
                TemplateString::Unquoted(_) => ExtendsTarget::Dynamic { span: bit.span },
            },
            None => ExtendsTarget::Dynamic { span: full_span },
        });
    }

    fn tag_role(&self, tag: &str) -> Option<TagRole> {
        self.tag_specs.get(tag).and_then(TagSpec::role)
    }
}
