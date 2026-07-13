mod arguments;
mod filters;
mod if_expressions;
mod scoping;

use djls_project::TemplateEnvironment;

use crate::db::Db;
use crate::scoping::LoadedLibraries;
use crate::scoping::SymbolIndex;
use crate::structure::ActiveTemplateNode;
use crate::structure::ActiveTemplateTag;
use crate::structure::ActiveTemplateVariable;
use crate::structure::compute_tag_index_for_file;
use crate::structure::grammar::ScopedTagIndex;
use crate::structure::grammar::TagClass;
use crate::tags::TagRole;

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
    file: djls_source::File,
    tag_index: &'a ScopedTagIndex,
    symbol_index: SymbolIndex,
    loaded_libraries: &'a LoadedLibraries,
    environment: TemplateEnvironment<'a>,

    // Tracking state for positional checks (e.g. {% extends %})
    extends_position: ExtendsPosition,
}

impl<'a> TemplateValidator<'a> {
    #[must_use]
    pub(crate) fn new(
        db: &'a dyn Db,
        file: djls_source::File,
        nodelist: djls_templates::NodeList<'_>,
        environment: TemplateEnvironment<'a>,
    ) -> Self {
        let tag_index = compute_tag_index_for_file(db, file, nodelist);
        let loaded_libraries =
            crate::scoping::compute_loaded_libraries_for_file(db, file, nodelist);
        let symbol_index = SymbolIndex::build_environment(db, loaded_libraries, environment);

        Self {
            db,
            file,
            tag_index,
            symbol_index,
            loaded_libraries,
            environment,
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

        let effective_spec = self.tag_index.at(span.start()).spec(name);
        let effective_role = effective_spec.and_then(crate::TagSpec::role);

        // 1. Extends validation
        if matches!(
            effective_role,
            Some(TagRole::TemplateReference(
                crate::references::TemplateReferenceKind::Extends
            ))
        ) {
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

        // 2. Scoping validation (skip structural tags and the effective library loader)
        if effective_role != Some(TagRole::TemplateLibraryLoader)
            && !matches!(
                self.tag_index.at(span.start()).classify(name),
                TagClass::Closer { .. } | TagClass::Intermediate { .. }
            )
        {
            let symbols = self.symbol_index.symbols_at(span.start());
            let unknown_load_can_supply_tag = self
                .loaded_libraries
                .has_unknown_load_that_can_shadow_symbol_before(
                    self.db,
                    span.start(),
                    name,
                    self.environment,
                );
            scoping::check_tag_scoping_rule(
                self.db,
                name,
                span,
                symbols,
                self.environment,
                unknown_load_can_supply_tag,
            );
        }

        // 3. Argument validation
        if let Some(spec) = effective_spec
            && let Some(rules) = spec.extracted_rules()
        {
            arguments::check_tag_arguments_rule(self.db, name, bits, span, rules);
        }

        // 4. Load library validation
        if effective_role == Some(TagRole::TemplateLibraryLoader) {
            scoping::check_load_libraries_rule(self.db, name, bits, self.environment);
        }

        // 5. If expression validation
        if effective_role == Some(TagRole::ControlTag) && (name == "if" || name == "elif") {
            if_expressions::check_if_expression_rule(self.db, name, bits, span);
        }

        self.extends_position = self.extends_position.record_non_text();
    }

    fn validate_variable(&mut self, variable: ActiveTemplateVariable<'_>) {
        if !variable.filters.is_empty() {
            let symbols = self.symbol_index.symbols_at(variable.span.start());

            for filter in variable.filters {
                let unknown_load_can_shadow_filter = self
                    .loaded_libraries
                    .has_unknown_load_that_can_shadow_symbol_before(
                        self.db,
                        variable.span.start(),
                        &filter.name,
                        self.environment,
                    );

                // 1. Filter Scoping
                scoping::check_filter_scoping_rule(
                    self.db,
                    filter,
                    symbols,
                    self.environment,
                    unknown_load_can_shadow_filter,
                );

                // 2. Filter Arity
                if !unknown_load_can_shadow_filter
                    && let Some(arity) = crate::filters::effective_filter_arity(
                        self.db,
                        self.file,
                        &filter.name,
                        &self.loaded_libraries.available_at(variable.span.start()),
                    )
                {
                    filters::check_filter_arity_rule(self.db, filter, &arity);
                }
            }
        }

        self.extends_position = self.extends_position.record_non_text();
    }
}
