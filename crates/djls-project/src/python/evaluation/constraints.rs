use std::cmp::Ordering;
use std::sync::Arc;

use djls_source::Origin;

use super::StructuralOrd;
use crate::python::PythonSourceModule;

// Four correlated predicates cover common settings aliases and binary boolean
// expressions without rebuilding the path explosion this domain replaced.
// Overflow existentially forgets later predicates: it may add false-positive
// worlds, but it never removes a feasible runtime world.
const MAX_TRACKED_PREDICATES: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum BranchJoinKind {
    Control,
    BindingChoice,
    Predicate,
}

/// One stable symbolic truth input in the module-level abstract interpreter.
///
/// Unknown values keep one truth decision until a modeled rebind or mutation
/// replaces it. Stateful user-defined `__bool__` methods lie outside this
/// evaluator's static settings subset and remain covered by unsupported-call
/// and mutation evidence rather than repeated truth transitions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PredicateIdentity {
    module: Option<PythonSourceModule>,
    origin: Origin,
    discriminator: Option<String>,
}

impl PredicateIdentity {
    pub(super) fn new(origin: Origin) -> Self {
        Self {
            module: None,
            origin,
            discriminator: None,
        }
    }

    pub(super) fn discriminate(&mut self, name: &str, module: Option<&PythonSourceModule>) {
        if self.module.is_none() {
            self.module = module.cloned();
        }
        if self.discriminator.is_none() {
            self.discriminator = Some(name.to_string());
        }
    }

    pub(super) fn origin(&self) -> Origin {
        self.origin
    }
}

impl StructuralOrd for PredicateIdentity {
    fn structural_cmp(&self, other: &Self) -> Ordering {
        match (&self.module, &other.module) {
            (None, None) => Ordering::Equal,
            (None, Some(_)) => Ordering::Less,
            (Some(_), None) => Ordering::Greater,
            (Some(left), Some(right)) => left.structural_cmp(right),
        }
        .then_with(|| self.origin.structural_cmp(&other.origin))
        .then_with(|| self.discriminator.cmp(&other.discriminator))
    }
}

/// One finite decision join. The module identity, source origin, and kind name
/// one coordinate; `arm_count` records its complete modeled domain.
#[derive(Debug, Clone, Eq)]
pub(super) struct BranchJoin {
    module: PythonSourceModule,
    origin: Origin,
    arm_count: usize,
    kind: BranchJoinKind,
    predicate_discriminator: Option<String>,
}

impl BranchJoin {
    pub(super) fn new(module: PythonSourceModule, origin: Origin, arm_count: usize) -> Self {
        assert!(arm_count > 0, "a branch join must have at least one arm");
        Self {
            module,
            origin,
            arm_count,
            kind: BranchJoinKind::Control,
            predicate_discriminator: None,
        }
    }

    pub(super) fn binding_choice(module: PythonSourceModule, origin: Origin, name: &str) -> Self {
        Self {
            module,
            origin,
            arm_count: 2,
            kind: BranchJoinKind::BindingChoice,
            predicate_discriminator: Some(name.to_string()),
        }
    }

    pub(super) fn predicate(module: PythonSourceModule, identity: &PredicateIdentity) -> Self {
        Self {
            module: identity.module.clone().unwrap_or(module),
            origin: identity.origin,
            arm_count: 2,
            kind: BranchJoinKind::Predicate,
            predicate_discriminator: identity.discriminator.clone(),
        }
    }

    pub(super) fn origin(&self) -> Origin {
        self.origin
    }

    fn identity_cmp(&self, other: &Self) -> Ordering {
        self.kind
            .cmp(&other.kind)
            .then_with(|| self.module.structural_cmp(&other.module))
            .then_with(|| self.origin.structural_cmp(&other.origin))
            .then_with(|| {
                self.predicate_discriminator
                    .cmp(&other.predicate_discriminator)
            })
    }

    fn same_identity(&self, other: &Self) -> bool {
        // Equality can reject distinct source sites before comparing module
        // paths. Canonical ordering must still compare modules first.
        self.kind == other.kind
            && self.origin == other.origin
            && self.predicate_discriminator == other.predicate_discriminator
            && self.module == other.module
    }

    fn assert_same_domain(&self, other: &Self) {
        assert_eq!(
            self.arm_count, other.arm_count,
            "one branch join coordinate cannot have two arm domains"
        );
    }

    #[cfg(test)]
    pub(super) fn predicate_for_test(origin: Origin) -> Self {
        let mut join = Self::for_test(origin, 2);
        join.kind = BranchJoinKind::Predicate;
        join
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

impl PartialEq for BranchJoin {
    fn eq(&self, other: &Self) -> bool {
        self.arm_count == other.arm_count && self.same_identity(other)
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
        self.identity_cmp(other)
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
    Branch(Arc<ConstraintBranch>),
}

// Only branches allocate. Immutable sharing makes clones of both roots and
// interior residuals cheap without introducing identities into equality or order.
#[derive(Debug, PartialEq, Eq)]
struct ConstraintBranch {
    join: BranchJoin,
    arms: Vec<ConstraintNode>,
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

    fn selected_arms(join: BranchJoin, selected: &[usize]) -> Self {
        let mut arms = vec![Self::Impossible; join.arm_count];
        for &arm in selected {
            let Some(selected_arm) = arms.get_mut(arm) else {
                // An invalid internal selection must not exclude any feasible arm.
                return Self::Unconstrained;
            };
            *selected_arm = Self::Unconstrained;
        }
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
        Self::Branch(Arc::new(ConstraintBranch { join, arms }))
    }

    fn collect_joins<'a>(&'a self, joins: &mut Vec<&'a BranchJoin>) {
        let Self::Branch(branch) = self else {
            return;
        };
        let ConstraintBranch { join, arms } = branch.as_ref();
        if let Some(existing) = joins.iter().find(|existing| existing.same_identity(join)) {
            existing.assert_same_domain(join);
        } else {
            joins.push(join);
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

    fn widen_predicates(self) -> Self {
        let mut joins = Vec::new();
        self.collect_joins(&mut joins);
        let mut predicates = joins
            .into_iter()
            .filter(|join| join.kind == BranchJoinKind::Predicate)
            .collect::<Vec<_>>();
        predicates.sort_by(|left, right| left.structural_cmp(right));
        let forgotten = predicates
            .into_iter()
            .skip(MAX_TRACKED_PREDICATES)
            .cloned()
            .collect::<Vec<_>>();
        forgotten
            .iter()
            .fold(self, |constraints, join| constraints.forget(join))
    }

    fn union(&self, other: &Self) -> Self {
        self.assert_compatible_domains(other);
        self.union_canonical(other).widen_predicates()
    }

    fn union_canonical(&self, other: &Self) -> Self {
        if self == other {
            return self.clone();
        }
        match (self, other) {
            (Self::Unconstrained, _) | (_, Self::Unconstrained) => Self::Unconstrained,
            (Self::Impossible, other) => other.clone(),
            (this, Self::Impossible) => this.clone(),
            (Self::Branch(left), Self::Branch(right)) => {
                match left.join.identity_cmp(&right.join) {
                    Ordering::Less => Self::branch(
                        left.join.clone(),
                        left.arms
                            .iter()
                            .map(|arm| arm.union_canonical(other))
                            .collect(),
                    ),
                    Ordering::Greater => Self::branch(
                        right.join.clone(),
                        right
                            .arms
                            .iter()
                            .map(|arm| self.union_canonical(arm))
                            .collect(),
                    ),
                    Ordering::Equal => {
                        left.join.assert_same_domain(&right.join);
                        Self::branch(
                            left.join.clone(),
                            left.arms
                                .iter()
                                .zip(&right.arms)
                                .map(|(left, right)| left.union_canonical(right))
                                .collect(),
                        )
                    }
                }
            }
        }
    }

    fn intersection(&self, other: &Self) -> Self {
        self.assert_compatible_domains(other);
        self.intersection_canonical(other).widen_predicates()
    }

    fn intersection_canonical(&self, other: &Self) -> Self {
        if self == other {
            return self.clone();
        }
        match (self, other) {
            (Self::Impossible, _) | (_, Self::Impossible) => Self::Impossible,
            (Self::Unconstrained, other) => other.clone(),
            (this, Self::Unconstrained) => this.clone(),
            (Self::Branch(left), Self::Branch(right)) => {
                match left.join.identity_cmp(&right.join) {
                    Ordering::Less => Self::branch(
                        left.join.clone(),
                        left.arms
                            .iter()
                            .map(|arm| arm.intersection_canonical(other))
                            .collect(),
                    ),
                    Ordering::Greater => Self::branch(
                        right.join.clone(),
                        right
                            .arms
                            .iter()
                            .map(|arm| self.intersection_canonical(arm))
                            .collect(),
                    ),
                    Ordering::Equal => {
                        left.join.assert_same_domain(&right.join);
                        Self::branch(
                            left.join.clone(),
                            left.arms
                                .iter()
                                .zip(&right.arms)
                                .map(|(left, right)| left.intersection_canonical(right))
                                .collect(),
                        )
                    }
                }
            }
        }
    }

    /// Existentially remove one join so selecting it again replaces its prior
    /// arm, matching assignment at a repeated control-flow coordinate.
    fn forget(&self, forgotten: &BranchJoin) -> Self {
        match self {
            Self::Impossible | Self::Unconstrained => self.clone(),
            Self::Branch(branch) => match branch.join.identity_cmp(forgotten) {
                Ordering::Less => Self::branch(
                    branch.join.clone(),
                    branch
                        .arms
                        .iter()
                        .map(|arm| arm.forget(forgotten))
                        .collect(),
                ),
                Ordering::Greater => self.clone(),
                Ordering::Equal => {
                    branch.join.assert_same_domain(forgotten);
                    branch
                        .arms
                        .iter()
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
            (Self::Impossible, _) | (Self::Unconstrained, Self::Branch(_)) => Ordering::Less,
            (_, Self::Impossible) | (Self::Branch(_), Self::Unconstrained) => Ordering::Greater,
            (Self::Branch(left), Self::Branch(right)) => {
                if Arc::ptr_eq(left, right) {
                    return Ordering::Equal;
                }
                left.join
                    .structural_cmp(&right.join)
                    .then_with(|| structural_cmp_nodes(&left.arms, &right.arms))
            }
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
    root: ConstraintNode,
}

impl BranchConstraints {
    pub(crate) fn unconstrained() -> Self {
        Self {
            root: ConstraintNode::Unconstrained,
        }
    }

    pub(super) fn select(&mut self, join: impl Into<BranchJoin>, arm: usize) {
        let join = join.into();
        self.root = self
            .root
            .forget(&join)
            .intersection(&ConstraintNode::selected(join, arm));
    }

    pub(super) fn select_arms(&mut self, join: impl Into<BranchJoin>, arms: &[usize]) {
        let join = join.into();
        self.root = self
            .root
            .forget(&join)
            .intersection(&ConstraintNode::selected_arms(join, arms));
    }

    /// Require one arm without forgetting an earlier requirement for the same
    /// coordinate. Repeated predicate assumptions must conflict rather than
    /// replace one another as repeated control-flow executions do.
    pub(super) fn required(join: BranchJoin, arm: usize) -> Self {
        Self {
            root: ConstraintNode::selected(join, arm),
        }
    }

    pub(super) fn impossible() -> Self {
        Self {
            root: ConstraintNode::Impossible,
        }
    }

    pub(crate) fn merge(&mut self, other: Self) {
        let Self { root } = other;
        self.root = self.root.union(&root);
    }

    pub(crate) fn is_impossible(&self) -> bool {
        self.root == ConstraintNode::Impossible
    }

    pub(crate) fn intersection(&self, other: &Self) -> Self {
        Self {
            root: self.root.intersection(&other.root),
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
    use std::sync::Arc;

    use djls_source::File;
    use djls_source::Span;
    use salsa::Id;
    use salsa::plumbing::FromId as _;

    use super::BranchConstraints;
    use super::BranchJoin;
    use super::ConstraintNode;
    use super::MAX_TRACKED_PREDICATES;
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
            root: ConstraintNode::Impossible,
        }
    }

    #[test]
    fn identity_equality_matches_canonical_order_without_including_arm_count() {
        use camino::Utf8PathBuf;

        use crate::python::PythonModuleName;
        use crate::python::SearchPath;

        let base = BranchJoin::for_test(origin(0, 1), 2);
        let different_domain = BranchJoin {
            arm_count: 3,
            ..base.clone()
        };
        assert!(base.same_identity(&different_domain));
        let joins = [
            base.clone(),
            different_domain,
            BranchJoin::for_test(origin(0, 2), 2),
            BranchJoin::for_test(origin(1, 1), 2),
            BranchJoin::predicate_for_test(base.origin),
            BranchJoin::binding_choice(base.module.clone(), base.origin, "first"),
            BranchJoin::binding_choice(base.module.clone(), base.origin, "second"),
            BranchJoin::new(
                PythonSourceModule::file_module(
                    PythonModuleName::parse("test").expect("valid test module"),
                    Utf8PathBuf::from("/test.py"),
                    base.origin.file,
                    SearchPath::FirstParty(Utf8PathBuf::from("/different-root")),
                ),
                base.origin,
                2,
            ),
        ];

        for left in &joins {
            for right in &joins {
                assert_eq!(
                    left.same_identity(right),
                    left.identity_cmp(right) == Ordering::Equal,
                    "identity equality must preserve every canonical identity field"
                );
                assert_eq!(
                    left == right,
                    left.structural_cmp(right) == Ordering::Equal,
                    "full equality must also include the arm domain"
                );
            }
        }
    }

    #[test]
    fn control_and_predicate_joins_at_one_origin_do_not_collide() {
        let shared_origin = origin(0, 1);
        let control = BranchJoin::for_test(shared_origin, 3);
        let predicate = BranchJoin::predicate_for_test(shared_origin);
        let constraints = BranchConstraints::required(control, 2)
            .intersection(&BranchConstraints::required(predicate, 1));

        assert!(!constraints.is_impossible());
    }

    #[test]
    fn predicate_overflow_forgets_later_decisions() {
        let joins = (1_u32..)
            .take(MAX_TRACKED_PREDICATES + 1)
            .map(|start| BranchJoin::predicate_for_test(origin(0, start)))
            .collect::<Vec<_>>();
        let constraints =
            joins
                .iter()
                .fold(BranchConstraints::unconstrained(), |constraints, join| {
                    constraints.intersection(&BranchConstraints::required(join.clone(), 1))
                });

        assert!(
            !constraints.compatible_with(&BranchConstraints::required(joins[0].clone(), 0)),
            "the earliest tracked predicate should remain required"
        );
        assert!(
            constraints.compatible_with(&BranchConstraints::required(
                joins[MAX_TRACKED_PREDICATES].clone(),
                0,
            )),
            "overflow should conservatively forget the latest predicate"
        );
    }

    #[test]
    fn predicate_widening_preserves_each_intermediate_forgetting_boundary() {
        let joins = (0..6)
            .map(|start| BranchJoin::predicate_for_test(origin(0, start)))
            .collect::<Vec<_>>();
        let mut constraints = BranchConstraints::required(joins[4].clone(), 1);
        assert!(
            constraints
                .intersection(&BranchConstraints::required(joins[4].clone(), 0))
                .is_impossible()
        );

        // Build the expected exact diagram directly, independently of Apply and
        // widening. At the fourth addition, P4 must already have disappeared.
        let expected_coordinates: &[&[usize]] =
            &[&[0, 4], &[0, 1, 4], &[0, 1, 2, 4], &[0, 1, 2, 3]];
        for (next, coordinates) in expected_coordinates.iter().enumerate() {
            constraints =
                constraints.intersection(&BranchConstraints::required(joins[next].clone(), 1));
            let expected =
                coordinates
                    .iter()
                    .rev()
                    .fold(ConstraintNode::Unconstrained, |residual, &index| {
                        ConstraintNode::branch(
                            joins[index].clone(),
                            vec![ConstraintNode::Impossible, residual],
                        )
                    });
            assert_eq!(constraints.root, expected);
            assert_eq!(constraints.root.structural_cmp(&expected), Ordering::Equal);
        }

        let four_predicates = constraints.clone();
        for (index, arm) in [(4, 0), (5, 1), (5, 0)] {
            constraints =
                constraints.intersection(&BranchConstraints::required(joins[index].clone(), arm));
            assert_eq!(constraints, four_predicates);
            assert!(!constraints.is_impossible());
        }

        // This call forgets two predicates at the same boundary, in structural
        // order; neither operand alone exceeds the predicate budget.
        let mut even = BranchConstraints::unconstrained();
        let mut odd = BranchConstraints::unconstrained();
        for index in [0, 2, 4] {
            even = even.intersection(&BranchConstraints::required(joins[index].clone(), 1));
            odd = odd.intersection(&BranchConstraints::required(joins[index + 1].clone(), 1));
        }
        assert_eq!(even.intersection(&odd), four_predicates);
    }

    #[test]
    fn required_arms_for_one_join_conflict() {
        let coordinate = BranchJoin::for_test(origin(0, 1), 2);
        let falsy = BranchConstraints::required(coordinate.clone(), 0);
        let truthy = BranchConstraints::required(coordinate, 1);

        assert!(falsy.intersection(&truthy).is_impossible());
    }

    #[test]
    fn selection_replaces_an_existing_arm() {
        let branch = join(origin(15, 1), 2);
        let mut constraints = selected(branch, 0);
        constraints.select(branch, 1);

        assert_eq!(constraints, selected(branch, 1));
    }

    #[test]
    fn shared_constraints_keep_cloned_selections_independent_and_reuse_residuals() {
        let branch = join(origin(15, 1), 3);
        let residual_join = join(origin(16, 1), 2);
        let residual = selected(residual_join, 1);
        let original = selected(branch, 2).intersection(&residual);
        let mut changed = original.clone();
        assert!(matches!(
            (&original.root, &changed.root),
            (ConstraintNode::Branch(left), ConstraintNode::Branch(right)) if Arc::ptr_eq(left, right)
        ));

        changed.select_arms(branch, &[0, 1]);
        assert_eq!(original, selected(branch, 2).intersection(&residual));
        assert!(original.intersection(&changed).is_impossible());
        assert_eq!(original.structural_cmp(&changed), Ordering::Greater);

        changed.merge(original);
        assert_eq!(changed, residual);
        assert!(
            matches!(
                (&changed.root, &residual.root),
                (ConstraintNode::Branch(left), ConstraintNode::Branch(right)) if Arc::ptr_eq(left, right)
            ),
            "exhausting the outer domain should reuse the interior residual"
        );

        let rebuilt = selected(residual_join, 1);
        assert!(matches!(
            (&residual.root, &rebuilt.root),
            (ConstraintNode::Branch(left), ConstraintNode::Branch(right)) if !Arc::ptr_eq(left, right)
        ));
        assert_eq!(
            residual, rebuilt,
            "allocation identity is not semantic identity"
        );
        assert_eq!(residual.structural_cmp(&rebuilt), Ordering::Equal);
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
    fn selecting_arms_matches_unioning_individual_selections() {
        let branch = join(origin(15, 1), 4);
        let mut grouped = BranchConstraints::unconstrained();
        grouped.select_arms(branch, &[0, 2, 3]);

        let mut individual = selected(branch, 0);
        individual.merge(selected(branch, 2));
        individual.merge(selected(branch, 3));

        assert_eq!(grouped, individual);
    }

    #[test]
    fn selecting_arms_preserves_residual_constraints_and_replaces_the_same_coordinate() {
        let before = join(origin(14, 1), 2);
        let branch = join(origin(15, 1), 3);
        let after = join(origin(16, 1), 2);
        let residual = selected(before, 1).intersection(&selected(after, 0));

        let mut grouped = residual.clone().intersection(&selected(branch, 1));
        grouped.select_arms(branch, &[0, 2]);

        let mut first = residual.clone();
        first.select(branch, 0);
        let mut third = residual;
        third.select(branch, 2);
        first.merge(third);

        assert_eq!(grouped, first);
    }

    #[test]
    fn selecting_a_complete_arm_domain_reduces_to_the_residual() {
        let branch = join(origin(15, 1), 3);
        let residual = selected(join(origin(16, 1), 2), 1);
        let mut grouped = residual.clone();
        grouped.select_arms(branch, &[0, 1, 2]);

        assert_eq!(grouped, residual);
    }

    #[test]
    fn selecting_an_invalid_arm_forgets_only_that_coordinate() {
        let branch = join(origin(15, 1), 3);
        let residual = selected(join(origin(16, 1), 2), 1);
        let mut grouped = residual.clone().intersection(&selected(branch, 0));
        grouped.select_arms(branch, &[0, 3]);

        assert_eq!(grouped, residual);
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
