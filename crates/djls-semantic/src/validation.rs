pub mod arguments;
pub mod extends;
pub mod filters;
pub mod if_expressions;
pub mod scoping;

use std::collections::HashMap;

use djls_source::Span;
use djls_templates::Filter;
use djls_templates::nodelist::Node;
use djls_templates::visitor::walk_nodelist;
use djls_templates::visitor::Visitor;

use crate::db::Db;
use crate::scoping::AvailableSymbols;
use crate::scoping::LoadedLibraries;
use crate::specs::filters::FilterAritySpecs;
use crate::specs::tags::TagSpecs;
use crate::structure::OpaqueRegions;

/// Combined validator that performs a single-pass validation of a template AST.
///
/// This visitor consolidates multiple validation rules (scoping, arity, arguments,
/// structure) into a single walk of the `NodeList`, reducing redundant traversals.
pub struct TemplateValidator<'a> {
    db: &'a dyn Db,
    tag_specs: TagSpecs,
    loaded_libraries: LoadedLibraries,
    template_libraries: djls_project::TemplateLibraries,
    opaque_regions: &'a OpaqueRegions,
    filter_arity_specs: FilterAritySpecs,

    // Environment symbol caches
    env_tags: Option<HashMap<djls_project::TemplateSymbolName, Vec<djls_project::DiscoveredSymbolCandidate>>>,
    env_filters: Option<HashMap<djls_project::TemplateSymbolName, Vec<djls_project::DiscoveredSymbolCandidate>>>,

    // Tracking state for positional checks (e.g. {% extends %})
    contains_nontext: bool,
    seen_extends: bool,
}

impl<'a> TemplateValidator<'a> {
    #[must_use]
    pub fn new(
        db: &'a dyn Db,
        nodelist: djls_templates::NodeList<'_>,
        opaque_regions: &'a OpaqueRegions,
    ) -> Self {
        let template_libraries = db.template_libraries();
        let tag_specs = db.tag_specs();
        let loaded_libraries = crate::scoping::compute_loaded_libraries(db, nodelist);
        let filter_arity_specs = db.filter_arity_specs();

        let env_tags = template_libraries
            .discovered_symbol_candidates_by_name(djls_project::TemplateSymbolKind::Tag);
        let env_filters = template_libraries
            .discovered_symbol_candidates_by_name(djls_project::TemplateSymbolKind::Filter);

        Self {
            db,
            tag_specs,
            loaded_libraries,
            template_libraries,
            opaque_regions,
            filter_arity_specs,
            env_tags,
            env_filters,
            contains_nontext: false,
            seen_extends: false,
        }
    }

    pub fn validate(mut self, nodes: &[Node]) {
        walk_nodelist(&mut self, nodes);
    }
}

impl Visitor for TemplateValidator<'_> {
    fn visit_tag(&mut self, name: &str, bits: &[String], span: Span) {
        let is_opaque = self.opaque_regions.is_opaque(span.start());

        // 1. Extends validation (cares about order/opacity)
        self.check_extends(name, span);

        if !is_opaque {
            // 2. Scoping validation
            self.check_tag_scoping(name, span);

            // 3. Argument validation
            self.check_tag_arguments(name, bits, span);

            // 4. Load library validation
            if name == "load" {
                self.check_load_libraries(bits, span);
            }

            // 5. If expression validation
            if name == "if" || name == "elif" {
                self.check_if_expression(name, bits, span);
            }
        }

        self.contains_nontext = true;
    }

    fn visit_variable(&mut self, _var: &str, filters: &[Filter], span: Span) {
        if !self.opaque_regions.is_opaque(span.start()) {
            let symbols = AvailableSymbols::at_position(
                &self.loaded_libraries,
                &self.template_libraries,
                span.start(),
            );

            for filter in filters {
                // 1. Filter Scoping
                self.check_filter_scoping(filter, &symbols);

                // 2. Filter Arity
                self.check_filter_arity(filter);
            }
        }

        self.contains_nontext = true;
    }

    fn visit_comment(&mut self, _content: &str, _span: Span) {
        // Comments don't count as non-text for {% extends %} check
    }

    fn visit_text(&mut self, _span: Span) {
        // Text doesn't count as non-text for {% extends %} check
    }

    fn visit_error(&mut self, _span: Span, _full_span: Span, _error: &djls_templates::ParseError) {
        // Errors don't count as non-text for {% extends %} check
    }
}

// Private implementation helpers that were previously free functions
impl TemplateValidator<'_> {
    fn check_extends(&mut self, name: &str, span: Span) {
        if name == "extends" {
            extends::check_extends_rule(self.db, span, self.seen_extends, self.contains_nontext);
            self.seen_extends = true;
        }
    }

    fn check_tag_scoping(&mut self, name: &str, span: Span) {
        // Skip structural tags and "load"
        if name == "load" || scoping::is_closer_or_intermediate(name, &self.tag_specs) {
            return;
        }

        let symbols = AvailableSymbols::at_position(
            &self.loaded_libraries,
            &self.template_libraries,
            span.start(),
        );

        scoping::check_tag_scoping_rule(self.db, name, span, &symbols, &self.env_tags);
    }

    fn check_tag_arguments(&mut self, name: &str, bits: &[String], span: Span) {
        if let Some(spec) = self.tag_specs.get(name) {
            if let Some(rules) = &spec.extracted_rules {
                arguments::check_tag_arguments_rule(self.db, name, bits, span, rules);
            }
        }
    }

    fn check_load_libraries(&mut self, bits: &[String], span: Span) {
        scoping::check_load_libraries_rule(self.db, bits, span, &self.template_libraries);
    }

    fn check_if_expression(&mut self, name: &str, bits: &[String], span: Span) {
        if_expressions::check_if_expression_rule(self.db, name, bits, span);
    }

    fn check_filter_scoping(&mut self, filter: &Filter, symbols: &AvailableSymbols) {
        scoping::check_filter_scoping_rule(self.db, filter, symbols, &self.env_filters);
    }

    fn check_filter_arity(&mut self, filter: &Filter) {
        filters::check_filter_arity_rule(self.db, filter, &self.filter_arity_specs, &self.template_libraries);
    }
}
