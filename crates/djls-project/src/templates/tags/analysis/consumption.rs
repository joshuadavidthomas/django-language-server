//! Joins of recorded Argument Consumption and its projection onto argument atoms.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use crate::templates::tags::types::ArgumentConsumption;
use crate::templates::tags::types::ConsumedTagBits;
use crate::templates::tags::types::SplitPosition;
use crate::templates::tags::types::TagBitSpan;

/// The argument counts an atom's position must hold for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArgumentLayout {
    Exact(usize),
    AtLeast(usize),
}

/// The consumption that holds on both of two alternative paths.
///
/// Spans recorded on different paths can differ, as when one path drops a
/// trailing bit before compiling the rest. Wherever two spans overlap for
/// every argument count, both paths consumed the overlap.
pub(crate) fn join_consumed(
    left: &BTreeMap<TagBitSpan, ArgumentConsumption>,
    right: &BTreeMap<TagBitSpan, ArgumentConsumption>,
) -> BTreeMap<TagBitSpan, ArgumentConsumption> {
    let mut joined: BTreeMap<TagBitSpan, ArgumentConsumption> = BTreeMap::new();
    for (left_bits, left_consumption) in left {
        for (right_bits, right_consumption) in right {
            let consumption = left_consumption.join(*right_consumption);
            if consumption.is_unknown() {
                continue;
            }
            if let Some(bits) = overlap(*left_bits, *right_bits) {
                let entry = joined.entry(bits).or_insert(consumption);
                *entry = entry.meet(consumption);
            }
        }
    }
    let redundant = joined
        .iter()
        .filter(|(bits, consumption)| {
            joined.iter().any(|(other, other_consumption)| {
                other != *bits && other_consumption == *consumption && contains(*other, **bits)
            })
        })
        .map(|(bits, _)| *bits)
        .collect::<Vec<_>>();
    for bits in redundant {
        joined.remove(&bits);
    }
    joined
}

/// The bits both spans include at every argument count, when that is a span.
fn overlap(left: TagBitSpan, right: TagBitSpan) -> Option<TagBitSpan> {
    let first = match compare(left.first, right.first)? {
        Ordering::Less | Ordering::Equal => right.first,
        Ordering::Greater => left.first,
    };
    let last = match compare(left.last, right.last)? {
        Ordering::Less | Ordering::Equal => left.last,
        Ordering::Greater => right.last,
    };
    // A span whose ends compare the wrong way is empty at every count.
    (compare(first, last) != Some(Ordering::Greater)).then_some(TagBitSpan { first, last })
}

/// Whether `outer` includes every bit of `inner` at every argument count.
fn contains(outer: TagBitSpan, inner: TagBitSpan) -> bool {
    matches!(
        compare(outer.first, inner.first),
        Some(Ordering::Less | Ordering::Equal)
    ) && matches!(
        compare(inner.last, outer.last),
        Some(Ordering::Less | Ordering::Equal)
    )
}

/// Order two positions when the order is the same at every argument count.
fn compare(left: SplitPosition, right: SplitPosition) -> Option<Ordering> {
    match (left, right) {
        (SplitPosition::Forward(left), SplitPosition::Forward(right)) => Some(left.cmp(&right)),
        (SplitPosition::Backward(left), SplitPosition::Backward(right)) => Some(right.cmp(&left)),
        (SplitPosition::Forward(_), SplitPosition::Backward(_))
        | (SplitPosition::Backward(_), SplitPosition::Forward(_)) => None,
    }
}

/// The consumption of every recorded span that covers `target` at each
/// argument count the layout allows. Covering uses all apply on one path.
pub(crate) fn consumption_covering(
    consumed: impl IntoIterator<Item = ConsumedTagBits>,
    target: TagBitSpan,
    layout: ArgumentLayout,
) -> ArgumentConsumption {
    consumed
        .into_iter()
        .filter(|consumed| {
            precedes(consumed.bits.first, target.first, layout)
                && precedes(target.last, consumed.bits.last, layout)
        })
        .fold(ArgumentConsumption::Unknown, |covering, consumed| {
            covering.meet(consumed.consumption)
        })
}

/// Whether `left` is at or before `right` for every allowed argument count.
fn precedes(left: SplitPosition, right: SplitPosition, layout: ArgumentLayout) -> bool {
    let index = |position: SplitPosition, arguments: usize| match position {
        SplitPosition::Forward(index) => isize::try_from(index).ok().map(|index| index - 1),
        SplitPosition::Backward(distance) => isize::try_from(arguments)
            .ok()
            .zip(isize::try_from(distance).ok())
            .map(|(arguments, distance)| arguments - distance),
    };
    match (left, right, layout) {
        (_, _, ArgumentLayout::Exact(arguments)) => index(left, arguments)
            .zip(index(right, arguments))
            .is_some_and(|(left, right)| left <= right),
        (
            SplitPosition::Forward(left),
            SplitPosition::Forward(right),
            ArgumentLayout::AtLeast(_),
        ) => left <= right,
        (
            SplitPosition::Backward(left),
            SplitPosition::Backward(right),
            ArgumentLayout::AtLeast(_),
        ) => left >= right,
        // A position from the end moves away as more arguments appear, so
        // the fewest arguments decide.
        (
            SplitPosition::Forward(_),
            SplitPosition::Backward(_),
            ArgumentLayout::AtLeast(minimum),
        ) => index(left, minimum)
            .zip(index(right, minimum))
            .is_some_and(|(left, right)| left <= right),
        (SplitPosition::Backward(_), SplitPosition::Forward(_), ArgumentLayout::AtLeast(_)) => {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn consumed(first: SplitPosition, last: SplitPosition) -> ConsumedTagBits {
        ConsumedTagBits {
            bits: TagBitSpan { first, last },
            consumption: ArgumentConsumption::Expression,
        }
    }

    #[test]
    fn open_slice_covers_atoms_only_where_every_count_reaches_them() {
        // `[compile_filter(bit) for bit in bits[2:-1]]`
        let slice = consumed(SplitPosition::Forward(2), SplitPosition::Backward(2));
        let covers = |target, layout| {
            consumption_covering([slice], TagBitSpan::single(target), layout)
                == ArgumentConsumption::Expression
        };

        assert!(!covers(
            SplitPosition::Forward(1),
            ArgumentLayout::AtLeast(4)
        ));
        assert!(covers(
            SplitPosition::Forward(2),
            ArgumentLayout::AtLeast(4)
        ));
        assert!(covers(
            SplitPosition::Forward(3),
            ArgumentLayout::AtLeast(4)
        ));
        // With four arguments, the fourth is the excluded last bit.
        assert!(!covers(
            SplitPosition::Forward(4),
            ArgumentLayout::AtLeast(4)
        ));
        assert!(covers(SplitPosition::Forward(4), ArgumentLayout::Exact(5)));
        assert!(covers(
            SplitPosition::Backward(2),
            ArgumentLayout::AtLeast(4)
        ));
        assert!(!covers(
            SplitPosition::Backward(1),
            ArgumentLayout::AtLeast(4)
        ));
    }

    #[test]
    fn paths_that_drop_a_trailing_bit_still_share_the_rest() {
        // `{% cache %}`: one path drops a trailing `using=` bit before
        // compiling `tokens[3:]`, the other compiles every trailing bit.
        let dropped = BTreeMap::from([(
            TagBitSpan {
                first: SplitPosition::Forward(3),
                last: SplitPosition::Backward(2),
            },
            ArgumentConsumption::Expression,
        )]);
        let kept = BTreeMap::from([
            (
                TagBitSpan::single(SplitPosition::Forward(1)),
                ArgumentConsumption::Expression,
            ),
            (
                TagBitSpan {
                    first: SplitPosition::Forward(3),
                    last: SplitPosition::Backward(1),
                },
                ArgumentConsumption::Expression,
            ),
        ]);

        assert_eq!(
            join_consumed(&dropped, &kept),
            BTreeMap::from([(
                TagBitSpan {
                    first: SplitPosition::Forward(3),
                    last: SplitPosition::Backward(2),
                },
                ArgumentConsumption::Expression,
            )])
        );
    }

    #[test]
    fn spans_without_a_fixed_order_do_not_join() {
        let front = BTreeMap::from([(
            TagBitSpan::single(SplitPosition::Forward(2)),
            ArgumentConsumption::Expression,
        )]);
        let back = BTreeMap::from([(
            TagBitSpan::single(SplitPosition::Backward(1)),
            ArgumentConsumption::Expression,
        )]);

        // The second bit is the last one only when there are two arguments.
        assert!(join_consumed(&front, &back).is_empty());
    }

    #[test]
    fn covering_uses_on_one_path_keep_the_stricter_consumption() {
        let keyword = ConsumedTagBits {
            bits: TagBitSpan {
                first: SplitPosition::Forward(1),
                last: SplitPosition::Backward(1),
            },
            consumption: ArgumentConsumption::OptionalKeywordExpression,
        };
        let single = consumed(SplitPosition::Forward(1), SplitPosition::Forward(1));

        assert_eq!(
            consumption_covering(
                [keyword, single],
                TagBitSpan::single(SplitPosition::Forward(1)),
                ArgumentLayout::AtLeast(1),
            ),
            ArgumentConsumption::Expression
        );
        assert_eq!(
            consumption_covering(
                [keyword, single],
                TagBitSpan::single(SplitPosition::Forward(2)),
                ArgumentLayout::AtLeast(2),
            ),
            ArgumentConsumption::OptionalKeywordExpression
        );
    }
}
