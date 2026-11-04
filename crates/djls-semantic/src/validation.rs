mod arguments;
mod expressions;
mod filters;
mod if_expressions;
mod scoping;

use djls_project::TagRole;
use djls_project::TemplateReferenceKind;
use djls_templates::Filter;

use crate::TagSpec;
use crate::db::Db;
use crate::errors::ValidationError;
use crate::expressions::consumed_arguments;
use crate::scoping::ScopedFilterFactResolver;
use crate::scoping::TemplateAnalysisProjection;
use crate::structure::ActiveTemplateNode;
use crate::structure::ActiveTemplateTag;
use crate::structure::ActiveTemplateVariable;
use crate::structure::active_template_nodes;

/// Tracks the validation state for `{% extends %}` positioning rules.
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
            Self::Start | Self::AfterContent => Self::AfterContent,
            Self::AfterExtends => Self::AfterExtends,
        }
    }
}

/// Validator over one converged [`TemplateAnalysisProjection`].
///
/// Tag validation reuses converged occurrence facts. Filter facts are requested lazily while
/// validating variables, over the projection's already-converged tree and load state.
pub(crate) struct TemplateValidator<'db> {
    db: &'db dyn Db,
    projection: TemplateAnalysisProjection<'db>,
    filter_facts: ScopedFilterFactResolver<'db>,
    extends_position: ExtendsPosition,
    errors: Vec<ValidationError>,
}

impl<'db> TemplateValidator<'db> {
    #[must_use]
    pub(crate) fn new(db: &'db dyn Db, projection: TemplateAnalysisProjection<'db>) -> Self {
        Self {
            db,
            projection,
            filter_facts: ScopedFilterFactResolver::new(db, projection),
            extends_position: ExtendsPosition::default(),
            errors: Vec::new(),
        }
    }

    pub(crate) fn validate(mut self) -> Vec<ValidationError> {
        let tree = self.projection.tree(self.db);
        for node in active_template_nodes(tree.regions(self.db), tree.root(self.db)) {
            match node {
                ActiveTemplateNode::Tag(tag) => self.validate_tag(tag),
                ActiveTemplateNode::Variable(variable) => self.validate_variable(variable),
            }
        }
        self.errors
    }

    fn validate_tag(&mut self, tag: ActiveTemplateTag<'_>) {
        let name = tag.tag;
        let bits = tag.bits;
        let span = tag.span;
        // A captured intermediate has no Tag Spec of its own; its opener's facts say how Django
        // consumes its bits.
        let intermediate = tag.opener_name_span.is_some();
        if intermediate {
            self.validate_consumed_arguments(tag);
        }
        let Some(facts) = self.projection.scoped_tag_facts(self.db).for_tag(tag) else {
            return;
        };
        let effective_spec = facts.spec.as_deref();
        let effective_role = effective_spec.and_then(TagSpec::role);

        if matches!(
            effective_role,
            Some(TagRole::TemplateReference(TemplateReferenceKind::Extends))
        ) {
            match self.extends_position {
                ExtendsPosition::Start => {}
                ExtendsPosition::AfterContent => {
                    self.errors.push(ValidationError::ExtendsMustBeFirst {
                        span: tag.full_span,
                    });
                }
                ExtendsPosition::AfterExtends => {
                    self.errors.push(ValidationError::MultipleExtends {
                        span: tag.full_span,
                    });
                }
            }
            self.extends_position = ExtendsPosition::AfterExtends;
        }

        if effective_role != Some(TagRole::TemplateLibraryLoader)
            && !facts.structure_accepts_spelling
        {
            scoping::check_tag_scoping_rule(
                &mut self.errors,
                name,
                span,
                &facts.availability,
                facts.unknown_load_can_shadow,
            );
        }

        if let Some(spec) = effective_spec
            && let Some(rules) = spec.extracted_rules()
        {
            arguments::check_tag_arguments_rule(&mut self.errors, name, bits, span, rules);
        }

        if effective_role == Some(TagRole::TemplateLibraryLoader) {
            scoping::check_load_libraries_rule(self.db, &mut self.errors, &facts.loader_arguments);
        }

        if !intermediate {
            self.validate_consumed_arguments(tag);
        }

        self.extends_position = self.extends_position.record_non_text();
    }

    fn validate_consumed_arguments(&mut self, tag: ActiveTemplateTag<'_>) {
        let consumed = consumed_arguments(self.projection.scoped_tag_facts(self.db), tag);
        expressions::check_consumed_expressions_rule(&mut self.errors, tag.tag, &consumed);
        for condition in &consumed.conditions {
            if_expressions::check_if_expression_rule(
                &mut self.errors,
                tag.tag,
                condition,
                tag.span,
            );
        }
        for filter in consumed.filters() {
            self.validate_filter(tag.span.start(), &filter);
        }
    }

    fn validate_variable(&mut self, variable: ActiveTemplateVariable<'_>) {
        for filter in variable.filters {
            self.validate_filter(variable.span.start(), filter);
        }

        self.extends_position = self.extends_position.record_non_text();
    }

    fn validate_filter(&mut self, occurrence_start: u32, filter: &Filter) {
        let facts = self.filter_facts.resolve(occurrence_start, filter);
        scoping::check_filter_scoping_rule(
            &mut self.errors,
            filter,
            &facts.availability,
            facts.unknown_load_can_shadow,
        );
        if !facts.unknown_load_can_shadow
            && let Some(arity) = facts.arity
        {
            filters::check_filter_arity_rule(&mut self.errors, filter, arity);
        }
    }
}
