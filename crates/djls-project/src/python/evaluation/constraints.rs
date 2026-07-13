use djls_source::Origin;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BranchConstraints {
    // Disjunction of conjunctions. Each conjunction records one arm selected at a control-flow
    // join. Origins identify joins without retaining AST nodes across query boundaries.
    pub(super) alternatives: Vec<Vec<(Origin, usize)>>,
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

    pub(super) fn merge(&mut self, other: Self) {
        self.alternatives.extend(other.alternatives);
        self.normalize();
    }

    fn normalize(&mut self) {
        self.alternatives
            .sort_by_key(|choices| format!("{choices:?}"));
        self.alternatives.dedup();
    }

    pub(crate) fn normalized_alternatives(&self) -> &[Vec<(Origin, usize)>] {
        &self.alternatives
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
}
