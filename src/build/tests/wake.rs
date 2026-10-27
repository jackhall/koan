//! The wake memo: whether a binding wakes a frontier, answered once per frontier, agrees with the
//! frontier's flattened unknowns ([`Frontier::meets_binding`]) over frontier DAGs with cuts.

use proptest::prelude::*;

use crate::memory::{Bump, BumpVec, program_storage};
use crate::scope::{Address, Binding, Bound, Cut, Frontier, Segment, Slot, Unknown};
use crate::values::{Nothing, Value};

use super::super::wake::Wakes;

/// One frontier of a generated DAG: its own unknowns, as (segment, slot), and the earlier
/// frontiers it reads, each by index beside its cut's segment.
type Drawn = (Vec<(u32, u32)>, Vec<(usize, u32)>);

fn unknown((segment, slot): (u32, u32)) -> Unknown {
    Unknown {
        segment: Segment(segment),
        at: Address::Slot(Slot(slot)),
    }
}

fn drawn() -> impl Strategy<Value = (Vec<Drawn>, Vec<(u32, u32)>)> {
    let pair = (0u32..6, 0u32..3);
    let frontier = (
        prop::collection::vec(pair.clone(), 0..3),
        prop::collection::vec((any::<prop::sample::Index>(), 0u32..6), 0..3),
    );
    (
        prop::collection::vec(frontier, 1..8),
        prop::collection::vec(pair, 0..4),
    )
        .prop_map(|(frontiers, bound)| {
            // Each frontier reads only earlier ones, as an ended frontier is read.
            let frontiers = (frontiers.into_iter().enumerate())
                .map(|(at, (own, reads))| {
                    let reads = match at {
                        0 => Vec::new(),
                        _ => (reads.into_iter())
                            .map(|(index, from)| (index.index(at), from))
                            .collect(),
                    };
                    (own, reads)
                })
                .collect();
            (frontiers, bound)
        })
}

proptest! {
    #[test]
    fn the_wake_memo_wakes_where_the_flattened_frontier_meets_the_binding((frontiers, bound) in drawn()) {
        let storage = program_storage();
        let writer = storage.brand().writer();
        let scratch = Bump::new();
        let mut staged = BumpVec::new_in(&scratch);
        let (mut seen, number) = (Vec::new(), Value::<Nothing>::Number(0.0).concrete_ktype());
        for pair in bound {
            if !seen.contains(&pair) {
                seen.push(pair);
                staged.push((unknown(pair), Bound::Type(number)));
            }
        }
        let binding = Binding::laid(writer, &mut staged);
        let mut laid: Vec<Frontier<'_>> = Vec::new();
        for (own, reads) in &frontiers {
            let mut unknowns = BumpVec::new_in(&scratch);
            unknowns.extend(own.iter().map(|pair| unknown(*pair)));
            let cuts: Vec<_> = (reads.iter())
                .map(|(index, from)| Cut { frontier: laid[*index], from: Segment(*from) })
                .collect();
            laid.push(Frontier::laid(writer, &mut unknowns, &cuts));
        }
        let wakes = Wakes::new(binding, &scratch);
        // Asked newest first, so a read is answered from the memo an earlier ask filled.
        for frontier in laid.iter().rev() {
            prop_assert_eq!(wakes.wakes(*frontier), frontier.meets_binding(binding));
        }
    }
}
