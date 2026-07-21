//! Finite object-scoped effects produced while evaluating module imports.
//!
//! Module identity belongs to the Python domain. This evaluator-owned state
//! records loaded-child coordinates, open causes, and contamination of the
//! recognized standard-library path namespaces without embedding intrinsic
//! `PythonModuleFacts`.

use std::cmp::Ordering;

use djls_source::Origin;

use super::BranchConstraints;
use super::PythonBinding;
use super::PythonBindingState;
use super::PythonNamespaceCause;
use super::PythonUnknown;
use super::PythonUnknownCause;
use super::PythonValue;
use super::StructuralOrd;
use crate::python::NamespacePortion;
use crate::python::PythonModule;
use crate::python::PythonNamespacePackage;
use crate::python::PythonPathIntrinsic;
use crate::python::PythonPathNamespace;
use crate::python::PythonSourceModule;

impl StructuralOrd for PythonSourceModule {
    fn structural_cmp(&self, other: &Self) -> Ordering {
        self.name()
            .cmp(other.name())
            .then_with(|| self.package().cmp(&other.package()))
            .then_with(|| self.path().cmp(other.path()))
            .then_with(|| self.file().structural_cmp(&other.file()))
            .then_with(|| self.search_path().structural_cmp(other.search_path()))
    }
}

impl StructuralOrd for NamespacePortion {
    fn structural_cmp(&self, other: &Self) -> Ordering {
        self.root()
            .structural_cmp(other.root())
            .then_with(|| self.dir().cmp(other.dir()))
    }
}

impl StructuralOrd for PythonNamespacePackage {
    fn structural_cmp(&self, other: &Self) -> Ordering {
        self.name()
            .cmp(other.name())
            .then_with(|| self.portions().len().cmp(&other.portions().len()))
            .then_with(|| {
                self.portions()
                    .iter()
                    .zip(other.portions())
                    .map(|(left, right)| left.structural_cmp(right))
                    .find(|ordering| *ordering != Ordering::Equal)
                    .unwrap_or(Ordering::Equal)
            })
    }
}

impl StructuralOrd for PythonModule {
    fn structural_cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::Source(left), Self::Source(right)) => left.structural_cmp(right),
            (Self::Namespace(left), Self::Namespace(right)) => left.structural_cmp(right),
            (Self::Source(_), Self::Namespace(_)) => Ordering::Less,
            (Self::Namespace(_), Self::Source(_)) => Ordering::Greater,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ModuleChildCoordinate {
    object: PythonModule,
    attribute: String,
    binding: PythonBinding,
}

impl StructuralOrd for ModuleChildCoordinate {
    fn structural_cmp(&self, other: &Self) -> Ordering {
        self.object
            .structural_cmp(&other.object)
            .then_with(|| self.attribute.cmp(&other.attribute))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ModuleEffectCause {
    object: PythonModule,
    cause: PythonNamespaceCause,
}

impl StructuralOrd for ModuleEffectCause {
    fn structural_cmp(&self, other: &Self) -> Ordering {
        self.object
            .structural_cmp(&other.object)
            .then_with(|| self.cause.structural_cmp(&other.cause))
    }
}

/// One child-fallback transition: the selected member projection and the paths
/// where prior object coordinates must survive travel as one coherent value.
#[derive(Clone)]
pub(crate) struct ChildImportFallback {
    member: PythonBinding,
    constraints: BranchConstraints,
    preserved: Option<BranchConstraints>,
}

impl ChildImportFallback {
    pub(crate) fn new(
        member: &PythonBinding,
        preserved: Option<&BranchConstraints>,
    ) -> Option<Self> {
        Some(Self {
            member: member.clone(),
            constraints: member.import_fallback_constraints()?,
            preserved: preserved.cloned(),
        })
    }

    fn attach_child_binding(
        &self,
        prior: &PythonBinding,
        child: &PythonModule,
        origin: Origin,
    ) -> PythonBinding {
        self.member
            .attach_module_for_import_fallback(prior, child, origin, self.preserved.as_ref())
    }

    fn merge_child_effect(
        &self,
        prior: &PythonBinding,
        incoming: &PythonBinding,
        origin: Origin,
    ) -> PythonBinding {
        self.member.merge_effect_for_import_fallback(
            prior,
            incoming,
            origin,
            self.preserved.as_ref(),
        )
    }

    pub(crate) fn constraints(&self) -> &BranchConstraints {
        &self.constraints
    }
}

/// Finite, deterministic loaded-child coordinates, object-scoped open causes,
/// and path-namespace contamination. This is a private recursive-import effect
/// product; it is never added
/// to settings-facing `PythonModuleFacts` equality or projection.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub(super) struct PathIntrinsicContamination(Vec<PythonPathNamespace>);

impl PathIntrinsicContamination {
    fn insert(&mut self, intrinsic: PythonPathIntrinsic) {
        let namespace = intrinsic.mutable_namespace();
        if !self.0.contains(&namespace) {
            self.0.push(namespace);
            self.0.sort_unstable();
        }
    }

    fn contains(&self, intrinsic: PythonPathIntrinsic) -> bool {
        self.0.contains(&intrinsic.mutable_namespace())
    }

    fn absorb(&mut self, incoming: impl IntoIterator<Item = PythonPathNamespace>) {
        for namespace in incoming {
            if !self.0.contains(&namespace) {
                self.0.push(namespace);
            }
        }
        self.0.sort_unstable();
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PythonModuleEffects {
    children: Vec<ModuleChildCoordinate>,
    causes: Vec<ModuleEffectCause>,
    path_intrinsic_contamination: PathIntrinsicContamination,
}

impl PythonModuleEffects {
    pub(super) fn with_path_intrinsic_contamination(
        path_intrinsic_contamination: PathIntrinsicContamination,
    ) -> Self {
        Self {
            path_intrinsic_contamination,
            ..Self::default()
        }
    }

    pub(super) fn path_intrinsic_contamination(&self) -> &PathIntrinsicContamination {
        &self.path_intrinsic_contamination
    }

    pub(crate) fn contaminate_path_intrinsic(&mut self, intrinsic: PythonPathIntrinsic) {
        self.path_intrinsic_contamination.insert(intrinsic);
    }

    pub(crate) fn path_intrinsic_is_contaminated(&self, intrinsic: PythonPathIntrinsic) -> bool {
        self.path_intrinsic_contamination.contains(intrinsic)
    }

    fn absorb_path_intrinsic_contamination(
        &mut self,
        incoming: impl IntoIterator<Item = PythonPathNamespace>,
    ) {
        self.path_intrinsic_contamination.absorb(incoming);
    }

    fn child_index(&self, object: &PythonModule, attribute: &str) -> Option<usize> {
        self.children
            .iter()
            .position(|child| &child.object == object && child.attribute == attribute)
    }

    fn read_child(&self, object: &PythonModule, attribute: &str) -> Option<&PythonBinding> {
        self.child_index(object, attribute)
            .map(|index| &self.children[index].binding)
    }

    /// Names already attached to `object`, in deterministic coordinate order.
    /// This is a projection of current object state, never filesystem discovery.
    pub(crate) fn child_names<'a>(
        &'a self,
        object: &'a PythonModule,
    ) -> impl Iterator<Item = &'a str> {
        self.children
            .iter()
            .filter(move |child| &child.object == object)
            .map(|child| child.attribute.as_str())
    }

    /// The child-coordinate alternatives for `(object, attribute)` rebased to
    /// the read `origin`, or a fully-unconstrained `Unbound` when no coordinate
    /// is attached. Intrinsic source fallback is the caller's concern; this
    /// keeps the residual `Unbound` so the caller applies its own policy.
    pub(crate) fn child_binding(
        &self,
        object: &PythonModule,
        attribute: &str,
        origin: Origin,
    ) -> PythonBinding {
        self.read_child(object, attribute)
            .cloned()
            .map_or_else(PythonBinding::unbound, |child| {
                child.rebase_binding_origin(origin)
            })
    }

    /// Join object-scoped open causes onto a binding's residual `Unbound`
    /// alternatives, one unknown per (unbound-constraint x cause-constraint)
    /// intersection. Residual `Unbound` is retained.
    pub(crate) fn apply_open_causes(
        &self,
        object: &PythonModule,
        mut binding: PythonBinding,
        origin: Origin,
    ) -> PythonBinding {
        let unbound_constraints = binding
            .alternatives_with_constraints()
            .filter(|&(state, _constraints)| *state == PythonBindingState::Unbound)
            .map(|(_state, constraints)| constraints.clone())
            .collect::<Vec<BranchConstraints>>();
        if unbound_constraints.is_empty() {
            return binding;
        }
        for cause in self.causes_for(object) {
            for unbound in &unbound_constraints {
                let constraints = unbound.intersection(&cause.constraints);
                if let Some(unknown) =
                    PythonBinding::constrained_unknown(&cause.unknown.cause, origin, &constraints)
                {
                    binding = binding.join(unknown, origin);
                }
            }
        }
        binding
    }

    fn set_child(&mut self, object: PythonModule, attribute: String, binding: PythonBinding) {
        match self.child_index(&object, &attribute) {
            Some(index) => self.children[index].binding = binding,
            None => self.children.push(ModuleChildCoordinate {
                object,
                attribute,
                binding,
            }),
        }
        self.normalize();
    }

    /// Sequential successful child attachment: assignment-like replacement of
    /// the coordinate with a `Bound(Module(child))` value.
    pub(crate) fn attach_child(
        &mut self,
        object: PythonModule,
        attribute: String,
        child: PythonModule,
        origin: Origin,
    ) {
        let value = PythonValue::module(child, origin);
        self.set_child(object, attribute, PythonBinding::bound(value, origin));
    }

    /// Attach a child where member projection permits fallback. Exact member
    /// branches retain the prior coordinate, while cycle-seed branches retain
    /// both possibilities, so fallback never becomes unconditional.
    pub(crate) fn attach_child_for_import_fallback(
        &mut self,
        object: PythonModule,
        attribute: String,
        child: &PythonModule,
        fallback: &ChildImportFallback,
        origin: Origin,
    ) {
        let prior = self
            .read_child(&object, &attribute)
            .cloned()
            .unwrap_or_else(PythonBinding::unbound);
        let binding = fallback.attach_child_binding(&prior, child, origin);
        self.set_child(object, attribute, binding);
    }

    /// Extend object-scoped open causes in first-seen order with exact
    /// deduplication.
    pub(crate) fn open_cause(&mut self, object: PythonModule, cause: PythonNamespaceCause) {
        self.causes.push(ModuleEffectCause { object, cause });
        self.normalize();
    }

    fn causes_for<'a>(
        &'a self,
        object: &'a PythonModule,
    ) -> impl Iterator<Item = &'a PythonNamespaceCause> {
        self.causes
            .iter()
            .filter(move |entry| &entry.object == object)
            .map(|entry| &entry.cause)
    }

    /// Merge imported effects in source order.
    ///
    /// - an absent incoming key is a no-op (prior coordinate untouched);
    /// - an incoming `Unbound` preserves the prior coordinate on its
    ///   constraints;
    /// - an exact module attachment assigns on its constraints;
    /// - an unknown incoming effect joins/degrades the prior coordinate
    ///   conservatively.
    pub(crate) fn merge(&mut self, incoming: Self, origin: Origin) {
        for ModuleChildCoordinate {
            object,
            attribute,
            binding,
        } in incoming.children
        {
            let merged = match self.read_child(&object, &attribute).cloned() {
                Some(prior) => binding.merge_imported_onto(&prior, origin),
                None => binding,
            };
            self.set_child(object, attribute, merged);
        }
        self.causes.extend(incoming.causes);
        self.absorb_path_intrinsic_contamination(incoming.path_intrinsic_contamination.0);
        self.normalize();
    }

    /// Merge one conditionally evaluated fallback module's object effects only
    /// where source member projection permits fallback. Existing coordinates
    /// survive on exact member alternatives; incoming causes are likewise
    /// intersected with the feasible fallback constraints.
    pub(crate) fn merge_for_import_fallback(
        &mut self,
        incoming: Self,
        fallback: &ChildImportFallback,
        origin: Origin,
    ) {
        for ModuleChildCoordinate {
            object,
            attribute,
            binding,
        } in incoming.children
        {
            let prior = self
                .read_child(&object, &attribute)
                .cloned()
                .unwrap_or_else(PythonBinding::unbound);
            let merged = fallback.merge_child_effect(&prior, &binding, origin);
            self.set_child(object, attribute, merged);
        }
        let fallback_constraints = fallback.constraints();
        self.causes
            .extend(incoming.causes.into_iter().filter_map(|mut entry| {
                entry.cause.constraints =
                    entry.cause.constraints.intersection(fallback_constraints);
                (!entry.cause.constraints.is_impossible()).then_some(entry)
            }));
        self.absorb_path_intrinsic_contamination(incoming.path_intrinsic_contamination.0);
        self.normalize();
    }

    /// Branch join: contribute `Unbound` for a branch that did not attach a
    /// coordinate, then normalize with `PythonBinding::join`. Each open cause is
    /// retained under its branch constraints.
    pub(crate) fn join_branches(branches: &[Self], origin: Origin) -> Self {
        let mut keys: Vec<(PythonModule, String)> = Vec::new();
        for branch in branches {
            for child in &branch.children {
                let key = (child.object.clone(), child.attribute.clone());
                if !keys.contains(&key) {
                    keys.push(key);
                }
            }
        }

        let mut joined = Self::default();
        for (object, attribute) in keys {
            let mut binding: Option<PythonBinding> = None;
            for (arm, branch) in branches.iter().enumerate() {
                let mut candidate = branch
                    .read_child(&object, &attribute)
                    .cloned()
                    .unwrap_or_else(PythonBinding::unbound);
                candidate.select_branch(origin, arm);
                binding = Some(match binding {
                    Some(current) => current.join(candidate, origin),
                    None => candidate,
                });
            }
            if let Some(binding) = binding {
                joined.children.push(ModuleChildCoordinate {
                    object,
                    attribute,
                    binding,
                });
            }
        }

        for (arm, branch) in branches.iter().enumerate() {
            for entry in &branch.causes {
                let mut cause = entry.cause.clone();
                cause.select_branch(origin, arm);
                joined.causes.push(ModuleEffectCause {
                    object: entry.object.clone(),
                    cause,
                });
            }
            joined.absorb_path_intrinsic_contamination(
                branch.path_intrinsic_contamination.0.iter().copied(),
            );
        }

        joined.normalize();
        joined
    }

    /// Zero-iteration loop degradation: the baseline (`self`) path is included,
    /// and any coordinate a body changed relative to the baseline degrades to an
    /// `UnsupportedExpression` unknown joined onto the baseline alternative.
    pub(crate) fn degrade_loop(&mut self, bodies: Vec<Self>, origin: Origin) {
        let mut changed: Vec<(PythonModule, String)> = Vec::new();
        let note_changed = |object: &PythonModule, attribute: &str, changed: &mut Vec<_>| {
            let key = (object.clone(), attribute.to_string());
            if !changed.contains(&key) {
                changed.push(key);
            }
        };
        for body in &bodies {
            for child in &body.children {
                if self.read_child(&child.object, &child.attribute) != Some(&child.binding) {
                    note_changed(&child.object, &child.attribute, &mut changed);
                }
            }
            for child in &self.children {
                if body.read_child(&child.object, &child.attribute).is_none() {
                    note_changed(&child.object, &child.attribute, &mut changed);
                }
            }
        }

        for body in bodies {
            self.causes.extend(body.causes);
            self.absorb_path_intrinsic_contamination(body.path_intrinsic_contamination.0);
        }

        for (object, attribute) in changed {
            let baseline = self
                .read_child(&object, &attribute)
                .cloned()
                .unwrap_or_else(PythonBinding::unbound);
            let degraded = baseline.join(
                PythonBinding::unknown(&PythonUnknownCause::UnsupportedExpression, origin),
                origin,
            );
            self.set_child(object, attribute, degraded);
        }

        self.normalize();
    }

    /// Cycle widening: an equal coordinate survives; a changed coordinate
    /// becomes an absorbing originless `Cycle` unknown. Object-scoped causes
    /// with an equal normalized set survive; a changed set is replaced with one
    /// absorbing originless `Cycle` cause.
    pub(crate) fn widen(mut self, previous: &Self) -> Self {
        self.absorb_path_intrinsic_contamination(
            previous.path_intrinsic_contamination.0.iter().copied(),
        );
        let mut keys: Vec<(PythonModule, String)> = Vec::new();
        for child in previous.children.iter().chain(&self.children) {
            let key = (child.object.clone(), child.attribute.clone());
            if !keys.contains(&key) {
                keys.push(key);
            }
        }
        for (object, attribute) in keys {
            let prior = previous.read_child(&object, &attribute);
            let computed = self.read_child(&object, &attribute);
            if prior != computed {
                self.set_child(object, attribute, PythonBinding::originless_cycle_unknown());
            }
        }

        let mut objects: Vec<PythonModule> = Vec::new();
        for entry in previous.causes.iter().chain(&self.causes) {
            if !objects.contains(&entry.object) {
                objects.push(entry.object.clone());
            }
        }
        for object in objects {
            let prior = normalized_causes_for(&previous.causes, &object);
            let computed = normalized_causes_for(&self.causes, &object);
            if prior != computed {
                self.causes.retain(|entry| entry.object != object);
                self.causes.push(ModuleEffectCause {
                    object,
                    cause: PythonNamespaceCause::unconstrained(PythonUnknown::new(
                        PythonUnknownCause::Cycle,
                        None,
                    )),
                });
            }
        }

        self.normalize();
        self
    }

    fn normalize(&mut self) {
        self.children.sort_by(ModuleChildCoordinate::structural_cmp);

        // Open causes preserve first-seen order with exact full-value dedup: no
        // structural sort and no cause-kind coalescing, so distinct origins or
        // constraints stay as distinct open causes.
        let mut deduped: Vec<ModuleEffectCause> = Vec::new();
        for entry in std::mem::take(&mut self.causes) {
            if !deduped.contains(&entry) {
                deduped.push(entry);
            }
        }
        self.causes = deduped;
    }
}

fn normalized_causes_for(
    causes: &[ModuleEffectCause],
    object: &PythonModule,
) -> Vec<PythonNamespaceCause> {
    let mut selected: Vec<PythonNamespaceCause> = causes
        .iter()
        .filter(|entry| &entry.object == object)
        .map(|entry| entry.cause.clone())
        .collect();
    selected.sort_by(PythonNamespaceCause::structural_cmp);
    selected
}

#[cfg(test)]
mod tests {
    use camino::Utf8PathBuf;
    use djls_source::File;
    use djls_source::Span;
    use salsa::plumbing::FromId;
    use salsa::plumbing::Id;

    use super::super::PythonBindingState;
    use super::super::PythonValueKind;
    use super::*;
    use crate::python::PythonModuleName;
    use crate::python::PythonSourceModule;
    use crate::python::SearchPath;

    fn origin(start: u32) -> Origin {
        Origin::new(File::from_id(Id::from_bits(1)), Span::new(start, 1))
    }

    fn source(name: &str, id: u64) -> PythonModule {
        let module = PythonSourceModule::file_module(
            PythonModuleName::parse(name).expect("test Python module name should be valid"),
            Utf8PathBuf::from(format!("/project/{name}.py")),
            File::from_id(Id::from_bits(id)),
            SearchPath::FirstParty(Utf8PathBuf::from("/project")),
        );
        PythonModule::Source(module)
    }

    fn namespace(name: &str) -> PythonModule {
        PythonModule::Namespace(PythonNamespacePackage::new(
            PythonModuleName::parse(name).expect("test Python module name should be valid"),
            Vec::new(),
        ))
    }

    fn cause(start: u32) -> PythonNamespaceCause {
        PythonNamespaceCause::unconstrained(PythonUnknown::new(
            PythonUnknownCause::UnsupportedExpression,
            [origin(start)],
        ))
    }

    fn attached_child(binding: &PythonBinding) -> Option<PythonModule> {
        binding.alternatives().find_map(|state| {
            let PythonBindingState::Bound(bound) = state else {
                return None;
            };
            let PythonValueKind::Module(id) = &bound.value.kind else {
                return None;
            };
            Some(id.clone())
        })
    }

    fn has_cycle_unknown(binding: &PythonBinding) -> bool {
        binding.alternatives().any(|state| {
            let PythonBindingState::Bound(bound) = state else {
                return false;
            };
            bound
                .value
                .unknown_value()
                .is_some_and(|unknown| unknown.cause == PythonUnknownCause::Cycle)
        })
    }

    fn has_unsupported_unknown(binding: &PythonBinding) -> bool {
        binding.alternatives().any(|state| {
            let PythonBindingState::Bound(bound) = state else {
                return false;
            };
            bound
                .value
                .unknown_value()
                .is_some_and(|unknown| unknown.cause == PythonUnknownCause::UnsupportedExpression)
        })
    }

    fn has_unbound(binding: &PythonBinding) -> bool {
        binding
            .alternatives()
            .any(|state| *state == PythonBindingState::Unbound)
    }

    #[test]
    fn path_contamination_canonicalizes_equivalent_namespace_members() {
        let mut module = PythonModuleEffects::default();
        module.contaminate_path_intrinsic(PythonPathIntrinsic::OsModule);
        let mut member = PythonModuleEffects::default();
        member.contaminate_path_intrinsic(PythonPathIntrinsic::OsPathJoinFunction);

        assert_eq!(
            module.path_intrinsic_contamination(),
            member.path_intrinsic_contamination()
        );
    }

    #[test]
    fn conditional_attachment_consumes_absence_and_preserves_correlated_member_cases() {
        let parent = source("pkg", 1);
        let child = source("pkg.child", 2);
        let join = origin(10);
        let mut present_constraints = BranchConstraints::unconstrained();
        present_constraints.select(join, 0);
        let mut absent_constraints = BranchConstraints::unconstrained();
        absent_constraints.select(join, 1);

        let mut present = PythonBinding::bound(
            PythonValue::string("member".to_string(), origin(1)),
            origin(1),
        );
        present.select_branch(join, 0);
        let mut absent = PythonBinding::unbound();
        absent.select_branch(join, 1);
        let mut uncertain =
            PythonBinding::unknown(&PythonUnknownCause::UnsupportedExpression, origin(2));
        uncertain.select_branch(join, 1);
        let member = present.join(absent, join).join(uncertain, join);

        let mut objects = PythonModuleEffects::default();
        let fallback = ChildImportFallback::new(&member, None)
            .expect("the conditional member has a feasible fallback path");
        objects.attach_child_for_import_fallback(
            parent.clone(),
            "child".to_string(),
            &child,
            &fallback,
            origin(3),
        );

        let binding = objects
            .read_child(&parent, "child")
            .expect("expected module child binding should exist");
        let mut module_constraints = None;
        let mut unknown_constraints = None;
        let mut unbound_constraints = None;
        for (state, constraints) in binding.alternatives_with_constraints() {
            match state {
                PythonBindingState::Bound(bound)
                    if matches!(bound.value.kind, PythonValueKind::Module(_)) =>
                {
                    module_constraints = Some(constraints.clone());
                }
                PythonBindingState::Bound(bound) if bound.value.unknown_value().is_some() => {
                    unknown_constraints = Some(constraints.clone());
                }
                PythonBindingState::Unbound => {
                    unbound_constraints = Some(constraints.clone());
                }
                PythonBindingState::Bound(_) => {}
            }
        }
        assert_eq!(module_constraints, Some(absent_constraints.clone()));
        assert_eq!(unknown_constraints, Some(absent_constraints));
        assert_eq!(unbound_constraints, Some(present_constraints));
    }

    #[test]
    fn sequential_attachment_replaces_the_coordinate() {
        let parent = source("pkg", 1);
        let first = source("pkg.a", 2);
        let second = source("pkg.b", 3);
        let mut objects = PythonModuleEffects::default();

        objects.attach_child(parent.clone(), "child".to_string(), first, origin(1));
        objects.attach_child(
            parent.clone(),
            "child".to_string(),
            second.clone(),
            origin(2),
        );

        let binding = objects
            .read_child(&parent, "child")
            .expect("attached child");
        assert_eq!(attached_child(binding), Some(second));
        assert_eq!(binding.alternatives().len(), 1);
    }

    #[test]
    fn branch_without_attachment_contributes_unbound_and_join_normalizes() {
        let parent = source("pkg", 1);
        let child = source("pkg.a", 2);
        let mut attached = PythonModuleEffects::default();
        attached.attach_child(parent.clone(), "child".to_string(), child, origin(1));
        let missing = PythonModuleEffects::default();

        let joined = PythonModuleEffects::join_branches(&[attached, missing], origin(10));

        let binding = joined
            .read_child(&parent, "child")
            .expect("coordinate present");
        assert!(attached_child(binding).is_some());
        assert!(
            has_unbound(binding),
            "the branch without attachment contributes Unbound"
        );
    }

    #[test]
    fn branch_join_of_distinct_attachments_retains_both() {
        let parent = source("pkg", 1);
        let first = source("pkg.a", 2);
        let second = source("pkg.b", 3);
        let mut left = PythonModuleEffects::default();
        left.attach_child(parent.clone(), "child".to_string(), first, origin(1));
        let mut right = PythonModuleEffects::default();
        right.attach_child(parent.clone(), "child".to_string(), second, origin(2));

        let joined = PythonModuleEffects::join_branches(&[left, right], origin(10));

        let binding = joined
            .read_child(&parent, "child")
            .expect("coordinate present");
        assert_eq!(binding.alternatives().len(), 2);
    }

    #[test]
    fn zero_iteration_loop_degrades_changed_coordinate_and_keeps_baseline() {
        let parent = source("pkg", 1);
        let child = source("pkg.a", 2);
        let mut baseline = PythonModuleEffects::default();
        baseline.attach_child(
            parent.clone(),
            "child".to_string(),
            child.clone(),
            origin(1),
        );

        let mut body = baseline.clone();
        body.attach_child(
            parent.clone(),
            "child".to_string(),
            source("pkg.b", 3),
            origin(2),
        );

        baseline.degrade_loop(vec![body], origin(10));

        let binding = baseline
            .read_child(&parent, "child")
            .expect("coordinate present");
        assert!(
            has_unsupported_unknown(binding),
            "changed coordinate degrades"
        );
    }

    #[test]
    fn loop_preserves_unrelated_coordinate() {
        let parent = source("pkg", 1);
        let stable = source("pkg.a", 2);
        let mut baseline = PythonModuleEffects::default();
        baseline.attach_child(
            parent.clone(),
            "stable".to_string(),
            stable.clone(),
            origin(1),
        );
        let body = baseline.clone();

        baseline.degrade_loop(vec![body], origin(10));

        let binding = baseline
            .read_child(&parent, "stable")
            .expect("coordinate present");
        assert_eq!(attached_child(binding), Some(stable));
        assert!(!has_unsupported_unknown(binding));
    }

    #[test]
    fn cycle_widening_keeps_equal_coordinate_and_absorbs_changed() {
        let parent = source("pkg", 1);
        let stable = source("pkg.a", 2);
        let mut previous = PythonModuleEffects::default();
        previous.attach_child(
            parent.clone(),
            "stable".to_string(),
            stable.clone(),
            origin(1),
        );
        previous.attach_child(
            parent.clone(),
            "changed".to_string(),
            source("pkg.b", 3),
            origin(1),
        );

        let mut computed = PythonModuleEffects::default();
        computed.attach_child(
            parent.clone(),
            "stable".to_string(),
            stable.clone(),
            origin(1),
        );
        computed.attach_child(
            parent.clone(),
            "changed".to_string(),
            source("pkg.c", 4),
            origin(2),
        );

        let widened = computed.widen(&previous);

        let stable_binding = widened
            .read_child(&parent, "stable")
            .expect("stable present");
        assert_eq!(attached_child(stable_binding), Some(stable));
        let changed_binding = widened
            .read_child(&parent, "changed")
            .expect("changed present");
        assert!(
            has_cycle_unknown(changed_binding),
            "changed coordinate widens to Cycle"
        );
    }

    #[test]
    fn merge_absent_incoming_key_is_a_no_op() {
        let parent = source("pkg", 1);
        let child = source("pkg.a", 2);
        let mut prior = PythonModuleEffects::default();
        prior.attach_child(
            parent.clone(),
            "child".to_string(),
            child.clone(),
            origin(1),
        );

        prior.merge(PythonModuleEffects::default(), origin(10));

        let binding = prior.read_child(&parent, "child").expect("preserved");
        assert_eq!(attached_child(binding), Some(child));
    }

    #[test]
    fn merge_incoming_unbound_preserves_prior() {
        let parent = source("pkg", 1);
        let child = source("pkg.a", 2);
        let mut prior = PythonModuleEffects::default();
        prior.attach_child(
            parent.clone(),
            "child".to_string(),
            child.clone(),
            origin(1),
        );

        let mut incoming = PythonModuleEffects::default();
        incoming.children.push(ModuleChildCoordinate {
            object: parent.clone(),
            attribute: "child".to_string(),
            binding: PythonBinding::unbound(),
        });

        prior.merge(incoming, origin(10));

        let binding = prior.read_child(&parent, "child").expect("preserved");
        assert_eq!(attached_child(binding), Some(child));
    }

    #[test]
    fn merge_exact_attachment_assigns() {
        let parent = source("pkg", 1);
        let first = source("pkg.a", 2);
        let second = source("pkg.b", 3);
        let mut prior = PythonModuleEffects::default();
        prior.attach_child(parent.clone(), "child".to_string(), first, origin(1));

        let mut incoming = PythonModuleEffects::default();
        incoming.attach_child(
            parent.clone(),
            "child".to_string(),
            second.clone(),
            origin(2),
        );

        prior.merge(incoming, origin(10));

        let binding = prior.read_child(&parent, "child").expect("assigned");
        assert_eq!(attached_child(binding), Some(second));
        assert_eq!(binding.alternatives().len(), 1);
    }

    #[test]
    fn merge_unknown_incoming_joins_prior_conservatively() {
        let parent = source("pkg", 1);
        let child = source("pkg.a", 2);
        let mut prior = PythonModuleEffects::default();
        prior.attach_child(parent.clone(), "child".to_string(), child, origin(1));

        let mut incoming = PythonModuleEffects::default();
        incoming.children.push(ModuleChildCoordinate {
            object: parent.clone(),
            attribute: "child".to_string(),
            binding: PythonBinding::unknown(&PythonUnknownCause::Cycle, origin(2)),
        });

        prior.merge(incoming, origin(10));

        let binding = prior.read_child(&parent, "child").expect("joined");
        assert!(attached_child(binding).is_some());
        assert!(has_cycle_unknown(binding));
        assert_eq!(binding.alternatives().len(), 2);
    }

    #[test]
    fn open_causes_dedupe_and_stay_object_scoped() {
        let first = source("pkg", 1);
        let second = source("other", 2);
        let mut objects = PythonModuleEffects::default();
        objects.open_cause(first.clone(), cause(1));
        objects.open_cause(first.clone(), cause(1));
        objects.open_cause(second.clone(), cause(3));

        assert_eq!(objects.causes_for(&first).count(), 1);
        assert_eq!(objects.causes_for(&second).count(), 1);
    }

    #[test]
    fn cycle_widening_replaces_changed_cause_set_with_one_cycle_cause() {
        let object = source("pkg", 1);
        let mut previous = PythonModuleEffects::default();
        previous.open_cause(object.clone(), cause(1));
        let mut computed = PythonModuleEffects::default();
        computed.open_cause(object.clone(), cause(2));

        let widened = computed.widen(&previous);

        let causes: Vec<_> = widened.causes_for(&object).collect();
        assert_eq!(causes.len(), 1);
        assert_eq!(causes[0].unknown.cause, PythonUnknownCause::Cycle);
        assert!(causes[0].unknown.origins().next().is_none());
    }

    fn module_ids(binding: &PythonBinding) -> Vec<PythonModule> {
        binding
            .alternatives()
            .filter_map(|state| {
                let PythonBindingState::Bound(bound) = state else {
                    return None;
                };
                let PythonValueKind::Module(id) = &bound.value.kind else {
                    return None;
                };
                Some(id.clone())
            })
            .collect()
    }

    #[test]
    fn merge_distributes_mixed_exact_and_unknown_incoming() {
        let parent = source("pkg", 1);
        let prior_child = source("pkg.a", 2);
        let assigned = source("pkg.b", 3);
        let mut prior = PythonModuleEffects::default();
        prior.attach_child(
            parent.clone(),
            "child".to_string(),
            prior_child.clone(),
            origin(1),
        );

        // Incoming mixes an exact module attachment with an unknown alternative.
        let incoming_binding =
            PythonBinding::bound(PythonValue::module(assigned.clone(), origin(2)), origin(2)).join(
                PythonBinding::unknown(&PythonUnknownCause::Cycle, origin(3)),
                origin(3),
            );
        let mut incoming = PythonModuleEffects::default();
        incoming.children.push(ModuleChildCoordinate {
            object: parent.clone(),
            attribute: "child".to_string(),
            binding: incoming_binding,
        });

        prior.merge(incoming, origin(10));

        let binding = prior
            .read_child(&parent, "child")
            .expect("merged coordinate");
        let ids = module_ids(binding);
        assert!(
            ids.contains(&assigned),
            "the exact module attachment assigns",
        );
        assert!(
            ids.contains(&prior_child),
            "the unknown case conservatively preserves the prior child",
        );
        assert!(
            has_cycle_unknown(binding),
            "the unknown case joins onto the prior",
        );
    }

    #[test]
    fn apply_open_causes_joins_unknown_onto_residual_unbound() {
        let object = source("pkg", 1);
        let mut objects = PythonModuleEffects::default();
        objects.open_cause(object.clone(), cause(1));

        let binding = objects.apply_open_causes(&object, PythonBinding::unbound(), origin(10));

        assert!(has_unbound(&binding), "residual Unbound is retained");
        assert!(
            has_unsupported_unknown(&binding),
            "the object-scoped open cause is joined as an unknown",
        );
    }

    #[test]
    fn apply_open_causes_leaves_fully_bound_binding_untouched() {
        let object = source("pkg", 1);
        let member = source("pkg.a", 2);
        let mut objects = PythonModuleEffects::default();
        objects.open_cause(object.clone(), cause(1));

        let bound = PythonBinding::bound(PythonValue::module(member, origin(1)), origin(1));
        let result = objects.apply_open_causes(&object, bound.clone(), origin(10));

        assert_eq!(
            result, bound,
            "no residual absence means no open-cause unknown is added",
        );
    }

    #[test]
    fn open_causes_preserve_first_seen_order_without_coalescing() {
        let object = source("pkg", 1);
        let mut objects = PythonModuleEffects::default();
        objects.open_cause(object.clone(), cause(5));
        objects.open_cause(object.clone(), cause(1));
        // An exact duplicate of the first cause is deduped.
        objects.open_cause(object.clone(), cause(5));

        let origins: Vec<u32> = objects
            .causes_for(&object)
            .flat_map(|cause| cause.unknown.origins().map(|origin| origin.span.start()))
            .collect();
        assert_eq!(
            origins,
            vec![5, 1],
            "same-kind distinct-origin causes stay separate in first-seen order",
        );
    }

    #[test]
    fn child_binding_rebases_cycle_evidence_to_the_read_origin() {
        let parent = source("pkg", 1);
        let mut objects = PythonModuleEffects::default();
        objects.children.push(ModuleChildCoordinate {
            object: parent.clone(),
            attribute: "child".to_string(),
            binding: PythonBinding::unknown(&PythonUnknownCause::Cycle, origin(1)),
        });

        let read_origin = origin(42);
        let mut binding = objects.child_binding(&parent, "child", read_origin);
        binding.rebase_cycle_unknowns(read_origin);

        assert!(has_cycle_unknown(&binding));
        let bound = binding
            .alternatives()
            .next()
            .and_then(|alternative| match alternative {
                PythonBindingState::Bound(bound) => Some(bound),
                PythonBindingState::Unbound => None,
            })
            .expect("a cycle unknown is bound");
        assert!(
            bound.value.origins().all(|origin| origin == read_origin),
            "cycle evidence is rebased to the read origin",
        );
    }

    #[test]
    fn namespace_and_source_objects_have_a_total_order() {
        let source = source("pkg", 1);
        let namespace = namespace("pkg");
        assert_eq!(source.structural_cmp(&source), Ordering::Equal);
        assert_ne!(source.structural_cmp(&namespace), Ordering::Equal);
        assert_eq!(
            source.structural_cmp(&namespace),
            namespace.structural_cmp(&source).reverse()
        );
    }

    #[test]
    fn namespace_identity_differs_by_ordered_portions() {
        let portion = |root: &str, dir: &str| {
            NamespacePortion::new(
                SearchPath::FirstParty(Utf8PathBuf::from(root)),
                Utf8PathBuf::from(dir),
            )
        };
        let package = |portions: Vec<NamespacePortion>| {
            PythonModule::Namespace(PythonNamespacePackage::new(
                PythonModuleName::parse("ns").expect("test Python module name should be valid"),
                portions,
            ))
        };

        let first_then_site = package(vec![
            portion("/project", "/project/ns"),
            portion("/site", "/site/ns"),
        ]);
        let site_then_first = package(vec![
            portion("/site", "/site/ns"),
            portion("/project", "/project/ns"),
        ]);
        let only_first = package(vec![portion("/project", "/project/ns")]);
        let identical = package(vec![
            portion("/project", "/project/ns"),
            portion("/site", "/site/ns"),
        ]);

        // Same name, same portions in the same order: one identity.
        assert_eq!(first_then_site.structural_cmp(&identical), Ordering::Equal,);
        // Reordered portions are a different search-path winner: distinct identity.
        assert_ne!(
            first_then_site.structural_cmp(&site_then_first),
            Ordering::Equal,
        );
        // A missing portion is a distinct identity that recompares by length first.
        assert_ne!(first_then_site.structural_cmp(&only_first), Ordering::Equal,);
        assert_eq!(
            first_then_site.structural_cmp(&site_then_first),
            site_then_first.structural_cmp(&first_then_site).reverse(),
        );
    }
}
