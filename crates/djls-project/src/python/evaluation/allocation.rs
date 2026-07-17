use djls_source::Origin;

use super::BranchConstraints;
use super::origin_sort_key;

/// An abstract source identity for a freshly allocated mutable object.
///
/// Allocation sites are abstract source identities, not runtime object IDs and
/// not diagnostic provenance. They preserve the branch constraints under which
/// the allocation is reachable so a later precision pass can use them.
#[derive(Debug, Clone, PartialEq, Eq)]
struct AllocationSite {
    origin: Origin,
    constraints: BranchConstraints,
}

/// The non-empty, canonical set of allocation sites owned by a concrete mutable
/// value (a list or a dictionary). Every list and dictionary owns at least one
/// site; tuples own none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AllocationSites(Vec<AllocationSite>);

impl AllocationSites {
    pub(super) fn one(origin: Origin) -> Self {
        Self(vec![AllocationSite {
            origin,
            constraints: BranchConstraints::unconstrained(),
        }])
    }

    /// Replace every site with a single canonical site at `origin`, used when a
    /// binary list concatenation allocates a fresh list at the operation
    /// expression.
    pub(super) fn rebase(&mut self, origin: Origin) {
        self.0.clear();
        self.0.push(AllocationSite {
            origin,
            constraints: BranchConstraints::unconstrained(),
        });
    }

    /// Union two site sets, coalescing equal origins by merging their branch
    /// constraints. Used when equal mutable values merge on a branch join.
    pub(super) fn merge(&mut self, incoming: Self) {
        self.0.extend(incoming.0);
        self.normalize();
    }

    pub(super) fn constrain(&mut self, constraints: &BranchConstraints) {
        for site in &mut self.0 {
            site.constraints = site.constraints.intersection(constraints);
        }
        self.normalize();
    }

    pub(super) fn origins(&self) -> impl Iterator<Item = Origin> + '_ {
        self.0.iter().map(|site| site.origin)
    }

    /// Whether two site groups can name the same runtime object. Alias matching
    /// is conservatively origin-based: two allocation sites at the same source
    /// origin are treated as the same object even under conflicting branch
    /// constraints. The constraints are preserved in the data for a later
    /// precision pass, but this plan does not use them to distinguish
    /// allocations, keeping alias invalidation conservative.
    fn shares_origin(&self, other: &Self) -> bool {
        self.0
            .iter()
            .any(|left| other.0.iter().any(|right| left.origin == right.origin))
    }

    fn normalize(&mut self) {
        self.0.sort_by_key(|site| {
            (
                origin_sort_key(&site.origin),
                format!("{:?}", site.constraints),
            )
        });
        let mut normalized: Vec<AllocationSite> = Vec::with_capacity(self.0.len());
        for site in std::mem::take(&mut self.0) {
            if let Some(existing) = normalized
                .iter_mut()
                .find(|existing| existing.origin == site.origin)
            {
                existing.constraints.merge(site.constraints);
            } else {
                normalized.push(site);
            }
        }
        self.0 = normalized;
        debug_assert!(
            !self.0.is_empty(),
            "allocation sites remain non-empty for every list and dictionary"
        );
    }
}

/// A possibly-empty, occurrence-preserving projection of the constrained
/// allocation-site groups reachable from a value or binding. Each reachable
/// mutable object contributes one [`AllocationSites`] group, so repeated
/// occurrences of the same object are retained rather than collapsed into a
/// membership set. Distinct from [`AllocationSites`]: it carries no non-empty
/// invariant and is a transient artifact of recursive alias discovery.
#[derive(Debug, Default)]
pub(crate) struct ReachableAllocationSites(Vec<AllocationSites>);

impl ReachableAllocationSites {
    /// Record one reachable mutable object's constrained sites, preserving the
    /// occurrence even when an equal group is already present.
    pub(super) fn push_group(&mut self, group: AllocationSites) {
        self.0.push(group);
    }

    /// Absorb every reachable group discovered elsewhere, preserving occurrences.
    pub(super) fn absorb(&mut self, other: Self) {
        self.0.extend(other.0);
    }

    pub(super) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Whether any reachable group here shares an allocation origin with any
    /// group in `other`. Branch constraints remain stored but deliberately do
    /// not narrow this plan's conservative alias matching.
    pub(super) fn intersects(&self, other: &Self) -> bool {
        self.0.iter().any(|group| other.intersects_group(group))
    }

    /// Whether any reachable group here shares an allocation origin with the
    /// given site group, regardless of branch constraints.
    pub(super) fn intersects_group(&self, group: &AllocationSites) -> bool {
        self.0.iter().any(|existing| existing.shares_origin(group))
    }
}

#[cfg(test)]
mod tests {
    use djls_source::File;
    use djls_source::Span;
    use salsa::plumbing::FromId;
    use salsa::plumbing::Id;

    use super::AllocationSites;
    use super::BranchConstraints;
    use super::Origin;
    use super::ReachableAllocationSites;

    fn origin(offset: usize) -> Origin {
        let file = File::from_id(Id::from_bits(1));
        Origin::new(file, Span::saturating_from_parts_usize(offset, 1))
    }

    fn selected(join: Origin, arm: usize) -> BranchConstraints {
        let mut constraints = BranchConstraints::unconstrained();
        constraints.select(join, arm);
        constraints
    }

    fn constrained_site(site: Origin, join: Origin, arm: usize) -> AllocationSites {
        let mut sites = AllocationSites::one(site);
        sites.constrain(&selected(join, arm));
        sites
    }

    #[test]
    fn a_single_site_is_non_empty_and_unconstrained() {
        let sites = AllocationSites::one(origin(1));
        assert_eq!(sites.origins().collect::<Vec<_>>(), vec![origin(1)]);
    }

    #[test]
    fn merge_coalesces_equal_origins_and_sorts_distinct_ones_canonically() {
        let mut sites = AllocationSites::one(origin(2));
        sites.merge(AllocationSites::one(origin(2)));
        assert_eq!(
            sites.origins().collect::<Vec<_>>(),
            vec![origin(2)],
            "equal origins coalesce into one site"
        );

        sites.merge(AllocationSites::one(origin(1)));
        assert_eq!(
            sites.origins().collect::<Vec<_>>(),
            vec![origin(1), origin(2)],
            "distinct origins are retained in canonical order"
        );
    }

    #[test]
    fn merge_coalesces_equal_origins_by_unioning_branch_constraints() {
        let join = origin(9);
        let mut sites = constrained_site(origin(1), join, 0);
        sites.merge(constrained_site(origin(1), join, 1));

        assert_eq!(
            sites.origins().collect::<Vec<_>>(),
            vec![origin(1)],
            "equal origins coalesce even under different branch arms"
        );
        // Constraints are preserved in the data, but origin-based matching means
        // the coalesced site shares an origin with a probe under either arm.
        assert!(sites.shares_origin(&constrained_site(origin(1), join, 0)));
        assert!(sites.shares_origin(&constrained_site(origin(1), join, 1)));
    }

    #[test]
    fn origin_based_matching_ignores_branch_constraints_for_this_plan() {
        let join = origin(9);
        let left = constrained_site(origin(1), join, 0);

        assert!(
            left.shares_origin(&AllocationSites::one(origin(1))),
            "an unconstrained site at the same origin shares it"
        );
        assert!(
            left.shares_origin(&constrained_site(origin(1), join, 1)),
            "same origin aliases conservatively even under conflicting branch arms"
        );
        assert!(
            !left.shares_origin(&AllocationSites::one(origin(2))),
            "different origins never share"
        );
    }

    #[test]
    fn reachable_sites_preserve_occurrences_and_intersect_by_group() {
        let mut reachable = ReachableAllocationSites::default();
        assert!(reachable.is_empty());

        reachable.push_group(AllocationSites::one(origin(1)));
        reachable.push_group(AllocationSites::one(origin(1)));
        assert!(!reachable.is_empty());

        // Both occurrences are retained: a wanted probe intersects the group.
        assert!(reachable.intersects_group(&AllocationSites::one(origin(1))));
        assert!(!reachable.intersects_group(&AllocationSites::one(origin(2))));

        let mut other = ReachableAllocationSites::default();
        other.absorb({
            let mut nested = ReachableAllocationSites::default();
            nested.push_group(AllocationSites::one(origin(2)));
            nested.push_group(AllocationSites::one(origin(1)));
            nested
        });
        assert!(
            reachable.intersects(&other),
            "shared origin makes groups alias"
        );

        let mut disjoint = ReachableAllocationSites::default();
        disjoint.push_group(AllocationSites::one(origin(3)));
        assert!(!reachable.intersects(&disjoint));
    }

    #[test]
    fn reachable_intersection_is_conservatively_origin_based() {
        let join = origin(9);
        let mut reachable = ReachableAllocationSites::default();
        reachable.push_group(constrained_site(origin(1), join, 0));

        // Same origin under a conflicting branch arm still aliases: this plan
        // keeps reachability conservatively origin-based rather than using the
        // preserved branch constraints to separate allocations.
        let mut conflicting = ReachableAllocationSites::default();
        conflicting.push_group(constrained_site(origin(1), join, 1));
        assert!(
            reachable.intersects(&conflicting),
            "same origin aliases even under mutually exclusive branches"
        );

        let mut different = ReachableAllocationSites::default();
        different.push_group(constrained_site(origin(2), join, 0));
        assert!(
            !reachable.intersects(&different),
            "a distinct origin is never an alias"
        );
    }
}
