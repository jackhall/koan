//! The keys fold over shared frontiers: what it reads off each frontier through its memo is what
//! a fold over the frontier's flattened unknowns reads.

use proptest::prelude::*;

use crate::memory::{Bump, BumpVec};
use crate::scope::tests::with_shapes;
use crate::scope::{Address, Cut, Frontier, Segment, ShapeKind, Shapes, Slot, UnitIndex, Unknown};

use super::super::folds::keyed_names;

/// Callables nested in callables, a block and a module body, so frontiers span chains of
/// several depths.
const SOURCE: &str = "LET f = (FN :{x :Number, n :Number} -> Any = #(\n  \
                      LET g = (FN :{y :Number} -> Number = #(1 < (y) <= 3))\n  \
                      MODULE m = ((LET h = (FN :{z :Number} -> Number = #(z))))\n  g\n))\n\
                      LET k = (FN :{w :Number} -> Number = #(w))\n\
                      LET z = 2\n1 < (z) <= 3";

/// `keyed_names` as it read a frontier flat: the seam the latest bind site among its unknowns,
/// the unit's own aside, and the names each keyable callable parameter among them a body around
/// the unit declares.
fn keyed_flat(
    shapes: &Shapes<'_>,
    frontiers: &[(Segment, UnitIndex, Frontier<'_>)],
    keyable: &dyn Fn(Segment, Slot) -> bool,
) -> Vec<(Segment, Slot)> {
    let mut named = Vec::new();
    for (segment, unit, frontier) in frontiers {
        if *unit == UnitIndex::ENTRY && shapes.shape_at(*segment).kind() == ShapeKind::Callable {
            continue;
        }
        let unknowns = frontier.unknowns();
        let Some(seam) = (unknowns.iter())
            .map(|unknown| shapes.bind_site(*unknown))
            .filter(|site| (site.segment, site.unit) != (*segment, *unit))
            .max()
        else {
            continue;
        };
        if seam.unit != UnitIndex::ENTRY
            || shapes.shape_at(seam.segment).kind() != ShapeKind::Callable
        {
            continue;
        }
        for unknown in unknowns {
            let Address::Slot(slot) = unknown.at else {
                continue;
            };
            if shapes.shape_at(unknown.segment).kind() == ShapeKind::Callable
                && shapes.encloses(unknown.segment, *segment)
                && keyable(unknown.segment, slot)
            {
                named.push((unknown.segment, slot));
            }
        }
    }
    named.sort_unstable();
    named.dedup();
    named
}

/// One frontier to lay: indices of its own unknowns, of the earlier frontiers it reads beside the
/// segment each is cut at, and of the unit it is the frontier of. Each index is taken modulo what
/// the shape table holds.
type Node = (Vec<u16>, Vec<(u16, u16)>, u16, u16);

fn nodes() -> impl Strategy<Value = Vec<Node>> {
    let node = (
        prop::collection::vec(any::<u16>(), 0..5),
        prop::collection::vec((any::<u16>(), any::<u16>()), 0..4),
        any::<u16>(),
        any::<u16>(),
    );
    prop::collection::vec(node, 1..16)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: crate::tests::case_share(1, 1), ..ProptestConfig::default() })]

    /// Over frontiers each reading earlier ones at arbitrary cuts, the keys fold names what a
    /// fold over each frontier's flattened unknowns names.
    #[test]
    fn the_keys_fold_reads_a_shared_frontier_as_its_flattening(nodes in nodes()) {
        with_shapes(SOURCE, |shapes, _, _, _| {
            let scratch = Bump::new();
            let bodies = shapes.bodies();
            let unknowns: Vec<Unknown> = (bodies.iter())
                .flat_map(|body| {
                    (0..body.slots()).map(|slot| Unknown {
                        segment: body.segment(),
                        at: Address::Slot(Slot(slot as u32)),
                    })
                })
                .collect();
            let pick = |at: u16, of: usize| at as usize % of;
            let mut laid: Vec<(Segment, UnitIndex, Frontier<'_>)> = Vec::new();
            for (own, reads, body, unit) in &nodes {
                let mut staged = BumpVec::new_in(&scratch);
                staged.extend(own.iter().map(|at| unknowns[pick(*at, unknowns.len())]));
                let mut cuts = BumpVec::new_in(&scratch);
                if !laid.is_empty() {
                    cuts.extend(reads.iter().map(|(read, from)| Cut {
                        frontier: laid[pick(*read, laid.len())].2,
                        from: bodies[pick(*from, bodies.len())].segment(),
                    }));
                }
                let frontier = Frontier::laid(&scratch, &mut staged, &cuts);
                let body = bodies[pick(*body, bodies.len())];
                let unit = match pick(*unit, body.units().len() + 1) {
                    0 => UnitIndex::ENTRY,
                    run => UnitIndex::of_run(run - 1),
                };
                laid.push((body.segment(), unit, frontier));
            }
            let keyable = |segment: Segment, slot: Slot| !(segment.index() + slot.0).is_multiple_of(3);
            let memoized = keyed_names(shapes, &laid, &scratch, &keyable, &[ShapeKind::Callable]);
            assert_eq!(memoized.to_vec(), keyed_flat(shapes, &laid, &keyable));
        });
    }
}
