#![allow(dead_code)]

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum EnrichmentCacheLookup {
    Hit(djls_project::ProjectEnrichmentDraft),
    Miss(djls_project::ProjectEnrichmentIssue),
}

impl EnrichmentCacheLookup {
    pub(crate) fn stale_hit(
        hints: djls_project::ProjectEnrichmentHints,
        key: djls_project::EnrichmentCacheKey,
        age: djls_project::CacheAge,
    ) -> Self {
        Self::Hit(djls_project::ProjectEnrichmentDraft::CachedStale {
            hints,
            issue: djls_project::ProjectEnrichmentIssue::CacheStale { key, age },
        })
    }

    pub(crate) fn read_failed(kind: djls_project::CacheIssueKind) -> Self {
        Self::Miss(djls_project::ProjectEnrichmentIssue::CacheReadFailed { kind })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enrichment_cache_represents_read_failure_as_typed_issue() {
        let lookup = EnrichmentCacheLookup::read_failed(djls_project::CacheIssueKind::Io);

        assert!(matches!(
            lookup,
            EnrichmentCacheLookup::Miss(
                djls_project::ProjectEnrichmentIssue::CacheReadFailed { .. }
            )
        ));
    }

    #[test]
    fn enrichment_cache_represents_stale_hit_as_hint_draft() {
        let lookup = EnrichmentCacheLookup::stale_hit(
            djls_project::ProjectEnrichmentHints::default(),
            djls_project::EnrichmentCacheKey::new("project"),
            djls_project::CacheAge::from_seconds(60),
        );

        assert!(matches!(
            lookup,
            EnrichmentCacheLookup::Hit(djls_project::ProjectEnrichmentDraft::CachedStale { .. })
        ));
    }
}
