use djls_source::Span;
use djls_templates::Node;
use rustc_hash::FxHashMap;
use rustc_hash::FxHashSet;

use crate::blocks::BlockTreeBuilder;
use crate::blocks::BlockTreeInner;
use crate::blocks::TagIndex;
use crate::ids::ElementId;
use crate::ids::VariableReference;
use crate::interned::{ArgumentList, TagName, VariablePath};
use crate::templatetags::TagSpecs;
use crate::traits::SemanticModel;
use crate::ValidationError;

/// Bundle of all analysis data computed in a single pass
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AnalysisBundle {
    /// Block tree structure
    pub block_tree: BlockTreeInner,
    
    /// Argument index: Span -> arguments
    pub arg_index: FxHashMap<Span, Vec<String>>,
    
    /// Offset index for fast lookups
    pub offset_index: OffsetIndex,
    
    /// Validation errors found during construction
    pub construction_errors: Vec<ValidationError>,
    
    /// Variable references
    pub variables: Vec<VariableReference>,
}

impl AnalysisBundle {
    /// Convert this analysis bundle to use interned strings
    /// This method is called after pure analysis to leverage Salsa's string deduplication
    pub fn with_interning<'db>(self, db: &'db dyn crate::Db) -> InternedAnalysisBundle<'db> {
        // Intern all tag names
        let mut interned_tags = FxHashMap::default();
        for (span, args) in &self.arg_index {
            if let Some(first_arg) = args.first() {
                // First argument is typically the tag name
                let tag_name = TagName::new(db, first_arg.clone());
                interned_tags.insert(*span, tag_name);
            }
        }
        
        // Intern argument lists
        let mut interned_args = FxHashMap::default();
        for (span, args) in &self.arg_index {
            let arg_list = ArgumentList::new(db, args.clone());
            interned_args.insert(*span, arg_list);
        }
        
        // Intern variable paths
        let mut interned_vars = Vec::new();
        for var_ref in &self.variables {
            // Split variable path on dots for path segments
            let segments: Vec<String> = var_ref.name.split('.').map(|s| s.to_string()).collect();
            let var_path = VariablePath::new(db, segments);
            interned_vars.push((var_path, var_ref.span));
        }
        
        // Convert HashMaps to Vecs for the tracked struct
        let tags_vec: Vec<(Span, TagName<'db>)> = interned_tags.into_iter().collect();
        let args_vec: Vec<(Span, ArgumentList<'db>)> = interned_args.into_iter().collect();
        
        // Create a BlockTree from BlockTreeInner
        let block_tree = crate::blocks::BlockTree::new(db, self.block_tree);
        
        InternedAnalysisBundle {
            block_tree,
            interned_tags: tags_vec,
            interned_args: args_vec,
            offset_index: self.offset_index,
            construction_errors: self.construction_errors,
            interned_variables: interned_vars,
        }
    }
}

/// Analysis bundle with interned strings for Salsa optimization
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct InternedAnalysisBundle<'db> {
    /// Block tree structure
    pub block_tree: crate::blocks::BlockTree<'db>,

    /// Interned tag names by span
    pub interned_tags: Vec<(Span, TagName<'db>)>,

    /// Interned argument lists by span
    pub interned_args: Vec<(Span, ArgumentList<'db>)>,

    /// Offset index for fast lookups
    pub offset_index: OffsetIndex,

    /// Validation errors found during construction
    pub construction_errors: Vec<ValidationError>,

    /// Variable references with interned paths
    pub interned_variables: Vec<InternedVariableReference<'db>>,
}

pub(crate) type InternedVariableReference<'db> = (VariablePath<'db>, Span);

/// Index for fast offset->element lookups
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub(crate) struct OffsetIndex {
    /// Sorted list of (start, end, element_id) for binary search
    pub elements: Vec<(u32, u32, ElementId)>,
}

impl OffsetIndex {
    /// Find the element at the given offset
    pub fn find_at(&self, offset: u32) -> Option<ElementId> {
        // Binary search for the element containing this offset
        self.elements
            .binary_search_by(|(start, end, _)| {
                if offset < *start {
                    std::cmp::Ordering::Greater
                } else if offset >= *end {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .ok()
            .map(|idx| self.elements[idx].2)
    }
}

/// Collects arguments from tag nodes
struct ArgumentCollector {
    arg_index: FxHashMap<Span, Vec<String>>,
}

impl ArgumentCollector {
    fn new() -> Self {
        Self {
            arg_index: FxHashMap::default(),
        }
    }
    
    fn observe(&mut self, node: &Node) {
        if let Node::Tag { bits, span, .. } = node {
            self.arg_index.insert(*span, bits.clone());
        }
    }
    
    fn finish(self) -> FxHashMap<Span, Vec<String>> {
        self.arg_index
    }
}

/// Builds offset index for fast lookups
struct OffsetIndexBuilder {
    elements: Vec<(u32, u32, ElementId)>,
    next_variable_id: u32,
    next_text_id: u32,
    next_semantic_id: u32,
}

impl OffsetIndexBuilder {
    fn new() -> Self {
        Self {
            elements: Vec::new(),
            next_variable_id: 0,
            next_text_id: 0,
            next_semantic_id: 0,
        }
    }
    
    fn observe(&mut self, node: &Node) {
        match node {
            Node::Tag { name, span, .. } => {
                let id = ElementId::Tag(crate::ids::SemanticId::new(self.next_semantic_id));
                self.next_semantic_id += 1;
                self.elements.push((span.start(), span.end(), id));
            }
            Node::Variable { span, .. } => {
                let id = ElementId::Variable(self.next_variable_id);
                self.next_variable_id += 1;
                self.elements.push((span.start(), span.end(), id));
            }
            Node::Text { span } => {
                let id = ElementId::Text(self.next_text_id);
                self.next_text_id += 1;
                self.elements.push((span.start(), span.end(), id));
            }
            _ => {}
        }
    }
    
    fn finish(mut self) -> OffsetIndex {
        // Sort by start position for binary search
        self.elements.sort_by_key(|(start, _, _)| *start);
        OffsetIndex {
            elements: self.elements,
        }
    }
}

/// Collects variable references
struct VariableCollector {
    variables: Vec<VariableReference>,
    seen: FxHashSet<(String, Span)>,
}

impl VariableCollector {
    fn new() -> Self {
        Self {
            variables: Vec::new(),
            seen: FxHashSet::default(),
        }
    }
    
    fn observe(&mut self, node: &Node) {
        if let Node::Variable { var, span, .. } = node {
            let key = (var.clone(), *span);
            if self.seen.insert(key) {
                self.variables.push(VariableReference {
                    name: var.clone(),
                    span: *span,
                });
            }
        }
    }
    
    fn finish(self) -> Vec<VariableReference> {
        self.variables
    }
}

/// Single-pass builder that produces all analysis data
pub(crate) struct AnalysisBuilder {
    block_builder: BlockTreeBuilder,
    arg_collector: ArgumentCollector,
    offset_builder: OffsetIndexBuilder,
    variable_collector: VariableCollector,
}

impl AnalysisBuilder {
    pub fn analyze(nodes: &[Node], specs: &TagSpecs) -> AnalysisBundle {
        let index = TagIndex::from_specs(specs);
        let mut builder = Self::new(index);
        
        for node in nodes {
            builder.visit(node);
        }
        
        builder.finish()
    }
    
    fn new(index: TagIndex) -> Self {
        Self {
            block_builder: BlockTreeBuilder::new(index),
            arg_collector: ArgumentCollector::new(),
            offset_builder: OffsetIndexBuilder::new(),
            variable_collector: VariableCollector::new(),
        }
    }
    
    fn visit(&mut self, node: &Node) {
        // Single traversal updates all collectors
        self.block_builder.observe(node.clone());
        self.arg_collector.observe(node);
        self.offset_builder.observe(node);
        self.variable_collector.observe(node);
    }
    
    fn finish(self) -> AnalysisBundle {
        let (block_tree, errors) = self.block_builder.construct();
        
        AnalysisBundle {
            block_tree,
            arg_index: self.arg_collector.finish(),
            offset_index: self.offset_builder.finish(),
            construction_errors: errors,
            variables: self.variable_collector.finish(),
        }
    }
}
