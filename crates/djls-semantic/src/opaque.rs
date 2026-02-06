//! Opaque region handling — skip validation inside `{% verbatim %}` etc.
//!
//! Opaque blocks are tags whose content should not be parsed or validated
//! as Django template syntax (e.g., `{% verbatim %}...{% endverbatim %}`).
//! The `OpaqueRegions` struct identifies these spans so that downstream
//! validation passes can skip nodes inside them.

use djls_source::Span;
use djls_templates::Node;

use crate::Db;

/// Spans that should skip validation (inside opaque blocks).
#[derive(Debug, Clone, Default)]
pub struct OpaqueRegions {
    regions: Vec<Span>,
}

impl OpaqueRegions {
    /// Check if a span falls inside an opaque region.
    ///
    /// A span is considered opaque if it is fully contained within
    /// any opaque region (i.e., region.start <= span.start AND
    /// span.end <= region.end).
    #[must_use]
    pub fn is_opaque(&self, span: Span) -> bool {
        self.regions
            .iter()
            .any(|r| r.start() <= span.start() && span.end() <= r.end())
    }
}

/// Build opaque regions from a nodelist using the opaque tag map from Db.
///
/// Scans the flat nodelist for opener→closer pairs (e.g., `verbatim`→`endverbatim`).
/// The region between the end of the opener and the start of the closer is marked
/// as opaque — all nodes with spans inside that region will be skipped by validation.
///
/// If an opener has no matching closer, it is silently ignored (the block structure
/// validator will catch `UnclosedTag` separately).
pub fn compute_opaque_regions(
    db: &dyn Db,
    nodelist: djls_templates::NodeList<'_>,
) -> OpaqueRegions {
    let opaque_tags = db.opaque_tag_map();

    if opaque_tags.is_empty() {
        return OpaqueRegions::default();
    }

    let nodes = nodelist.nodelist(db);
    let mut regions = Vec::new();
    let mut i = 0;

    while i < nodes.len() {
        if let Node::Tag { name, span, .. } = &nodes[i] {
            if let Some(closer) = opaque_tags.get(name.as_str()) {
                let open_end = span.end();

                // Search forward for the matching closer
                let mut j = i + 1;
                while j < nodes.len() {
                    if let Node::Tag {
                        name: close_name,
                        span: close_span,
                        ..
                    } = &nodes[j]
                    {
                        if close_name == closer {
                            let region_start = open_end;
                            let region_end = close_span.start();
                            if region_end > region_start {
                                regions.push(Span::new(region_start, region_end - region_start));
                            }
                            i = j;
                            break;
                        }
                    }
                    j += 1;
                }
            }
        }
        i += 1;
    }

    OpaqueRegions { regions }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_regions_never_opaque() {
        let regions = OpaqueRegions::default();
        assert!(!regions.is_opaque(Span::new(0, 10)));
        assert!(!regions.is_opaque(Span::new(50, 5)));
    }

    #[test]
    fn span_inside_region_is_opaque() {
        let regions = OpaqueRegions {
            regions: vec![Span::new(10, 20)], // region [10, 30)
        };
        // Fully contained
        assert!(regions.is_opaque(Span::new(15, 5))); // [15, 20) inside [10, 30)
        assert!(regions.is_opaque(Span::new(10, 5))); // [10, 15) starts at boundary
        assert!(regions.is_opaque(Span::new(25, 5))); // [25, 30) ends at boundary
    }

    #[test]
    fn span_outside_region_not_opaque() {
        let regions = OpaqueRegions {
            regions: vec![Span::new(10, 20)], // region [10, 30)
        };
        assert!(!regions.is_opaque(Span::new(0, 5))); // before
        assert!(!regions.is_opaque(Span::new(35, 5))); // after
    }

    #[test]
    fn span_partially_overlapping_not_opaque() {
        let regions = OpaqueRegions {
            regions: vec![Span::new(10, 20)], // region [10, 30)
        };
        // Starts before, ends inside — NOT fully contained
        assert!(!regions.is_opaque(Span::new(5, 10))); // [5, 15) — starts before 10
                                                       // Starts inside, ends after — NOT fully contained
        assert!(!regions.is_opaque(Span::new(25, 10))); // [25, 35) — ends after 30
    }

    #[test]
    fn multiple_regions() {
        let regions = OpaqueRegions {
            regions: vec![
                Span::new(10, 10), // [10, 20)
                Span::new(50, 10), // [50, 60)
            ],
        };
        assert!(regions.is_opaque(Span::new(12, 3))); // in first region
        assert!(regions.is_opaque(Span::new(52, 3))); // in second region
        assert!(!regions.is_opaque(Span::new(30, 5))); // between regions
    }

    #[test]
    fn nested_regions() {
        // Two adjacent opaque blocks with gap between
        let regions = OpaqueRegions {
            regions: vec![
                Span::new(10, 20), // [10, 30)
                Span::new(40, 20), // [40, 60)
            ],
        };
        assert!(regions.is_opaque(Span::new(15, 5))); // inside first
        assert!(regions.is_opaque(Span::new(45, 5))); // inside second
        assert!(!regions.is_opaque(Span::new(32, 5))); // between the two
    }

    #[test]
    fn zero_length_region_not_opaque() {
        // Edge case: region with zero length
        let regions = OpaqueRegions {
            regions: vec![Span::new(10, 0)], // [10, 10) — empty
        };
        // A span of length 0 at position 10 is [10, 10) — contained in [10, 10)
        assert!(regions.is_opaque(Span::new(10, 0)));
        // But a span with actual content at that point is not
        assert!(!regions.is_opaque(Span::new(10, 1)));
    }

    #[test]
    fn exact_boundary_match() {
        let regions = OpaqueRegions {
            regions: vec![Span::new(10, 20)], // [10, 30)
        };
        // Span exactly matching the region boundaries
        assert!(regions.is_opaque(Span::new(10, 20))); // [10, 30) == region
    }
}
