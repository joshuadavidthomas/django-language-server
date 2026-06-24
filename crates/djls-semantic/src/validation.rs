pub(crate) mod arguments;
pub(crate) mod filters;
pub(crate) mod if_expressions;
pub(crate) mod scoping;

use djls_project::InactiveLibraries;
use djls_project::StaticKnowledge;
use djls_project::TemplateLibraries;
use djls_project::inactive_template_libraries;
use djls_source::Span;
use djls_templates::Filter;
use djls_templates::Node;
use djls_templates::Visitor;
use djls_templates::walk_nodelist;

use crate::db::Db;
use crate::filters::FilterAritySpecs;
use crate::scoping::LoadedLibraries;
use crate::scoping::SymbolIndex;
use crate::structure::OpaqueRegions;
use crate::structure::compute_tag_index;
use crate::structure::grammar::TagClass;
use crate::structure::grammar::TagIndex;
use crate::tags::TagSpecs;

/// Tracks the validation state for `{% extends %}` positioning rules.
///
/// Ensures:
/// 1. `{% extends %}` must be the first non-text node (S122)
/// 2. Only one `{% extends %}` allowed per template (S123)
#[derive(Debug, Clone, Copy, Default)]
enum ExtendsPosition {
    #[default]
    Start,
    AfterContent,
    AfterExtends,
}

impl ExtendsPosition {
    fn record_non_text(self) -> Self {
        match self {
            Self::Start => Self::AfterContent,
            other => other,
        }
    }
}

/// Combined validator that performs a single-pass validation of a template AST.
///
/// This visitor consolidates multiple validation rules (scoping, arity, bits,
/// structure) into a single walk of the `NodeList`, reducing redundant traversals.
pub(crate) struct TemplateValidator<'a> {
    db: &'a dyn Db,
    tag_specs: &'a TagSpecs,
    tag_index: &'a TagIndex,
    symbol_index: &'a SymbolIndex,
    loaded_libraries: &'a LoadedLibraries,
    template_libraries: &'a TemplateLibraries,
    inactive_libraries: &'a InactiveLibraries,
    opaque_regions: &'a OpaqueRegions,
    filter_arity_specs: &'a FilterAritySpecs,

    // Tracking state for positional checks (e.g. {% extends %})
    extends_position: ExtendsPosition,
}

impl<'a> TemplateValidator<'a> {
    #[must_use]
    pub(crate) fn new(
        db: &'a dyn Db,
        nodelist: djls_templates::NodeList<'_>,
        opaque_regions: &'a OpaqueRegions,
    ) -> Self {
        let template_libraries = db.template_libraries();
        let inactive_libraries = db
            .project()
            .map_or(InactiveLibraries::empty_ref(), |project| {
                inactive_template_libraries(db, project)
            });
        let tag_specs = db.tag_specs();
        let tag_index = compute_tag_index(db);
        let loaded_libraries = crate::scoping::compute_loaded_libraries(db, nodelist);
        let symbol_index = crate::scoping::compute_symbol_index(db, nodelist);
        let filter_arity_specs = db.filter_arity_specs();

        Self {
            db,
            tag_specs,
            tag_index,
            symbol_index,
            loaded_libraries,
            template_libraries,
            inactive_libraries,
            opaque_regions,
            filter_arity_specs,
            extends_position: ExtendsPosition::default(),
        }
    }

    pub(crate) fn validate(mut self, nodes: &[Node]) {
        walk_nodelist(&mut self, nodes);
    }
}

impl Visitor for TemplateValidator<'_> {
    fn visit_tag(
        &mut self,
        name: &str,
        _name_span: Span,
        bits: &[djls_templates::TagBit],
        span: Span,
    ) {
        let is_opaque = self.opaque_regions.is_opaque(span.start());

        // 1. Extends validation (cares about order/opacity)
        if name == "extends" {
            use djls_templates::TagDelimiter;
            use salsa::Accumulator;

            use crate::ValidationError;
            use crate::ValidationErrorAccumulator;

            let full_span = span.expand(TagDelimiter::LENGTH_U32, TagDelimiter::LENGTH_U32);

            match self.extends_position {
                ExtendsPosition::Start => {}
                ExtendsPosition::AfterContent => {
                    ValidationErrorAccumulator(ValidationError::ExtendsMustBeFirst {
                        span: full_span,
                    })
                    .accumulate(self.db);
                }
                ExtendsPosition::AfterExtends => {
                    ValidationErrorAccumulator(ValidationError::MultipleExtends {
                        span: full_span,
                    })
                    .accumulate(self.db);
                }
            }

            self.extends_position = ExtendsPosition::AfterExtends;
        }

        if !is_opaque {
            // 2. Scoping validation (skip structural tags and "load")
            if name != "load"
                && !matches!(
                    self.tag_index.classify(name),
                    TagClass::Closer { .. } | TagClass::Intermediate { .. }
                )
            {
                let symbols = self.symbol_index.symbols_at(span.start());
                scoping::check_tag_scoping_rule(
                    self.db,
                    name,
                    span,
                    symbols,
                    self.inactive_libraries,
                    self.template_libraries.knowledge,
                );
            }

            // 3. Argument validation
            if let Some(spec) = self.tag_specs.get(name)
                && let Some(rules) = spec.extracted_rules()
            {
                arguments::check_tag_arguments_rule(self.db, name, bits, span, rules);
            }

            // 4. Load library validation
            scoping::check_load_libraries_rule(
                self.db,
                name,
                bits,
                self.template_libraries,
                self.inactive_libraries,
            );

            // 5. If expression validation
            if name == "if" || name == "elif" {
                if_expressions::check_if_expression_rule(self.db, name, bits, span);
            }
        }

        self.extends_position = self.extends_position.record_non_text();
    }

    fn visit_variable(&mut self, _var: &str, _var_span: Span, filters: &[Filter], span: Span) {
        if !filters.is_empty() && !self.opaque_regions.is_opaque(span.start()) {
            let symbols = self.symbol_index.symbols_at(span.start());

            for filter in filters {
                // 1. Filter Scoping
                scoping::check_filter_scoping_rule(
                    self.db,
                    filter,
                    symbols,
                    self.inactive_libraries,
                    self.template_libraries.knowledge,
                );

                // 2. Filter Arity
                let unknown_load_can_shadow_filter = self.template_libraries.knowledge
                    == StaticKnowledge::Partial
                    && self
                        .loaded_libraries
                        .has_unknown_load_that_can_shadow_symbol_before(
                            span.start(),
                            &filter.name,
                            self.template_libraries,
                        );
                if !unknown_load_can_shadow_filter {
                    filters::check_filter_arity_rule(
                        self.db,
                        filter,
                        self.filter_arity_specs,
                        self.template_libraries.knowledge,
                    );
                }
            }
        }

        self.extends_position = self.extends_position.record_non_text();
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
