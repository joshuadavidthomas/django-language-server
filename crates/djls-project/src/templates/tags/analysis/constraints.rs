use crate::templates::tags::types::ArgumentCountConstraint;
use crate::templates::tags::types::ChoiceAt;
use crate::templates::tags::types::RequiredKeyword;

/// Constraints on a template tag call inferred from Python parser code.
///
/// This is the shared constraint IR accumulated during analysis before final
/// `TagRule` assembly. Guard extraction uses the `or()` and `and()` methods to
/// encode boolean condition semantics. `or` combines independent rejection
/// reasons. The accepted side of `and` is a disjunction, so `and` retains only
/// proofs present on both alternatives.
#[derive(Debug, Clone, Default, PartialEq)]
#[allow(clippy::struct_field_names)]
pub(crate) struct ExtractedTagConstraints {
    pub arg_constraints: Vec<ArgumentCountConstraint>,
    pub required_keywords: Vec<RequiredKeyword>,
    pub choice_at_constraints: Vec<ChoiceAt>,
}

impl ExtractedTagConstraints {
    pub(crate) fn single_length(c: ArgumentCountConstraint) -> Self {
        Self {
            arg_constraints: vec![c],
            ..Default::default()
        }
    }

    pub(crate) fn single_keyword(k: RequiredKeyword) -> Self {
        Self {
            required_keywords: vec![k],
            ..Default::default()
        }
    }

    pub(crate) fn single_choice(c: ChoiceAt) -> Self {
        Self {
            choice_at_constraints: vec![c],
            ..Default::default()
        }
    }

    /// Disjunction: error when either side is true → each is independent.
    pub(crate) fn or(mut self, other: Self) -> Self {
        self.arg_constraints.extend(other.arg_constraints);
        self.required_keywords.extend(other.required_keywords);
        self.choice_at_constraints
            .extend(other.choice_at_constraints);
        self
    }

    /// Conjunction: acceptance means either operand is false. Keep only facts
    /// proved by both accepted alternatives; the flat IR cannot encode their
    /// correlation.
    pub(crate) fn and(self, other: &Self) -> Self {
        Self {
            arg_constraints: self
                .arg_constraints
                .into_iter()
                .filter(|constraint| other.arg_constraints.contains(constraint))
                .collect(),
            required_keywords: self
                .required_keywords
                .into_iter()
                .filter(|keyword| other.required_keywords.contains(keyword))
                .collect(),
            choice_at_constraints: self
                .choice_at_constraints
                .into_iter()
                .filter(|choice| other.choice_at_constraints.contains(choice))
                .collect(),
        }
    }

    pub(crate) fn extend(&mut self, other: Self) {
        for constraint in other.arg_constraints {
            if !self.arg_constraints.contains(&constraint) {
                self.arg_constraints.push(constraint);
            }
        }
        for keyword in other.required_keywords {
            if !self.required_keywords.contains(&keyword) {
                self.required_keywords.push(keyword);
            }
        }
        for choice in other.choice_at_constraints {
            if !self.choice_at_constraints.contains(&choice) {
                self.choice_at_constraints.push(choice);
            }
        }
    }
}
