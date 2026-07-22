use std::collections::VecDeque;

use djls_project::Project;
use djls_project::ScopedTemplateReferenceResolution;
use djls_project::TemplateBackendScope;
use djls_project::TemplateName;
use djls_project::TemplateOrigin;
use djls_project::TemplateResolution;
use djls_project::TemplateResolutionResult;
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

use crate::TagSpec;
use crate::db::Db;
use crate::references::TemplateReferenceKind;
use crate::scoping::ScopedTagFacts;
use crate::scoping::template_analysis_projection_for_file_in_scope;
use crate::structure::BlockRole;
use crate::structure::RegionId;
use crate::structure::Regions;
use crate::structure::TemplateNode;
use crate::tags::TagRole;

// The loop is one state transition over a correlated parent chain; extracting its few remaining
// lines would split cycle and inconclusive-parent decisions from the state they update.
#[allow(clippy::too_many_lines)]
#[salsa::tracked(returns(copy))]
pub fn template_inheritance(db: &dyn Db, project: Project, file: File) -> TemplateInheritance<'_> {
    let resolution = template_resolution(db, project);
    let mut ancestors = Vec::new();
    let scope_file = file;
    let mut current_file = file;
    let mut current_origins = inheritance_origins(db, resolution, file);
    let mut excluded = current_origins
        .iter()
        .map(|(origin, _)| *origin)
        .collect::<Vec<_>>();

    let end = loop {
        let TemplateParseResult::Parsed(nodelist) = parse_template(db, current_file) else {
            // Parser failures leave no observable extends target. Treat the
            // file as a root rather than claiming a resolution failure.
            break ChainEnd::Root;
        };

        let Some(extends) = template_extends_target(db, current_file, nodelist, scope_file) else {
            break ChainEnd::Root;
        };
        let raw_name = match extends {
            ExtendsTarget::Literal { name, .. } => name,
            ExtendsTarget::Dynamic { span } => {
                break ChainEnd::Dynamic { span };
            }
        };
        let raw_template_name = TemplateName::new(db, raw_name.clone());

        if current_origins.is_empty() {
            // An originless file has no anchor for a relative reference and no backend scope to
            // correlate with. Absolute references still use project-wide resolution, whose
            // result preserves the feasible settings alternatives rather than selecting an
            // arbitrary inventory origin.
            if resolve_relative_name(None, raw_template_name.name(db), false).is_none() {
                break ChainEnd::Unresolved { name: raw_name };
            }
            match resolution.resolve(db, raw_template_name) {
                TemplateResolutionResult::Found(origin) => {
                    current_file = origin.file(db);
                    ancestors.push(origin);
                    current_origins =
                        vec![(origin, resolution.backend_scope_for_origin(db, origin))];
                    if !excluded.contains(&origin) {
                        excluded.push(origin);
                    }
                    continue;
                }
                TemplateResolutionResult::DoesNotExist(_) => {
                    break ChainEnd::Unresolved { name: raw_name };
                }
                TemplateResolutionResult::Inconclusive(_) => {
                    break ChainEnd::InconclusiveParent { name: raw_name };
                }
            }
        }

        let mut scoped = Vec::new();
        let mut invalid_relative = false;
        for (source, scope) in &current_origins {
            let Some(outcome) = resolution.resolve_reference_from_origin_in_scope(
                db,
                *source,
                raw_template_name,
                &excluded,
                false,
                scope,
            ) else {
                invalid_relative = true;
                break;
            };
            scoped.push((outcome, scope.clone()));
        }
        if invalid_relative || scoped.is_empty() {
            break ChainEnd::Unresolved { name: raw_name };
        }

        match join_parent_resolutions(db, &scoped) {
            JoinedParentResolution::Found {
                file: parent_file,
                representative,
                origins: next_origins,
            } => {
                current_file = parent_file;
                ancestors.push(representative);
                for (origin, _) in &next_origins {
                    if !excluded.contains(origin) {
                        excluded.push(*origin);
                    }
                }
                current_origins = next_origins;
            }
            JoinedParentResolution::DoesNotExist => {
                let cycle = scoped.iter().all(|(outcome, scope)| {
                    resolution
                        .resolve_reference_from_origin_in_scope(
                            db,
                            outcome.source,
                            raw_template_name,
                            &[],
                            false,
                            scope,
                        )
                        .is_some_and(|without_exclusions| {
                            matches!(without_exclusions.result, TemplateResolutionResult::Found(origin) if excluded.iter().any(|excluded| excluded.file(db) == origin.file(db)))
                        })
                });
                break if cycle {
                    ChainEnd::Cycle
                } else {
                    ChainEnd::Unresolved { name: raw_name }
                };
            }
            // Different normalized names, backend-local winners, or incomplete searches cannot
            // be collapsed to one safe parent chain.
            JoinedParentResolution::Inconclusive => {
                break ChainEnd::InconclusiveParent { name: raw_name };
            }
        }
    };

    TemplateInheritance::new(db, ancestors, end)
}

enum JoinedParentResolution<'db> {
    Found {
        file: File,
        representative: TemplateOrigin<'db>,
        origins: Vec<(TemplateOrigin<'db>, TemplateBackendScope)>,
    },
    DoesNotExist,
    Inconclusive,
}

fn join_parent_resolutions<'db>(
    db: &'db dyn Db,
    scoped: &[(ScopedTemplateReferenceResolution<'db>, TemplateBackendScope)],
) -> JoinedParentResolution<'db> {
    let mut found = None;
    let mut origins = Vec::new();
    let mut missing = false;

    for (outcome, scope) in scoped {
        match &outcome.result {
            TemplateResolutionResult::Found(origin) => {
                let file = origin.file(db);
                if missing || found.is_some_and(|(found_file, _)| found_file != file) {
                    return JoinedParentResolution::Inconclusive;
                }
                if found.is_none() {
                    found = Some((file, *origin));
                }
                if !origins
                    .iter()
                    .any(|(existing, existing_scope)| existing == origin && existing_scope == scope)
                {
                    origins.push((*origin, scope.clone()));
                }
            }
            TemplateResolutionResult::DoesNotExist(_) => {
                if found.is_some() {
                    return JoinedParentResolution::Inconclusive;
                }
                missing = true;
            }
            TemplateResolutionResult::Inconclusive(_) => {
                return JoinedParentResolution::Inconclusive;
            }
        }
    }

    match found {
        Some((file, representative)) => JoinedParentResolution::Found {
            file,
            representative,
            origins,
        },
        None if missing => JoinedParentResolution::DoesNotExist,
        None => JoinedParentResolution::Inconclusive,
    }
}

fn inheritance_origins<'db>(
    db: &'db dyn Db,
    resolution: TemplateResolution<'db>,
    file: File,
) -> Vec<(TemplateOrigin<'db>, TemplateBackendScope)> {
    resolution
        .template_names_for_file(db, file)
        .iter()
        .flat_map(|name| resolution.origins_for_name(db, *name))
        .filter(|origin| origin.file(db) == file)
        .map(|origin| (*origin, resolution.backend_scope_for_origin(db, *origin)))
        .collect()
}

#[salsa::tracked(returns(clone))]
fn template_extends_target<'db>(
    db: &'db dyn Db,
    file: File,
    nodelist: NodeList<'db>,
    scope_file: File,
) -> Option<ExtendsTarget> {
    template_symbols_in_scope(db, file, nodelist, scope_file)
        .extends()
        .cloned()
}

#[salsa::tracked]
pub struct TemplateInheritance<'db> {
    #[returns(ref)]
    pub ancestors: Vec<TemplateOrigin<'db>>,
    #[returns(clone)]
    pub end: ChainEnd,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ChainEnd {
    Root,
    Dynamic { span: Span },
    Unresolved { name: String },
    InconclusiveParent { name: String },
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
        .find_map(|origin| first_block_site_in_scope(db, origin.file(db), file, name))
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
        for block in template_symbols_in_scope(db, ancestor_file, nodelist, file).blocks() {
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

/// Descendant templates that define `name`, discovered through exact reverse extends edges.
///
/// Each candidate reference is re-resolved from its source origin and backend scope. Traversal
/// therefore follows the concrete origin selected by Django rather than every file sharing a
/// template name.
pub fn block_overrides(db: &dyn Db, project: Project, file: File, name: &str) -> Vec<BlockSite> {
    let resolution = template_resolution(db, project);
    let roots = resolution
        .template_names_for_file(db, file)
        .iter()
        .flat_map(|template_name| resolution.origins_for_name(db, *template_name))
        .filter(|origin| origin.file(db) == file)
        .copied()
        .collect::<Vec<_>>();
    if roots.is_empty() {
        return Vec::new();
    }

    let mut queue = VecDeque::from(roots.clone());
    let mut visited_origins = roots.into_iter().collect::<FxHashSet<_>>();
    let mut emitted_sites = FxHashSet::default();
    let mut overrides = Vec::new();

    while let Some(target) = queue.pop_front() {
        for descendant in resolution.origins(db) {
            if !origin_extends_exact_target(db, resolution, descendant, target) {
                continue;
            }

            if !visited_origins.insert(descendant) {
                continue;
            }

            // A physical descendant can be reached through several origin names. Keep traversing
            // each origin because its relative-name anchor and backend scope differ, but emit its
            // block definition only once.
            if let Some(site) =
                first_block_site_in_scope(db, descendant.file(db), descendant.file(db), name)
                && emitted_sites.insert((site.file, site.name_span, site.full_span))
            {
                overrides.push(site);
            }
            queue.push_back(descendant);
        }
    }

    overrides
}

fn origin_extends_exact_target<'db>(
    db: &'db dyn Db,
    resolution: TemplateResolution<'db>,
    source: TemplateOrigin<'db>,
    target: TemplateOrigin<'db>,
) -> bool {
    let file = source.file(db);
    let TemplateParseResult::Parsed(nodelist) = parse_template(db, file) else {
        return false;
    };
    let Some(ExtendsTarget::Literal { name, .. }) =
        template_symbols_in_scope(db, file, nodelist, file).extends()
    else {
        return false;
    };
    let Some(resolved_name) =
        resolve_relative_name(Some(source.template_name(db).name(db)), name, false)
    else {
        return false;
    };
    let name = TemplateName::new(db, resolved_name.into_owned());
    let scope = resolution.backend_scope_for_origin(db, source);
    matches!(
        resolution.resolve_excluding_origins_in_scope(db, name, &[source], &scope),
        TemplateResolutionResult::Found(origin) if origin == target
    )
}

fn first_block_site_in_scope(
    db: &dyn Db,
    file: File,
    scope_file: File,
    name: &str,
) -> Option<BlockSite> {
    let TemplateParseResult::Parsed(nodelist) = parse_template(db, file) else {
        return None;
    };
    template_symbols_in_scope(db, file, nodelist, scope_file)
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
pub fn template_symbols<'db>(
    db: &'db dyn Db,
    file: File,
    nodelist: NodeList<'db>,
) -> TemplateSymbols {
    template_symbols_in_scope(db, file, nodelist, file).clone()
}

#[salsa::tracked(returns(ref))]
fn template_symbols_in_scope<'db>(
    db: &'db dyn Db,
    file: File,
    nodelist: NodeList<'db>,
    scope_file: File,
) -> TemplateSymbols {
    let projection = template_analysis_projection_for_file_in_scope(db, file, nodelist, scope_file);
    let tree = projection.tree(db);
    let regions = tree.regions(db);
    let mut builder = SymbolBuilder {
        tag_facts: projection.scoped_tag_facts(db),
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
    tag_facts: &'a ScopedTagFacts,
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
                name_span,
                bits,
                full_span,
                body,
                role: BlockRole::Opener,
            } => {
                self.collect_definition(tag, *name_span, bits, *self.regions.get(*body).span());
                self.collect_extends(tag, *name_span, bits, *full_span);
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
                name_span,
                bits,
                full_span,
            } => {
                self.collect_extends(tag, *name_span, bits, *full_span);
            }
            TemplateNode::Opaque { .. }
            | TemplateNode::Variable { .. }
            | TemplateNode::Comment { .. }
            | TemplateNode::Text { .. }
            | TemplateNode::Error { .. } => {}
        }
    }

    fn collect_definition(&mut self, tag: &str, name_span: Span, bits: &[TagBit], full_span: Span) {
        let Some(role) = self.tag_role(tag, name_span) else {
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

    fn collect_extends(&mut self, tag: &str, name_span: Span, bits: &[TagBit], full_span: Span) {
        if self.extends.is_some() {
            return;
        }
        if !matches!(
            self.tag_role(tag, name_span),
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

    fn tag_role(&self, _tag: &str, name_span: Span) -> Option<TagRole> {
        self.tag_facts
            .for_name_span(name_span)
            .and_then(|facts| facts.spec.as_ref())
            .and_then(TagSpec::role)
    }
}
