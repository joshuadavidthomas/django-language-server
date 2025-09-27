use salsa::Accumulator;

use crate::analysis::AnalysisBuilder;
use crate::analysis::AnalysisBundle;
use crate::analysis::InternedAnalysisBundle;
use crate::blocks::BlockTree;
use crate::db::Db;
use crate::db::ValidationErrorAccumulator;
use crate::ids::BlockDefinition;
use crate::ids::ElementId;
use crate::ids::SemanticElement;
use crate::ids::TemplateDependency;
use crate::ids::VariableInfo;
use crate::ids::VariableReference;
use crate::semantic::args::validate_block_tags as validate_block_tags_impl;
use crate::semantic::args::validate_non_block_tags as validate_non_block_tags_impl;
use crate::semantic::forest::ForestBuilder;
use crate::semantic::forest::SemanticForest;
use crate::traits::SemanticModel;
use crate::ValidationError;

/// Primary analysis query - single traversal produces all data
#[salsa::tracked]
pub(crate) fn analyze_template(db: &dyn Db, nodelist: djls_templates::NodeList<'_>) -> AnalysisBundle {
    let nodes = nodelist.nodelist(db);
    let specs = db.tag_specs();
    AnalysisBuilder::analyze(nodes, &specs)
}

/// Optimized analysis query with interned strings
pub(crate) fn analyze_template_interned<'db>(
    db: &'db dyn Db,
    nodelist: djls_templates::NodeList<'db>,
) -> InternedAnalysisBundle<'db> {
    let bundle = analyze_template(db, nodelist);
    bundle.with_interning(db)
}

/// Build block tree (extracts from bundle)
#[salsa::tracked]
pub fn build_block_tree<'db>(db: &'db dyn Db, nodelist: djls_templates::NodeList<'db>) -> BlockTree<'db> {
    let bundle = analyze_template(db, nodelist);
    
    // Accumulate construction errors
    for error in bundle.construction_errors {
        ValidationErrorAccumulator(error).accumulate(db);
    }
    
    BlockTree::new(db, bundle.block_tree)
}

/// Build semantic forest (uses bundle data)
#[salsa::tracked]
pub fn build_semantic_forest<'db>(db: &'db dyn Db, nodelist: djls_templates::NodeList<'db>) -> SemanticForest<'db> {
    let bundle = analyze_template(db, nodelist);
    
    // Build forest from pre-computed data (no re-traversal!)
    let mut forest_builder = ForestBuilder::new(bundle.block_tree);
    forest_builder.set_arg_index(bundle.arg_index);
    let forest_inner = forest_builder.construct();
    
    SemanticForest::new(db, forest_inner)
}

/// Compute tag spans
#[salsa::tracked]
pub fn compute_tag_spans(db: &dyn Db, nodelist: djls_templates::NodeList<'_>) -> Vec<djls_source::Span> {
    let forest = build_semantic_forest(db, nodelist);
    forest.compute_tag_spans(db)
}

/// Find element at offset (uses offset index)
#[salsa::tracked]
pub fn find_element_at_offset(
    db: &dyn Db,
    nodelist: djls_templates::NodeList<'_>,
    offset: u32,
) -> SemanticElement {
    let bundle = analyze_template(db, nodelist);
    let element_id = bundle.offset_index.find_at(offset);
    
    match element_id {
        Some(ElementId::Tag(id)) => {
            // Look up tag information from forest
            let forest = build_semantic_forest(db, nodelist);
            if let Some(tag_info) = forest.find_tag_by_id(db, id) {
                SemanticElement::Tag {
                    id,
                    name: tag_info.name.to_string(),
                    span: tag_info.span,
                    arguments: tag_info.arguments.to_vec(),
                }
            } else {
                SemanticElement::None
            }
        }
        Some(ElementId::Variable(var_id)) => {
            // Look up variable from bundle
            if let Some(var_ref) = bundle.variables.get(var_id as usize) {
                SemanticElement::Variable {
                    name: var_ref.name.clone(),
                    span: var_ref.span,
                }
            } else {
                SemanticElement::None
            }
        }
        _ => SemanticElement::None,
    }
}

/// Find enclosing block at offset
#[salsa::tracked]
pub fn find_enclosing_block(
    db: &dyn Db,
    nodelist: djls_templates::NodeList<'_>,
    offset: u32,
) -> Option<crate::blocks::BlockId> {
    let tree = build_block_tree(db, nodelist);
    tree.find_enclosing_block(db, offset)
}

/// Find containing tag
#[salsa::tracked]
pub fn find_containing_tag(
    db: &dyn Db,
    nodelist: djls_templates::NodeList<'_>,
    offset: u32,
) -> Option<crate::ids::TagReference> {
    let forest = build_semantic_forest(db, nodelist);
    forest.find_containing_tag(db, offset)
}

/// Validate template (collects all errors)
#[salsa::tracked]
pub fn validate_template(db: &dyn Db, nodelist: djls_templates::NodeList<'_>) {
    // Trigger validation queries
    let forest = build_semantic_forest(db, nodelist);
    let tag_spans = compute_tag_spans(db, nodelist);
    
    // Validate block tags
    validate_block_tags_impl(db, forest.roots(db));
    
    // Validate non-block tags
    validate_non_block_tags_impl(db, nodelist, &tag_spans);
}

/// Collect all variables in template
#[salsa::tracked]
pub fn collect_variables(db: &dyn Db, nodelist: djls_templates::NodeList<'_>) -> Vec<VariableReference> {
    let bundle = analyze_template(db, nodelist);
    bundle.variables
}

/// Find variable at offset
#[salsa::tracked]
pub fn find_variable_at_offset(
    db: &dyn Db,
    nodelist: djls_templates::NodeList<'_>,
    offset: u32,
) -> Option<VariableInfo> {
    let bundle = analyze_template(db, nodelist);
    
    // Find variable containing this offset
    for var in &bundle.variables {
        if offset >= var.span.start() && offset < var.span.end() {
            return Some(VariableInfo {
                name: var.name.clone(),
                span: var.span,
                definition_span: None, // TODO: implement when we have scoping
            });
        }
    }
    
    None
}

/// Collect block definitions
#[salsa::tracked]
pub fn collect_block_definitions(db: &dyn Db, nodelist: djls_templates::NodeList<'_>) -> Vec<BlockDefinition> {
    let mut blocks = Vec::new();
    
    for node in nodelist.nodelist(db) {
        if let djls_templates::Node::Tag { name, bits, span } = node {
            if name == "block" && !bits.is_empty() {
                blocks.push(BlockDefinition {
                    name: bits[0].clone(),
                    span: *span,
                });
            }
        }
    }
    
    blocks
}

/// Collect template dependencies
#[salsa::tracked]
pub fn collect_template_dependencies(db: &dyn Db, nodelist: djls_templates::NodeList<'_>) -> Vec<TemplateDependency> {
    let mut deps = Vec::new();
    
    for node in nodelist.nodelist(db) {
        if let djls_templates::Node::Tag { name, bits, span } = node {
            if name == "extends" && !bits.is_empty() {
                deps.push(TemplateDependency::Extends {
                    path: bits[0].clone(),
                    span: *span,
                });
            } else if name == "include" && !bits.is_empty() {
                deps.push(TemplateDependency::Include {
                    path: bits[0].clone(),
                    span: *span,
                });
            }
        }
    }
    
    deps
}

/// Get all variables in scope at a given offset
#[salsa::tracked]
pub fn variables_in_scope_at_offset<'db>(
    db: &'db dyn Db,
    nodelist: djls_templates::NodeList<'db>,
    offset: u32,
) -> Vec<VariableInfo> {
    let bundle = analyze_template(db, nodelist);
    
    // TODO: Implement proper scope analysis
    // For now, return all variables in template
    bundle.variables.iter().map(|var| {
        VariableInfo {
            name: var.name.clone(),
            span: var.span,
            definition_span: None,
        }
    }).collect()
}
