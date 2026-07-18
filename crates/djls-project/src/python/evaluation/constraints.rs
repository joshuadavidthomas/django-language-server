use std::cmp::Ordering;

use djls_source::Origin;

use super::StructuralOrd;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BranchConstraints {
    // Disjunction of conjunctions. Each conjunction records one arm selected at a control-flow
    // join. Origins identify joins without retaining AST nodes across query boundaries.
    alternatives: Vec<Vec<(Origin, usize)>>,
}

impl BranchConstraints {
    pub(crate) fn unconstrained() -> Self {
        Self {
            alternatives: vec![Vec::new()],
        }
    }

    pub(super) fn select(&mut self, join: Origin, arm: usize) {
        for alternative in &mut self.alternatives {
            alternative.retain(|(existing, _)| *existing != join);
            alternative.push((join, arm));
            alternative.sort_by(cmp_branch_choice);
        }
        self.normalize();
    }

    pub(crate) fn merge(&mut self, other: Self) {
        self.alternatives.extend(other.alternatives);
        self.normalize();
    }

    fn normalize(&mut self) {
        for choices in &mut self.alternatives {
            choices.sort_by(cmp_branch_choice);
        }
        self.alternatives
            .sort_by(|left, right| cmp_conjunction(left, right));
        self.alternatives.dedup();
    }

    pub(crate) fn is_impossible(&self) -> bool {
        self.alternatives.is_empty()
    }

    pub(crate) fn intersection(&self, other: &Self) -> Self {
        let mut alternatives = Vec::new();
        for left in &self.alternatives {
            for right in &other.alternatives {
                let mut choices = left.clone();
                let mut compatible = true;
                for &(join, arm) in right {
                    if let Some((_, existing_arm)) = choices
                        .iter()
                        .find(|(existing_join, _)| *existing_join == join)
                    {
                        if *existing_arm != arm {
                            compatible = false;
                            break;
                        }
                    } else {
                        choices.push((join, arm));
                    }
                }
                if compatible {
                    choices.sort_by(cmp_branch_choice);
                    alternatives.push(choices);
                }
            }
        }
        let mut constraints = Self { alternatives };
        constraints.normalize();
        constraints
    }

    pub(crate) fn compatible_with(&self, other: &Self) -> bool {
        self.alternatives.iter().any(|left| {
            other
                .alternatives
                .iter()
                .any(|right| conjunctions_are_compatible(left, right))
        })
    }
}

impl StructuralOrd for BranchConstraints {
    fn structural_cmp(&self, other: &Self) -> Ordering {
        for (left, right) in self.alternatives.iter().zip(&other.alternatives) {
            let ordering = cmp_conjunction(left, right);
            if ordering != Ordering::Equal {
                return ordering;
            }
        }
        self.alternatives.len().cmp(&other.alternatives.len())
    }
}

fn conjunctions_are_compatible(left: &[(Origin, usize)], right: &[(Origin, usize)]) -> bool {
    let mut left_index = 0;
    let mut right_index = 0;
    while let (Some((left_join, left_arm)), Some((right_join, right_arm))) =
        (left.get(left_index), right.get(right_index))
    {
        match left_join.structural_cmp(right_join) {
            Ordering::Less => left_index += 1,
            Ordering::Greater => right_index += 1,
            Ordering::Equal if left_arm != right_arm => return false,
            Ordering::Equal => {
                left_index += 1;
                right_index += 1;
            }
        }
    }
    true
}

fn cmp_branch_choice(left: &(Origin, usize), right: &(Origin, usize)) -> Ordering {
    left.0
        .structural_cmp(&right.0)
        .then_with(|| left.1.cmp(&right.1))
}

fn cmp_conjunction(left: &[(Origin, usize)], right: &[(Origin, usize)]) -> Ordering {
    for (left, right) in left.iter().zip(right) {
        let ordering = cmp_branch_choice(left, right);
        if ordering != Ordering::Equal {
            return ordering;
        }
    }
    left.len().cmp(&right.len())
}

#[cfg(test)]
mod tests {
    use std::cmp::Ordering;

    use djls_source::File;
    use djls_source::Span;
    use salsa::plumbing::FromId as _;

    use super::BranchConstraints;
    use super::Origin;
    use super::StructuralOrd;

    fn origin(file_index: u32, start: u32) -> Origin {
        // SAFETY: Test indexes are below `salsa::Id::MAX_U32`; these synthetic
        // files are compared only as opaque IDs and are never read.
        let file = File::from_id(unsafe { salsa::Id::from_index(file_index) });
        Origin::new(file, Span::new(start, 1))
    }

    fn selected(join: Origin, arm: usize) -> BranchConstraints {
        let mut constraints = BranchConstraints::unconstrained();
        constraints.select(join, arm);
        constraints
    }

    #[test]
    fn typed_provenance_order_selection_replaces_arm_and_canonicalizes_choices() {
        let numerically_first = origin(15, 2);
        let numerically_later = origin(16, 1);
        let mut constraints = selected(numerically_later, 0);
        constraints.select(numerically_first, 1);
        constraints.select(numerically_later, 1);

        assert_eq!(
            constraints.alternatives,
            vec![vec![(numerically_first, 1), (numerically_later, 1)]]
        );
    }

    #[test]
    fn typed_provenance_order_branch_arms_are_distinct_and_total() {
        let join = origin(15, 1);
        let first_arm = selected(join, 0);
        let second_arm = selected(join, 1);

        assert_ne!(first_arm, second_arm);
        assert_eq!(first_arm.structural_cmp(&second_arm), Ordering::Less);
        assert_eq!(second_arm.structural_cmp(&first_arm), Ordering::Greater);

        let mut alternatives = second_arm.clone();
        alternatives.merge(first_arm.clone());
        assert_eq!(
            alternatives.alternatives,
            vec![vec![(join, 0)], vec![(join, 1)]]
        );
        assert_ne!(alternatives.structural_cmp(&first_arm), Ordering::Equal);
    }

    #[test]
    fn typed_provenance_order_unequal_constraints_never_compare_equal() {
        let first = origin(15, 1);
        let second = origin(16, 1);
        let first_arm = selected(first, 0);
        let second_arm = selected(first, 1);
        let impossible = first_arm.intersection(&second_arm);
        let unconstrained = BranchConstraints::unconstrained();
        let mut conjunction = first_arm.clone();
        conjunction.select(second, 0);
        let mut disjunction = first_arm.clone();
        disjunction.merge(selected(second, 0));
        let constraints = [
            impossible,
            unconstrained,
            first_arm,
            second_arm,
            conjunction,
            disjunction,
        ];

        for left in &constraints {
            for right in &constraints {
                assert_eq!(
                    left.structural_cmp(right) == Ordering::Equal,
                    left == right,
                    "structural equality and comparison must agree"
                );
            }
        }
    }

    #[test]
    fn typed_provenance_order_conjunction_and_disjunction_are_order_independent() {
        let first = origin(15, 1);
        let second = origin(16, 1);

        let mut forward_conjunction = selected(first, 0);
        forward_conjunction.select(second, 1);
        let mut reversed_conjunction = selected(second, 1);
        reversed_conjunction.select(first, 0);
        assert_eq!(forward_conjunction, reversed_conjunction);
        assert_eq!(
            forward_conjunction.structural_cmp(&reversed_conjunction),
            Ordering::Equal
        );

        let first_alternative = selected(first, 0);
        let second_alternative = selected(second, 1);
        let mut forward_disjunction = first_alternative.clone();
        forward_disjunction.merge(second_alternative.clone());
        let mut reversed_disjunction = second_alternative;
        reversed_disjunction.merge(first_alternative);
        assert_eq!(forward_disjunction, reversed_disjunction);
        assert_eq!(
            forward_disjunction.structural_cmp(&reversed_disjunction),
            Ordering::Equal
        );
    }

    #[test]
    fn merge_and_intersection_are_deterministic() {
        let left = selected(origin(0, 1), 0);
        let right = selected(origin(0, 2), 1);

        let mut left_then_right = left.clone();
        left_then_right.merge(right.clone());
        let mut right_then_left = right.clone();
        right_then_left.merge(left.clone());
        assert_eq!(left_then_right, right_then_left);
        assert_eq!(left.intersection(&right), right.intersection(&left));
    }

    #[test]
    fn conflicts_are_impossible_and_compatible_constraints_intersect() {
        let join = origin(0, 1);
        let left = selected(join, 0);
        let conflicting = selected(join, 1);
        let independent = selected(origin(0, 2), 0);

        assert!(left.intersection(&conflicting).is_impossible());
        assert!(!left.compatible_with(&conflicting));
        assert!(!left.intersection(&independent).is_impossible());
        assert!(left.compatible_with(&independent));
    }

    #[test]
    fn compatibility_handles_unconstrained_impossible_and_disjunctive_paths() {
        let join = origin(0, 1);
        let first_arm = selected(join, 0);
        let second_arm = selected(join, 1);
        let impossible = first_arm.intersection(&second_arm);
        let unconstrained = BranchConstraints::unconstrained();
        let mut either_arm = first_arm.clone();
        either_arm.merge(second_arm.clone());

        assert!(unconstrained.compatible_with(&first_arm));
        assert!(first_arm.compatible_with(&unconstrained));
        assert!(!impossible.compatible_with(&first_arm));
        assert!(!first_arm.compatible_with(&impossible));
        assert!(first_arm.compatible_with(&first_arm));
        assert!(!first_arm.compatible_with(&second_arm));
        assert!(either_arm.compatible_with(&second_arm));
        assert!(second_arm.compatible_with(&either_arm));
    }
}
