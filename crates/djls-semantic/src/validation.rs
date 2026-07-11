mod arguments;
mod filters;
mod if_expressions;
mod scoping;

use djls_project::TemplateLibraries;

use crate::db::Db;
use crate::filters::FilterAritySpecs;
use crate::scoping::LoadedLibraries;
use crate::scoping::SymbolIndex;
use crate::structure::ActiveTemplateNode;
use crate::structure::ActiveTemplateTag;
use crate::structure::ActiveTemplateVariable;
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

/// Combined validator for active template semantic facts.
///
/// The validator consumes the `TemplateTree`-derived active view rather than the
/// raw parser `NodeList`, so opaque body content cannot affect semantic rules.
pub(crate) struct TemplateValidator<'a> {
    db: &'a dyn Db,
    tag_specs: &'a TagSpecs,
    tag_index: &'a TagIndex,
    symbol_index: &'a SymbolIndex,
    loaded_libraries: &'a LoadedLibraries,
    template_libraries: &'a TemplateLibraries,
    filter_arity_specs: &'a FilterAritySpecs,

    // Tracking state for positional checks (e.g. {% extends %})
    extends_position: ExtendsPosition,
}

impl<'a> TemplateValidator<'a> {
    #[must_use]
    pub(crate) fn new(db: &'a dyn Db, nodelist: djls_templates::NodeList<'_>) -> Self {
        let template_libraries = db.template_libraries();
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
            filter_arity_specs,
            extends_position: ExtendsPosition::default(),
        }
    }

    pub(crate) fn validate(mut self, nodes: &[ActiveTemplateNode<'_>]) {
        for node in nodes {
            match node {
                ActiveTemplateNode::Tag(tag) => self.validate_tag(*tag),
                ActiveTemplateNode::Variable(variable) => self.validate_variable(*variable),
            }
        }
    }

    fn validate_tag(&mut self, tag: ActiveTemplateTag<'_>) {
        let name = tag.tag;
        let bits = tag.bits;
        let span = tag.span;

        // 1. Extends validation
        if name == "extends" {
            use salsa::Accumulator;

            use crate::ValidationError;
            use crate::ValidationErrorAccumulator;

            match self.extends_position {
                ExtendsPosition::Start => {}
                ExtendsPosition::AfterContent => {
                    ValidationErrorAccumulator(ValidationError::ExtendsMustBeFirst {
                        span: tag.full_span,
                    })
                    .accumulate(self.db);
                }
                ExtendsPosition::AfterExtends => {
                    ValidationErrorAccumulator(ValidationError::MultipleExtends {
                        span: tag.full_span,
                    })
                    .accumulate(self.db);
                }
            }

            self.extends_position = ExtendsPosition::AfterExtends;
        }

        // 2. Scoping validation (skip structural tags and "load")
        if name != "load"
            && !matches!(
                self.tag_index.classify(name),
                TagClass::Closer { .. } | TagClass::Intermediate { .. }
            )
        {
            let symbols = self.symbol_index.symbols_at(span.start());
            scoping::check_tag_scoping_rule(self.db, name, span, symbols, self.template_libraries);
        }

        // 3. Argument validation
        if let Some(spec) = self.tag_specs.get(name)
            && let Some(rules) = spec.extracted_rules()
        {
            arguments::check_tag_arguments_rule(self.db, name, bits, span, rules);
        }

        // 4. Load library validation
        scoping::check_load_libraries_rule(self.db, name, bits, self.template_libraries);

        // 5. If expression validation
        if name == "if" || name == "elif" {
            if_expressions::check_if_expression_rule(self.db, name, bits, span);
        }

        self.extends_position = self.extends_position.record_non_text();
    }

    fn validate_variable(&mut self, variable: ActiveTemplateVariable<'_>) {
        if !variable.filters.is_empty() {
            let symbols = self.symbol_index.symbols_at(variable.span.start());

            for filter in variable.filters {
                // 1. Filter Scoping
                scoping::check_filter_scoping_rule(
                    self.db,
                    filter,
                    symbols,
                    self.template_libraries,
                );

                // 2. Filter Arity
                let unknown_load_can_shadow_filter = self
                    .loaded_libraries
                    .has_unknown_load_that_can_shadow_symbol_before(
                        variable.span.start(),
                        &filter.name,
                        self.template_libraries,
                    );
                if !unknown_load_can_shadow_filter {
                    filters::check_filter_arity_rule(self.db, filter, self.filter_arity_specs);
                }
            }
        }

        self.extends_position = self.extends_position.record_non_text();
    }
}
