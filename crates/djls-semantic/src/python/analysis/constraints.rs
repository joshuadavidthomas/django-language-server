use crate::python::types::ArgumentCountConstraint;
use crate::python::types::ChoiceAt;
use crate::python::types::RequiredKeyword;

/// Constraints inferred from a guard condition.
///
/// This type models condition semantics only. Exception messages are attached
/// by guard extraction, after the raising guard has been evaluated.
///
/// Provides algebraic `or()` and `and()` methods that encode boolean
/// composition semantics from if/raise guard analysis:
/// - `or`: error when either side is true → each constraint is independent
/// - `and`: both must be true for error → length constraints dropped, keywords/choices kept
#[derive(Debug, Clone, Default)]
#[allow(clippy::struct_field_names)]
pub struct ConstraintSet {
    pub arg_constraints: Vec<ArgumentCountConstraint>,
    pub required_keywords: Vec<RequiredKeyword>,
    pub choice_at_constraints: Vec<ChoiceAt>,
}

impl ConstraintSet {
    pub fn single_length(c: ArgumentCountConstraint) -> Self {
        Self {
            arg_constraints: vec![c],
            ..Default::default()
        }
    }

    pub fn single_keyword(k: RequiredKeyword) -> Self {
        Self {
            required_keywords: vec![k],
            ..Default::default()
        }
    }

    pub fn single_choice(c: ChoiceAt) -> Self {
        Self {
            choice_at_constraints: vec![c],
            ..Default::default()
        }
    }

    /// Disjunction: error when either side is true → each is independent.
    pub fn or(mut self, other: Self) -> Self {
        self.arg_constraints.extend(other.arg_constraints);
        self.required_keywords.extend(other.required_keywords);
        self.choice_at_constraints
            .extend(other.choice_at_constraints);
        self
    }

    /// Conjunction: both must be true for error → drop length constraints,
    /// keep keyword/choice constraints.
    pub fn and(self, other: Self) -> Self {
        let mut required_keywords = self.required_keywords;
        required_keywords.extend(other.required_keywords);
        let mut choice_at_constraints = self.choice_at_constraints;
        choice_at_constraints.extend(other.choice_at_constraints);
        Self {
            arg_constraints: Vec::new(),
            required_keywords,
            choice_at_constraints,
        }
    }

    pub fn extend(&mut self, other: Self) {
        self.arg_constraints.extend(other.arg_constraints);
        self.required_keywords.extend(other.required_keywords);
        self.choice_at_constraints
            .extend(other.choice_at_constraints);
    }
}
