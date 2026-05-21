use crate::Db;

/// Temporary Phase 1 Project Facts availability adapter.
///
/// Phase 3C deletion gate: move pure Project Facts readiness classification to
/// `djls-project::availability`. Delete this type, or narrow it to a
/// semantic-only adapter once `djls-project` owns the shared availability seam.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProjectFactsAvailability {
    Present,
    Absent { reason: ProjectFactsAbsentReason },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProjectFactsAbsentReason {
    StartupNotLoaded,
}

#[must_use]
pub fn project_facts_availability(db: &dyn Db) -> ProjectFactsAvailability {
    if db.project().is_some() {
        ProjectFactsAvailability::Present
    } else {
        ProjectFactsAvailability::Absent {
            reason: ProjectFactsAbsentReason::StartupNotLoaded,
        }
    }
}
