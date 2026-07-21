mod arguments;
mod filters;
mod if_expressions;
mod scoping;

use crate::TagSpec;
use crate::db::Db;
use crate::references::TemplateReferenceKind;
use crate::scoping::TemplateAnalysisProjection;
use crate::structure::ActiveTemplateNode;
use crate::structure::ActiveTemplateTag;
use crate::structure::ActiveTemplateVariable;
use crate::structure::active_template_nodes;
use crate::tags::TagRole;

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
            Self::Start => Self::AfterContent,
            other => other,
        }
    }
}

/// Validator over one converged [`TemplateAnalysisProjection`].
///
/// Construction performs no grammar, load, symbol, or Filter reconstruction.
pub(crate) struct TemplateValidator<'db> {
    db: &'db dyn Db,
    projection: TemplateAnalysisProjection<'db>,
    extends_position: ExtendsPosition,
}

impl<'db> TemplateValidator<'db> {
    #[must_use]
    pub(crate) fn new(db: &'db dyn Db, projection: TemplateAnalysisProjection<'db>) -> Self {
        Self {
            db,
            projection,
            extends_position: ExtendsPosition::default(),
        }
    }

    pub(crate) fn validate(mut self) {
        let tree = self.projection.tree(self.db);
        let nodes = active_template_nodes(tree.regions(self.db), tree.root(self.db));
        for node in &nodes {
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
        let Some(facts) = self.projection.scoped_tag_facts(self.db).for_tag(tag) else {
            return;
        };
        let effective_spec = facts.spec.as_ref();
        let effective_role = effective_spec.and_then(TagSpec::role);

        if matches!(
            effective_role,
            Some(TagRole::TemplateReference(TemplateReferenceKind::Extends))
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

        if effective_role != Some(TagRole::TemplateLibraryLoader)
            && !facts.structure_accepts_spelling
        {
            scoping::check_tag_scoping_rule(
                self.db,
                name,
                span,
                &facts.availability,
                facts.unknown_load_can_shadow,
            );
        }

        if let Some(spec) = effective_spec
            && let Some(rules) = spec.extracted_rules()
        {
            arguments::check_tag_arguments_rule(self.db, name, bits, span, rules);
        }

        if effective_role == Some(TagRole::TemplateLibraryLoader) {
            scoping::check_load_libraries_rule(self.db, &facts.loader_arguments);
        }

        if effective_role == Some(TagRole::ControlTag) && (name == "if" || name == "elif") {
            if_expressions::check_if_expression_rule(self.db, name, bits, span);
        }

        self.extends_position = self.extends_position.record_non_text();
    }

    fn validate_variable(&mut self, variable: ActiveTemplateVariable<'_>) {
        for filter in variable.filters {
            let Some(facts) = self
                .projection
                .scoped_filter_facts(self.db)
                .for_filter(filter)
            else {
                continue;
            };
            scoping::check_filter_scoping_rule(
                self.db,
                filter,
                &facts.availability,
                facts.unknown_load_can_shadow,
            );
            if !facts.unknown_load_can_shadow
                && let Some(arity) = facts.arity.as_ref()
            {
                filters::check_filter_arity_rule(self.db, filter, arity);
            }
        }

        self.extends_position = self.extends_position.record_non_text();
    }
}
