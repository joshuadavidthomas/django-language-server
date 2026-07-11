use std::collections::VecDeque;

use djls_project::Project;
use djls_project::TemplateName;
use djls_project::TemplateOrigin;
use djls_project::TemplateResolution;
use djls_project::resolve_relative_name;
use djls_project::template_resolution;
use djls_source::File;
use djls_source::Span;
use djls_templates::NodeList;
use djls_templates::TagBit;
use djls_templates::TemplateParseResult;
use djls_templates::TemplateString;
use djls_templates::parse_template;
use rustc_hash::FxHashSet;

use crate::db::Db;
use crate::references::TemplateReferenceKind;
use crate::references::references_to_template_name;
use crate::structure::BlockRole;
use crate::structure::RegionId;
use crate::structure::Regions;
use crate::structure::TemplateNode;
use crate::structure::build_template_tree;
use crate::tags::TagRole;
use crate::tags::TagSpec;
use crate::tags::TagSpecs;

#[salsa::tracked]
pub fn template_inheritance(db: &dyn Db, project: Project, file: File) -> TemplateInheritance<'_> {
    let resolution = template_resolution(db, project);
    let mut ancestors = Vec::new();
    let mut visited = FxHashSet::default();
    visited.insert(file);
    let mut current_file = file;
    let mut current_template_name = resolution.primary_template_name(db, file);

    let end = loop {
        let TemplateParseResult::Parsed(nodelist) = parse_template(db, current_file) else {
            // Parser failures leave no observable extends target. Treat the
            // file as a root rather than claiming a resolution failure.
            break ChainEnd::Root;
        };

        let Some(extends) = template_extends_target(db, nodelist) else {
            break ChainEnd::Root;
        };
        let name = match extends {
            ExtendsTarget::Literal { name, .. } => name,
            ExtendsTarget::Dynamic { span } => {
                break ChainEnd::Dynamic { span };
            }
        };
        let current_template_name_text = current_template_name.map(|name| name.name(db).as_str());
        let Some(resolved_name) =
            resolve_relative_name(current_template_name_text, name.as_str(), false)
        else {
            break ChainEnd::Unresolved { name };
        };

        let template_name = TemplateName::new(db, resolved_name.into_owned());
        let candidates = resolution.origins_for_name(db, template_name);
        if candidates.is_empty() {
            break if resolution.known_template_dirs(db).is_some() {
                ChainEnd::Unresolved {
                    name: template_name.name(db).clone(),
                }
            } else {
                ChainEnd::IncompleteDirs
            };
        }

        let Some(origin) = candidates
            .iter()
            .copied()
            .find(|origin| !visited.contains(&origin.file(db)))
        else {
            break ChainEnd::Cycle;
        };

        current_file = origin.file(db);
        current_template_name = Some(origin.template_name(db));
        ancestors.push(origin);
        visited.insert(current_file);
    };

    TemplateInheritance::new(db, ancestors, end)
}

#[salsa::tracked]
fn template_extends_target<'db>(db: &'db dyn Db, nodelist: NodeList<'db>) -> Option<ExtendsTarget> {
    template_symbols(db, nodelist).extends().cloned()
}

#[salsa::tracked]
pub struct TemplateInheritance<'db> {
    #[returns(ref)]
    pub ancestors: Vec<TemplateOrigin<'db>>,
    pub end: ChainEnd,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub enum ChainEnd {
    Root,
    Dynamic { span: Span },
    Unresolved { name: String },
    IncompleteDirs,
    Cycle,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct BlockSite {
    pub file: File,
    pub name_span: Span,
    pub full_span: Span,
}

/// Nearest ancestor definition of `name`.
pub fn parent_block(db: &dyn Db, project: Project, file: File, name: &str) -> Option<BlockSite> {
    template_inheritance(db, project, file)
        .ancestors(db)
        .iter()
        .find_map(|origin| first_block_site(db, origin.file(db), name))
}

/// Block names visible from ancestors, using the nearest definition site per name.
pub fn inherited_blocks(db: &dyn Db, project: Project, file: File) -> Vec<(String, BlockSite)> {
    let mut seen: FxHashSet<&str> = FxHashSet::default();
    let mut inherited = Vec::new();

    for origin in template_inheritance(db, project, file).ancestors(db) {
        let ancestor_file = origin.file(db);
        let TemplateParseResult::Parsed(nodelist) = parse_template(db, ancestor_file) else {
            continue;
        };
        for block in template_symbols(db, nodelist).blocks() {
            if seen.insert(block.name.as_str()) {
                inherited.push((
                    block.name.clone(),
                    BlockSite {
                        file: ancestor_file,
                        name_span: block.name_span,
                        full_span: block.full_span,
                    },
                ));
            }
        }
    }

    inherited
}

/// Descendant templates that define `name`, discovered through reverse extends edges.
///
/// This is a best-effort, name-keyed query: reverse template references are keyed by target
/// template name, not by a resolved origin. If this file is shadowed under a template name, a
/// child extending that name may be returned even when Django would bind it to another origin.
/// Descendants are still gated on their first literal extends target, so later duplicate extends
/// tags do not create inheritance edges. This matches the existing template find-references
/// contract.
pub fn block_overrides(db: &dyn Db, project: Project, file: File, name: &str) -> Vec<BlockSite> {
    let resolution = template_resolution(db, project);
    let mut queue = VecDeque::new();
    let mut queued_names = FxHashSet::default();

    for &template_name in resolution.template_names_for_file(db, file) {
        if queued_names.insert(template_name) {
            queue.push_back(template_name);
        }
    }

    let mut visited_files = FxHashSet::default();
    visited_files.insert(file);
    let mut overrides = Vec::new();

    while let Some(target_name) = queue.pop_front() {
        for reference in references_to_template_name(db, project, target_name) {
            if reference.kind(db) != TemplateReferenceKind::Extends {
                continue;
            }

            let descendant_file = reference.source_file(db);
            if !file_winning_extends_target_is(db, resolution, descendant_file, target_name) {
                continue;
            }

            if !visited_files.insert(descendant_file) {
                continue;
            }

            if let Some(site) = first_block_site(db, descendant_file, name) {
                overrides.push(site);
            }

            for &template_name in resolution.template_names_for_file(db, descendant_file) {
                if queued_names.insert(template_name) {
                    queue.push_back(template_name);
                }
            }
        }
    }

    overrides
}

fn file_winning_extends_target_is<'db>(
    db: &'db dyn Db,
    resolution: TemplateResolution<'db>,
    file: File,
    target_name: TemplateName<'db>,
) -> bool {
    let TemplateParseResult::Parsed(nodelist) = parse_template(db, file) else {
        return false;
    };

    let Some(ExtendsTarget::Literal { name, .. }) = template_symbols(db, nodelist).extends() else {
        return false;
    };
    let current_template_name = resolution
        .primary_template_name(db, file)
        .map(|name| name.name(db).as_str());
    let Some(resolved_name) = resolve_relative_name(current_template_name, name, false) else {
        return false;
    };

    resolved_name.as_ref() == target_name.name(db)
}

fn first_block_site(db: &dyn Db, file: File, name: &str) -> Option<BlockSite> {
    let TemplateParseResult::Parsed(nodelist) = parse_template(db, file) else {
        return None;
    };
    template_symbols(db, nodelist)
        .blocks()
        .iter()
        .find(|block| block.name == name)
        .map(|block| BlockSite {
            file,
            name_span: block.name_span,
            full_span: block.full_span,
        })
}

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
