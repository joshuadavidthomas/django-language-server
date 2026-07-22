use std::cmp::Ordering;

use djls_source::Origin;

use super::StructuralOrd;
use crate::python::PythonSourceModule;

/// One finite control-flow join. The module identity and source origin name
/// one execution coordinate; `arm_count` records its complete modeled domain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct BranchJoin {
    module: PythonSourceModule,
    origin: Origin,
    arm_count: usize,
}

impl BranchJoin {
    pub(super) fn new(module: PythonSourceModule, origin: Origin, arm_count: usize) -> Self {
        assert!(arm_count > 0, "a branch join must have at least one arm");
        Self {
            module,
            origin,
            arm_count,
        }
    }

    pub(super) fn origin(&self) -> Origin {
        self.origin
    }

    fn identity_cmp(&self, other: &Self) -> Ordering {
        self.module
            .structural_cmp(&other.module)
            .then_with(|| self.origin.structural_cmp(&other.origin))
    }

    fn assert_same_domain(&self, other: &Self) {
        assert_eq!(
            self.arm_count, other.arm_count,
            "one branch join coordinate cannot have two arm domains"
        );
    }

    #[cfg(test)]
    fn for_test(origin: Origin, arm_count: usize) -> Self {
        use camino::Utf8PathBuf;

        use crate::python::PythonModuleName;
        use crate::python::SearchPath;

        Self::new(
            PythonSourceModule::file_module(
                PythonModuleName::parse("test").expect("static test module name is valid"),
                Utf8PathBuf::from("/test.py"),
                origin.file,
                SearchPath::FirstParty(Utf8PathBuf::from("/")),
            ),
            origin,
            arm_count,
        )
    }
}

#[cfg(test)]
impl From<Origin> for BranchJoin {
    fn from(origin: Origin) -> Self {
        Self::for_test(origin, 2)
    }
}

impl StructuralOrd for BranchJoin {
    fn structural_cmp(&self, other: &Self) -> Ordering {
        self.module
            .structural_cmp(&other.module)
            .then_with(|| self.origin.structural_cmp(&other.origin))
            .then_with(|| self.arm_count.cmp(&other.arm_count))
    }
}

/// A canonical ordered multi-valued decision diagram over branch joins.
///
/// Branch nodes follow structural origin order along every path. A node whose
/// arms all lead to the same child is reduced to that child, so exhaustive
/// branch domains collapse without enumerating path conjunction products.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ConstraintNode {
    Impossible,
    Unconstrained,
    Branch { join: BranchJoin, arms: Vec<Self> },
}

impl ConstraintNode {
    fn selected(join: BranchJoin, arm: usize) -> Self {
        assert!(
            arm < join.arm_count,
            "branch arm {arm} is outside a {}-arm join",
            join.arm_count
        );
        let mut arms = vec![Self::Impossible; join.arm_count];
        arms[arm] = Self::Unconstrained;
        Self::branch(join, arms)
    }

    fn branch(join: BranchJoin, arms: Vec<Self>) -> Self {
        assert_eq!(
            arms.len(),
            join.arm_count,
            "branch node must cover every join arm"
        );
        if let Some(first) = arms.first()
            && arms.iter().all(|arm| arm == first)
        {
            return first.clone();
        }
        Self::Branch { join, arms }
    }

    fn collect_joins(&self, joins: &mut Vec<BranchJoin>) {
        let Self::Branch { join, arms } = self else {
            return;
        };
        if let Some(existing) = joins
            .iter()
            .find(|existing| existing.identity_cmp(join) == Ordering::Equal)
        {
            existing.assert_same_domain(join);
        } else {
            joins.push(join.clone());
        }
        for arm in arms {
            arm.collect_joins(joins);
        }
    }

    fn assert_compatible_domains(&self, other: &Self) {
        let mut joins = Vec::new();
        self.collect_joins(&mut joins);
        other.collect_joins(&mut joins);
    }

    fn union(&self, other: &Self) -> Self {
        self.assert_compatible_domains(other);
        self.union_canonical(other)
    }

    fn union_canonical(&self, other: &Self) -> Self {
        if self == other {
            return self.clone();
        }
        match (self, other) {
            (Self::Unconstrained, _) | (_, Self::Unconstrained) => Self::Unconstrained,
            (Self::Impossible, other) => other.clone(),
            (this, Self::Impossible) => this.clone(),
            (
                Self::Branch {
                    join: left_join,
                    arms: left_arms,
                },
                Self::Branch {
                    join: right_join,
                    arms: right_arms,
                },
            ) => match left_join.identity_cmp(right_join) {
                Ordering::Less => Self::branch(
                    left_join.clone(),
                    left_arms
                        .iter()
                        .map(|arm| arm.union_canonical(other))
                        .collect(),
                ),
                Ordering::Greater => Self::branch(
                    right_join.clone(),
                    right_arms
                        .iter()
                        .map(|arm| self.union_canonical(arm))
                        .collect(),
                ),
                Ordering::Equal => {
                    left_join.assert_same_domain(right_join);
                    Self::branch(
                        left_join.clone(),
                        left_arms
                            .iter()
                            .zip(right_arms)
                            .map(|(left, right)| left.union_canonical(right))
                            .collect(),
                    )
                }
            },
        }
    }

    fn intersection(&self, other: &Self) -> Self {
        self.assert_compatible_domains(other);
        self.intersection_canonical(other)
    }

    fn intersection_canonical(&self, other: &Self) -> Self {
        if self == other {
            return self.clone();
        }
        match (self, other) {
            (Self::Impossible, _) | (_, Self::Impossible) => Self::Impossible,
            (Self::Unconstrained, other) => other.clone(),
            (this, Self::Unconstrained) => this.clone(),
            (
                Self::Branch {
                    join: left_join,
                    arms: left_arms,
                },
                Self::Branch {
                    join: right_join,
                    arms: right_arms,
                },
            ) => match left_join.identity_cmp(right_join) {
                Ordering::Less => Self::branch(
                    left_join.clone(),
                    left_arms
                        .iter()
                        .map(|arm| arm.intersection_canonical(other))
                        .collect(),
                ),
                Ordering::Greater => Self::branch(
                    right_join.clone(),
                    right_arms
                        .iter()
                        .map(|arm| self.intersection_canonical(arm))
                        .collect(),
                ),
                Ordering::Equal => {
                    left_join.assert_same_domain(right_join);
                    Self::branch(
                        left_join.clone(),
                        left_arms
                            .iter()
                            .zip(right_arms)
                            .map(|(left, right)| left.intersection_canonical(right))
                            .collect(),
                    )
                }
            },
        }
    }

    /// Existentially remove one join so selecting it again replaces its prior
    /// arm, matching assignment at a repeated control-flow coordinate.
    fn forget(&self, forgotten: &BranchJoin) -> Self {
        match self {
            Self::Impossible | Self::Unconstrained => self.clone(),
            Self::Branch { join, arms } => match join.identity_cmp(forgotten) {
                Ordering::Less => Self::branch(
                    join.clone(),
                    arms.iter().map(|arm| arm.forget(forgotten)).collect(),
                ),
                Ordering::Greater => self.clone(),
                Ordering::Equal => {
                    join.assert_same_domain(forgotten);
                    arms.iter()
                        .fold(Self::Impossible, |result, arm| result.union_canonical(arm))
                }
            },
        }
    }
}

impl StructuralOrd for ConstraintNode {
    fn structural_cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::Impossible, Self::Impossible) | (Self::Unconstrained, Self::Unconstrained) => {
                Ordering::Equal
            }
            (Self::Impossible, _) | (Self::Unconstrained, Self::Branch { .. }) => Ordering::Less,
            (_, Self::Impossible) | (Self::Branch { .. }, Self::Unconstrained) => Ordering::Greater,
            (
                Self::Branch {
                    join: left_join,
                    arms: left_arms,
                },
                Self::Branch {
                    join: right_join,
                    arms: right_arms,
                },
            ) => left_join
                .structural_cmp(right_join)
                .then_with(|| structural_cmp_nodes(left_arms, right_arms)),
        }
    }
}

fn structural_cmp_nodes(left: &[ConstraintNode], right: &[ConstraintNode]) -> Ordering {
    for (left, right) in left.iter().rev().zip(right.iter().rev()) {
        let ordering = left.structural_cmp(right);
        if ordering != Ordering::Equal {
            return ordering;
        }
    }
    left.len().cmp(&right.len())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BranchConstraints {
    root: Box<ConstraintNode>,
}

impl BranchConstraints {
    pub(crate) fn unconstrained() -> Self {
        Self {
            root: Box::new(ConstraintNode::Unconstrained),
        }
    }

    pub(super) fn select(&mut self, join: impl Into<BranchJoin>, arm: usize) {
        let join = join.into();
        *self.root = self
            .root
            .forget(&join)
            .intersection(&ConstraintNode::selected(join, arm));
    }

    pub(crate) fn merge(&mut self, other: Self) {
        let Self { root } = other;
        *self.root = self.root.union(&root);
    }

    pub(crate) fn is_impossible(&self) -> bool {
        *self.root == ConstraintNode::Impossible
    }

    pub(crate) fn intersection(&self, other: &Self) -> Self {
        Self {
            root: Box::new(self.root.intersection(&other.root)),
        }
    }

    pub(crate) fn compatible_with(&self, other: &Self) -> bool {
        !self.intersection(other).is_impossible()
    }
}

impl StructuralOrd for BranchConstraints {
    fn structural_cmp(&self, other: &Self) -> Ordering {
        self.root.structural_cmp(&other.root)
    }
}

#[cfg(test)]
mod tests {
    use std::cmp::Ordering;

    use djls_source::File;
    use djls_source::Span;
    use salsa::Id;
    use salsa::plumbing::FromId as _;

    use super::BranchConstraints;
    use super::BranchJoin;
    use super::ConstraintNode;
    use super::Origin;
    use super::PythonSourceModule;
    use super::StructuralOrd;

    fn origin(file_index: u32, start: u32) -> Origin {
        // SAFETY: Test indexes are below `salsa::Id::MAX_U32`; these synthetic
        // files are compared only as opaque IDs and are never read.
        let file = File::from_id(unsafe { Id::from_index(file_index) });
        Origin::new(file, Span::new(start, 1))
    }

    #[derive(Clone, Copy)]
    struct TestJoin {
        origin: Origin,
        arm_count: usize,
    }

    fn join(origin: Origin, arm_count: usize) -> TestJoin {
        TestJoin { origin, arm_count }
    }

    impl From<TestJoin> for BranchJoin {
        fn from(join: TestJoin) -> Self {
            Self::for_test(join.origin, join.arm_count)
        }
    }

    fn selected(join: TestJoin, arm: usize) -> BranchConstraints {
        let mut constraints = BranchConstraints::unconstrained();
        constraints.select(BranchJoin::for_test(join.origin, join.arm_count), arm);
        constraints
    }

    fn impossible() -> BranchConstraints {
        BranchConstraints {
            root: Box::new(ConstraintNode::Impossible),
        }
    }

    #[test]
    fn selection_replaces_an_existing_arm() {
        let branch = join(origin(15, 1), 2);
        let mut constraints = selected(branch, 0);
        constraints.select(branch, 1);

        assert_eq!(constraints, selected(branch, 1));
    }

    #[test]
    fn structural_order_follows_branch_arm_order() {
        let branch = join(origin(15, 1), 3);

        assert_eq!(
            selected(branch, 0).structural_cmp(&selected(branch, 1)),
            Ordering::Less
        );
        assert_eq!(
            selected(branch, 1).structural_cmp(&selected(branch, 2)),
            Ordering::Less
        );
    }

    #[test]
    fn all_join_arms_reduce_to_the_shared_residual() {
        let branch = join(origin(15, 1), 2);
        let residual = join(origin(16, 1), 2);
        let mut first_arm = selected(branch, 0).intersection(&selected(residual, 1));
        let second_arm = selected(branch, 1).intersection(&selected(residual, 1));
        first_arm.merge(second_arm);

        assert_eq!(first_arm, selected(residual, 1));
    }

    #[test]
    fn large_complete_join_reduces_to_unconstrained() {
        let branch = join(origin(15, 1), 23);
        let mut constraints = impossible();
        for arm in 0..23 {
            constraints.merge(selected(branch, arm));
        }

        assert_eq!(constraints, BranchConstraints::unconstrained());
    }

    #[test]
    fn one_arm_domain_is_unconstrained() {
        let branch = join(origin(15, 1), 1);
        assert_eq!(selected(branch, 0), BranchConstraints::unconstrained());
    }

    #[test]
    fn incomplete_or_different_residual_groups_do_not_reduce() {
        let branch = join(origin(15, 1), 3);
        let first_residual = join(origin(16, 1), 2);
        let second_residual = join(origin(17, 1), 2);
        let mut incomplete = selected(branch, 0);
        incomplete.merge(selected(branch, 1));
        assert_ne!(incomplete, BranchConstraints::unconstrained());

        let mut different = selected(branch, 0).intersection(&selected(first_residual, 0));
        different.merge(selected(branch, 1).intersection(&selected(second_residual, 0)));
        assert_ne!(different, BranchConstraints::unconstrained());
    }

    #[test]
    fn nested_complete_domains_reduce_without_path_products() {
        let joins = [
            join(origin(15, 1), 23),
            join(origin(16, 1), 2),
            join(origin(17, 1), 4),
            join(origin(18, 1), 2),
        ];
        let mut constraints = impossible();
        for first in 0..23 {
            for second in 0..2 {
                for third in 0..4 {
                    for fourth in 0..2 {
                        let mut alternative = BranchConstraints::unconstrained();
                        for (join, arm) in joins.into_iter().zip([first, second, third, fourth]) {
                            alternative = alternative.intersection(&selected(join, arm));
                        }
                        constraints.merge(alternative);
                    }
                }
            }
        }

        assert_eq!(constraints, BranchConstraints::unconstrained());
    }

    #[test]
    fn normalization_is_independent_of_merge_grouping() {
        let first = join(origin(15, 1), 2);
        let second = join(origin(16, 1), 2);
        let a = selected(first, 0).intersection(&selected(second, 0));
        let b = selected(first, 1).intersection(&selected(second, 0));
        let c = selected(first, 0).intersection(&selected(second, 1));

        let mut left_grouped = a.clone();
        left_grouped.merge(b.clone());
        left_grouped.merge(c.clone());

        let mut right_grouped = b;
        right_grouped.merge(c);
        let mut right_grouped_result = a;
        right_grouped_result.merge(right_grouped);

        assert_eq!(left_grouped, right_grouped_result);
    }

    #[test]
    fn structural_order_unequal_constraints_never_compare_equal() {
        let first = join(origin(15, 1), 2);
        let second = join(origin(16, 1), 2);
        let first_arm = selected(first, 0);
        let second_arm = selected(first, 1);
        let impossible = first_arm.intersection(&second_arm);
        let unconstrained = BranchConstraints::unconstrained();
        let conjunction = first_arm.intersection(&selected(second, 0));
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
    fn merge_and_intersection_obey_algebraic_laws() {
        let first = selected(join(origin(0, 1), 2), 0);
        let second = selected(join(origin(0, 2), 2), 1);
        let third = selected(join(origin(0, 3), 3), 2);

        let mut left_grouped = first.clone();
        left_grouped.merge(second.clone());
        left_grouped.merge(third.clone());
        let mut right_grouped = second.clone();
        right_grouped.merge(third.clone());
        let mut right_result = first.clone();
        right_result.merge(right_grouped);
        assert_eq!(left_grouped, right_result, "union must be associative");

        let left_intersection = first.intersection(&second).intersection(&third);
        let right_intersection = first.intersection(&second.intersection(&third));
        assert_eq!(
            left_intersection, right_intersection,
            "intersection must be associative"
        );
        assert_eq!(first.intersection(&second), second.intersection(&first));

        let mut idempotent = first.clone();
        idempotent.merge(first.clone());
        assert_eq!(idempotent, first);
    }

    #[test]
    fn conflicts_are_impossible_and_compatible_constraints_intersect() {
        let branch = join(origin(0, 1), 2);
        let left = selected(branch, 0);
        let conflicting = selected(branch, 1);
        let independent = selected(join(origin(0, 2), 2), 0);

        assert!(left.intersection(&conflicting).is_impossible());
        assert!(!left.compatible_with(&conflicting));
        assert!(!left.intersection(&independent).is_impossible());
        assert!(left.compatible_with(&independent));
    }

    #[test]
    #[should_panic(expected = "one branch join coordinate cannot have two arm domains")]
    fn one_join_coordinate_cannot_have_two_domains() {
        let two_arms = join(origin(0, 1), 2);
        let three_arms = join(origin(0, 1), 3);
        let _ = selected(two_arms, 0).intersection(&selected(three_arms, 0));
    }

    #[test]
    #[should_panic(expected = "one branch join coordinate cannot have two arm domains")]
    fn domain_mismatch_is_rejected_under_disjoint_outer_arms() {
        let outer = join(origin(0, 1), 2);
        let two_arms = join(origin(0, 2), 2);
        let three_arms = join(origin(0, 2), 3);
        let mut left = selected(outer, 0).intersection(&selected(two_arms, 0));
        let right = selected(outer, 1).intersection(&selected(three_arms, 0));

        left.merge(right);
    }

    #[test]
    fn module_identity_separates_equal_source_origins() {
        use camino::Utf8PathBuf;

        use crate::python::PythonModuleName;
        use crate::python::SearchPath;

        let source_origin = origin(0, 1);
        let module = |root: &str| {
            PythonSourceModule::file_module(
                PythonModuleName::parse("shared").expect("static test module name is valid"),
                Utf8PathBuf::from("/shared.py"),
                source_origin.file,
                SearchPath::FirstParty(Utf8PathBuf::from(root)),
            )
        };
        let first = BranchJoin::new(module("/first"), source_origin, 2);
        let second = BranchJoin::new(module("/second"), source_origin, 2);
        let constrained = |join: BranchJoin, arm| {
            let mut constraints = BranchConstraints::unconstrained();
            constraints.select(join, arm);
            constraints
        };

        let cross_execution =
            constrained(first.clone(), 0).intersection(&constrained(second.clone(), 1));
        assert!(!cross_execution.is_impossible());

        let mut replaced = cross_execution;
        replaced.select(first.clone(), 1);
        assert_eq!(
            replaced,
            constrained(first, 1).intersection(&constrained(second, 1))
        );
    }

    #[test]
    #[should_panic(expected = "outside a 2-arm join")]
    fn selection_rejects_an_out_of_domain_arm() {
        let _ = selected(join(origin(0, 1), 2), 2);
    }
}
