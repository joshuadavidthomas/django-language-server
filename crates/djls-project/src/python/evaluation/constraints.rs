use djls_source::Origin;

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
            alternative.sort_by_key(|choice| format!("{choice:?}"));
        }
        self.normalize();
    }

    pub(crate) fn merge(&mut self, other: Self) {
        self.alternatives.extend(other.alternatives);
        self.normalize();
    }

    fn normalize(&mut self) {
        self.alternatives
            .sort_by_key(|choices| format!("{choices:?}"));
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
                    choices.sort_by_key(|choice| format!("{choice:?}"));
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
            other.alternatives.iter().any(|right| {
                left.iter().all(|(left_join, left_arm)| {
                    right.iter().all(|(right_join, right_arm)| {
                        left_join != right_join || left_arm == right_arm
                    })
                })
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use djls_source::File;
    use djls_source::Span;
    use salsa::plumbing::FromId;
    use salsa::plumbing::Id;

    use super::BranchConstraints;
    use super::Origin;

    fn origin(offset: usize) -> Origin {
        let file = File::from_id(Id::from_bits(1));
        Origin::new(file, Span::saturating_from_parts_usize(offset, 1))
    }

    fn selected(join: Origin, arm: usize) -> BranchConstraints {
        let mut constraints = BranchConstraints::unconstrained();
        constraints.select(join, arm);
        constraints
    }

    #[test]
    fn selection_replaces_the_join_arm_and_keeps_canonical_choices() {
        let first = origin(1);
        let second = origin(2);
        let mut constraints = selected(first, 0);
        constraints.select(second, 1);
        constraints.select(first, 1);

        let mut expected = vec![(first, 1), (second, 1)];
        expected.sort_by_key(|choice| format!("{choice:?}"));
        assert_eq!(constraints.alternatives, vec![expected]);
    }

    #[test]
    fn merge_and_intersection_are_deterministic() {
        let left = selected(origin(1), 0);
        let right = selected(origin(2), 1);

        let mut left_then_right = left.clone();
        left_then_right.merge(right.clone());
        let mut right_then_left = right.clone();
        right_then_left.merge(left.clone());
        assert_eq!(left_then_right, right_then_left);
        assert_eq!(left.intersection(&right), right.intersection(&left));
    }

    #[test]
    fn conflicts_are_impossible_and_compatible_constraints_intersect() {
        let join = origin(1);
        let left = selected(join, 0);
        let conflicting = selected(join, 1);
        let independent = selected(origin(2), 0);

        assert!(left.intersection(&conflicting).is_impossible());
        assert!(!left.compatible_with(&conflicting));
        assert!(!left.intersection(&independent).is_impossible());
        assert!(left.compatible_with(&independent));
    }
}
