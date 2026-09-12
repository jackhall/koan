//! The liveness-matrix invariants, over random interleavings of the verbs. The hand-written tests
//! pin shapes; this one pins that no order of `create` / `hold` / `alloc_into` / `keep` / `redeem`
//! / `read` / `release` can break the conditions the whole model rests on
//! ([../README.md § Invariants](../README.md#invariants)):
//!
//! - a recycled slot is named by nothing — no occupant's row in either relation, and no frozen
//!   aggregate;
//! - a cell undisposed after its declared death is named by an occupant's birth row — the one
//!   relation with no sealed half to convert into — or counted by an undisposed tree child;
//! - every sealed cell's holder count equals the number of hold sets that name it, and the reverse
//!   naming index is exactly the transpose of the aggregates;
//! - every bit and id of a dormant carrier's mask is covered by storage its cell is answerable
//!   for — mask validity, over the whole reach table rather than one stored continuation;
//! - the relocation map and the lineages agree in both directions, and every relocated key names
//!   an entry that exists — so a dormant carrier forwarded through any number of merges still
//!   redeems to the value it was kept as, which the redeem verb reads back and checks;
//! - no hold set names its own owner, and every present sealed cell has a holder — which together
//!   make a sealed cell that survives a wound-down run a ring by arithmetic, with no ring walk in
//!   the loop;
//! - the live tier never grows across a release, so storage that has sealed never re-enters it;
//! - a sealed cell that survives a wound-down run was named by two hold sets at some point — the
//!   universal the hand-written ring-dissolution tests are three instances of;
//! - every memoized closure still equals the walk that would recompute it, and no memo exists
//!   unless a price query put it there — the never-invalidated memo carried across every
//!   interleaving, and the substrate's own paths pricing nothing;
//! - and, over the tree pool ([../../tree/README.md](../../tree/README.md)): no tombstone names
//!   storage that is gone, every tombstone is on exactly one lineage list, every parent's child
//!   count equals the children that still name it, every pledge is an ancestor and every
//!   intermediate below a pledged cell is pledged at least as shallow, and a value kept in a tree
//!   cell redeems from an entitled cell to the number it was kept as.

use proptest::prelude::*;

use super::super::*;
use super::{Borrowed, Number, live_bytes, number_here, one, operand_at, pin, take};
use crate::tree::{Ancestor, TreeState};

const CAP: u32 = 6;

/// A verdict that reaches both arms across a run: two pins, then a copy. Deterministic, so a
/// shrunk failure replays exactly — and the invariants have to hold under either answer, since a
/// copy mints no hold where a pin would have.
fn alternating() -> impl FnMut(Prices) -> Verdict + 'static {
    let mut seen = 0u32;
    move |_| {
        seen += 1;
        if seen.is_multiple_of(3) {
            Verdict::Copy
        } else {
            Verdict::Pin
        }
    }
}

/// One verb, with its operands as indices into the handles minted so far — so a generated run
/// names cells that may since have died, which is the point: a stale operand must be refused, not
/// mis-applied.
#[derive(Clone, Debug)]
enum Verb {
    Create {
        parent: Option<usize>,
    },
    CreateTree {
        parent: usize,
        under_tree: bool,
    },
    PlaceFromTree {
        producer: usize,
        consumer: usize,
        into_tree: bool,
    },
    KeepTree {
        cell: usize,
    },
    RedeemInTree {
        cell: usize,
        index: usize,
    },
    ReleaseTree {
        cell: usize,
    },
    Hold {
        holder: usize,
        held: usize,
    },
    Place {
        producer: usize,
        consumer: usize,
    },
    Continue {
        cell: usize,
        over: usize,
    },
    Keep {
        cell: usize,
    },
    Redeem {
        cell: usize,
        index: usize,
    },
    Read {
        cell: usize,
    },
    Release {
        cell: usize,
        refuse: bool,
    },
    Price {
        index: usize,
    },
}

/// The verbs that can reach a merge — the ones that change a relation or take a release path.
/// What the merge-coverage test draws from: a verb that reaches no merge would only spend a step
/// the corpus needed for one, and the three shapes are already rare in a random interleaving.
fn merge_verb() -> impl Strategy<Value = Verb> {
    prop_oneof![
        proptest::option::of(0..8usize).prop_map(|parent| Verb::Create { parent }),
        (0..8usize, 0..8usize).prop_map(|(holder, held)| Verb::Hold { holder, held }),
        (0..8usize, 0..8usize).prop_map(|(producer, consumer)| Verb::Place { producer, consumer }),
        (0..8usize, 0..8usize).prop_map(|(cell, over)| Verb::Continue { cell, over }),
        (0..8usize).prop_map(|cell| Verb::Read { cell }),
        (0..8usize, any::<bool>()).prop_map(|(cell, refuse)| Verb::Release { cell, refuse }),
    ]
}

/// One of `handles`, chosen by a generated index. Wrapping rather than indexing directly: a tree
/// verb has nothing to say when it names past the end, and the staleness these runs are meant to
/// exercise is a handle in the list whose cell has died, not an index off it.
fn wrapped<H: Copy>(handles: &[H], index: usize) -> Option<H> {
    match handles.is_empty() {
        true => None,
        false => Some(handles[index % handles.len()]),
    }
}

/// The tree pool's own verbs: the three that move a cell through its life, the placement door a
/// tree step drives, and the two dormant-carrier doors keyed to a tree home.
fn tree_verb() -> impl Strategy<Value = Verb> {
    prop_oneof![
        (0..8usize, any::<bool>())
            .prop_map(|(parent, under_tree)| Verb::CreateTree { parent, under_tree }),
        (0..8usize, 0..8usize, any::<bool>()).prop_map(|(producer, consumer, into_tree)| {
            Verb::PlaceFromTree {
                producer,
                consumer,
                into_tree,
            }
        }),
        (0..8usize).prop_map(|cell| Verb::KeepTree { cell }),
        (0..8usize, 0..8usize).prop_map(|(cell, index)| Verb::RedeemInTree { cell, index }),
        (0..8usize).prop_map(|cell| Verb::ReleaseTree { cell }),
    ]
}

/// Those plus the two dormant-carrier doors, which mint no hold and take no release path of their
/// own — what they do reach is the reach table, the relocation map, and the masks a merge
/// forwards.
fn state_verb() -> impl Strategy<Value = Verb> {
    prop_oneof![
        3 => merge_verb(),
        1 => (0..8usize).prop_map(|cell| Verb::Keep { cell }),
        1 => (0..8usize, 0..8usize).prop_map(|(cell, index)| Verb::Redeem { cell, index }),
        4 => tree_verb(),
    ]
}

/// Those plus the price query, which the invariant sweep needs interleaved among them to catch a
/// memo taken against a graph that then kept moving.
fn verb() -> impl Strategy<Value = Verb> {
    prop_oneof![
        6 => state_verb(),
        1 => (0..8usize).prop_map(|index| Verb::Price { index }),
    ]
}

/// The tier's ids in id order, since a walk's answers must not depend on hash iteration order.
fn sorted_ids(graph: &CellGraph<Borrowed>) -> Vec<SealedId> {
    let mut ids: Vec<SealedId> = graph.sealed.ids().collect();
    ids.sort();
    ids
}

/// `memoized` carries the ids that already held a memo before this step, and is refreshed to the
/// current set on the way out. Only a price query may write one, so unless the step just run was a
/// `Price` verb, an id outside that set carrying a memo is one a mint or a release left behind.
fn check_invariants(graph: &CellGraph<Borrowed>, memoized: &mut Vec<SealedId>, priced: bool) {
    let occupied: Vec<u32> = (0..CAP)
        .filter(|slot| graph.slots[*slot as usize].state != SlabState::Free)
        .collect();

    for slot in 0..CAP {
        let by_birth = graph.birth.held_by_any(occupied.iter().copied(), slot);
        let by_pins = graph.pins.held_by_any(occupied.iter().copied(), slot);
        let by_aggregate = !graph.naming[slot as usize].is_empty();
        match graph.slots[slot as usize].state {
            SlabState::Free => {
                assert!(
                    !by_birth && !by_pins && !by_aggregate,
                    "slot {slot} is free but something still names it"
                );
                assert!(
                    graph.sealed_holds[slot as usize].is_empty(),
                    "slot {slot} is free but kept a sealed hold"
                );
            }
            // Two relations keep a dead cell in place: a descendant's birth row, and an
            // undisposed tree child, which is the same relation counted rather than rowed.
            SlabState::Dead => assert!(
                by_birth || graph.tree_children_of(graph.occupant(slot)) > 0,
                "slot {slot} is undisposed but nothing names it, so it should have left the slab"
            ),
            SlabState::Live => {}
        }
    }

    // Quiescence spans both tiers: the slab being clear is only half of it, and a graph that
    // reports itself empty while a sealed cell survives would hide exactly the ring this test
    // hunts.
    assert_eq!(
        graph.is_empty(),
        occupied.is_empty() && graph.sealed.is_empty(),
        "is_empty disagrees with the two tiers it summarizes"
    );

    let sealed_ids: Vec<SealedId> = graph.sealed.ids().collect();
    for id in &sealed_ids {
        let sealed_cell = graph.sealed.get(*id).unwrap();
        let from_cells = occupied
            .iter()
            .filter(|slot| graph.sealed_holds[**slot as usize].contains(*id))
            .count();
        let from_sealed = sealed_ids
            .iter()
            .filter(|other| *other != id)
            .filter(|other| {
                graph
                    .sealed
                    .get(**other)
                    .unwrap()
                    .aggregate
                    .names_sealed(*id)
            })
            .count();
        assert_eq!(
            sealed_cell.holders as usize,
            from_cells + from_sealed,
            "sealed cell {id:?} counts holders that do not name it, or misses ones that do"
        );
        // A count of zero reclaims a sealed cell on the spot, so a sealed cell still in the tier at
        // zero is stranded storage nothing can ever release.
        assert!(
            sealed_cell.holders >= 1,
            "sealed cell {id:?} is present with no holder"
        );
        assert!(
            !sealed_cell.aggregate.names_sealed(*id),
            "sealed cell {id:?} names itself"
        );
        for named in sealed_cell.aggregate.slab_slots() {
            assert!(
                graph.naming[named as usize].contains(*id),
                "sealed cell {id:?} names slot {named} without registering in the naming index"
            );
        }
        for named in sealed_cell.aggregate.sealed().iter() {
            assert!(
                sealed_ids.contains(&named),
                "sealed cell {id:?} names a retired sealed cell"
            );
        }
    }

    for slot in 0..CAP {
        assert!(
            !graph.pins.test(slot, slot),
            "slot {slot} holds itself, so its count could never reach zero"
        );
        for id in graph.naming[slot as usize].iter() {
            let sealed_cell = graph
                .sealed
                .get(id)
                .expect("the naming index names a live sealed cell");
            assert!(
                sealed_cell.aggregate.names(slot),
                "the naming index claims sealed cell {id:?} names slot {slot}"
            );
        }
        for id in graph.sealed_holds[slot as usize].iter() {
            assert!(
                sealed_ids.contains(&id),
                "slot {slot} holds a retired sealed cell"
            );
        }
        // Dormant carriers' masks are covered: every bit and id of every entry of the cell's
        // reach table names storage the cell is answerable for, so a read through one is sound.
        for mask in graph.slots[slot as usize].reaches.iter() {
            for named in mask.slab_slots() {
                assert!(
                    graph.slots[named as usize].state != SlabState::Free,
                    "slot {slot} keeps a carrier naming the recycled slot {named}"
                );
                assert!(
                    named == slot || graph.pins.test(slot, named),
                    "slot {slot} keeps a carrier naming slot {named}, which it does not hold"
                );
            }
            for named in mask.sealed().iter() {
                assert!(
                    sealed_ids.contains(&named),
                    "slot {slot} keeps a carrier naming a retired sealed cell"
                );
                assert!(
                    graph.sealed_holds[slot as usize].contains(named),
                    "slot {slot} keeps a carrier naming {named:?}, a sealed cell it does not hold"
                );
            }
        }
    }

    // Relocation is consistent both ways: every key the map answers for names storage that is still
    // there and answers for that key in turn, and every lineage entry a slot or a sealed cell
    // carries is a key of the map pointing back at it. A one-way break would strand a dormant
    // carrier or hand one storage that is not its own.
    for (handle, location) in graph.relocation_entries() {
        assert!(
            !graph.is_live(handle),
            "a live cell answers for its own dormant carriers, so it needs no relocation entry"
        );
        match location {
            SlabForward::Slab { slot, first_index } => {
                let cell = &graph.slots[slot as usize];
                assert!(
                    cell.state != SlabState::Free,
                    "{handle:?} is relocated to the recycled slot {slot}"
                );
                assert!(
                    graph.slot_lineage(slot).contains(&handle),
                    "slot {slot} answers for {handle:?} without carrying it on its chain"
                );
                assert!(
                    first_index < cell.reaches.len(),
                    "{handle:?} is relocated past the end of slot {slot}'s reach table"
                );
            }
            SlabForward::Sealed(id) => {
                assert!(
                    graph.sealed.get(id).is_some(),
                    "a relocation entry names a retired sealed cell"
                );
                assert!(
                    graph.lineage_of(id).contains(&handle),
                    "sealed cell {id:?} answers for {handle:?} without carrying it on its chain"
                );
            }
        }
    }
    for slot in 0..CAP {
        for handle in graph.slot_lineage(slot) {
            assert_eq!(
                graph.relocation_of(handle).map(|location| match location {
                    SlabForward::Slab { slot, .. } => Some(slot),
                    SlabForward::Sealed(_) => None,
                }),
                Some(Some(slot)),
                "slot {slot} carries {handle:?} on its chain without the map pointing here"
            );
        }
    }
    for id in &sealed_ids {
        for handle in graph.lineage_of(*id) {
            assert_eq!(
                graph.relocation_of(handle),
                Some(SlabForward::Sealed(*id)),
                "sealed cell {id:?} carries {handle:?} on its chain without the map pointing here"
            );
        }
    }

    // The occupancy signal is a maintained total, not a scan, so it has to agree with one.
    let scanned: usize = sealed_ids
        .iter()
        .map(|id| graph.sealed.get(*id).unwrap().retained_bytes())
        .sum();
    let occupancy = graph.occupancy();
    assert_eq!(
        occupancy.retained_bytes, scanned,
        "the tier's running byte total drifted from what its sealed cells retain"
    );
    assert_eq!(occupancy.sealed_cells, sealed_ids.len());
    assert_eq!(occupancy.occupied as usize, occupied.len());
    assert_eq!(occupancy.cap, CAP);

    let mut now_memoized = Vec::new();
    for id in &sealed_ids {
        let sealed_cell = graph.sealed.get(*id).unwrap();
        let Some(memo) = sealed_cell.memo() else {
            continue;
        };
        now_memoized.push(*id);
        assert!(
            priced || memoized.contains(id),
            "sealed cell {id:?} carries a memo no price query asked for"
        );
        // Recomputed from scratch, consulting no memo at all: a closure memoized as frozen still
        // names no live cell, spans the same sealed cells, and prices at the same bytes. Nothing
        // inside a frozen closure changes, and this is the check that says so for every
        // interleaving.
        let fresh = graph.transitive_pins(GraphNode::Sealed(*id), false);
        assert!(
            fresh.cells.is_empty(),
            "the memoized closure of {id:?} has since named a live cell"
        );
        let mut walked = fresh.sealed.clone();
        walked.sort();
        let mut memoized = memo.to_vec();
        memoized.sort();
        assert_eq!(walked, memoized, "the memoized closure of {id:?} drifted");
        assert_eq!(
            graph.bytes_of(&fresh),
            memoized
                .iter()
                .map(|id| graph.sealed_bytes(*id))
                .sum::<usize>(),
            "the memoized closure of {id:?} no longer prices to the walked total"
        );
    }
    *memoized = now_memoized;
    check_tree_invariants(graph);
}

/// What the redeem door has to answer, derived from the relations rather than from the door: the
/// executing cell's **root** — its own slot when it is a slab cell — against where the key's home
/// resolves to now.
fn expected_redeem(
    graph: &CellGraph<Borrowed>,
    executing: u32,
    home: CellHandle,
) -> Result<(), RedeemError> {
    let slab_home = match home {
        CellHandle::Slab(handle) => handle,
        CellHandle::Tree(handle) => match graph.trees().resolve(handle) {
            None => return Err(RedeemError::Gone),
            Some(crate::tree::TreeForward::Tree(index)) => {
                return match graph.trees().root(index) == executing {
                    true => Ok(()),
                    false => Err(RedeemError::Unheld),
                };
            }
            Some(crate::tree::TreeForward::Slab(handle)) => handle,
        },
    };
    match graph.locate(slab_home) {
        None => Err(RedeemError::Gone),
        Some(SlabForward::Slab { slot, .. }) => {
            match slot == executing
                || graph.pins.test(executing, slot)
                || graph.birth.test(executing, slot)
            {
                true => Ok(()),
                false => Err(RedeemError::Unheld),
            }
        }
        Some(SlabForward::Sealed(id)) => {
            match graph.sealed_holds[executing as usize].contains(id) {
                true => Ok(()),
                false => Err(RedeemError::Unheld),
            }
        }
    }
}

/// Redeem inside a step and check what comes back: a value that answers must read the number it
/// was kept as, which is what says a mask forwarded through a merge — or a tombstone chain — still
/// names the right storage.
fn check_redeem(
    context: &StepContext<'_, '_, Borrowed>,
    dormant: Dormant<Number>,
    carried: u32,
) -> Result<(), RedeemError> {
    match context.redeem(dormant) {
        Ok(carrier) => {
            assert_eq!(
                *context.read(&carrier).value(),
                carried,
                "a redeemed value read storage that was not its own"
            );
            Ok(())
        }
        Err(error) => Err(error),
    }
}

/// The tree pool's own invariants, checked after every step beside the matrix ones.
fn check_tree_invariants(graph: &CellGraph<Borrowed>) {
    let pool = graph.trees();
    let occupied: Vec<u32> = pool.occupied().collect();
    let alive = |index: u32| matches!(pool.state(index), TreeState::Live | TreeState::Dead);

    // Every tombstone points at storage that is still answerable — a tree cell that has it, or a
    // slab handle the relocation map forwards — and never at a place that has gone.
    for index in occupied.iter().copied() {
        if pool.state(index) != TreeState::Absorbed {
            continue;
        }
        match pool
            .tombstone_target(index)
            .expect("a tombstone records where its bytes went")
        {
            CellHandle::Tree(target) => assert!(
                pool.state(target.index()) != TreeState::Free,
                "tombstone {index} points at a recycled pool slot"
            ),
            CellHandle::Slab(handle) => assert!(
                graph.locate(handle).is_some(),
                "tombstone {index} points at a slab cell nothing answers for"
            ),
        }
    }

    // Every tombstone is on exactly one lineage list, and every lineage list holds only tombstones.
    let heads = graph
        .relocated_tree_tombstones()
        .into_iter()
        .chain((0..graph.cap).filter_map(|slot| graph.tree_tombstones_of(graph.occupant(slot))))
        .chain(
            occupied
                .iter()
                .copied()
                .filter_map(|index| pool.tombstones(index)),
        );
    let mut listed: Vec<u32> = Vec::new();
    let mut pending: Vec<u32> = heads.collect();
    while let Some(index) = pending.pop() {
        assert_eq!(
            pool.state(index),
            TreeState::Absorbed,
            "a lineage list holds pool slot {index}, which is not a tombstone"
        );
        assert!(
            !listed.contains(&index),
            "tombstone {index} is on two lineage lists"
        );
        listed.push(index);
        pending.extend(pool.next_tombstone(index));
    }
    for index in occupied.iter().copied() {
        assert!(
            pool.state(index) != TreeState::Absorbed || listed.contains(&index),
            "tombstone {index} is on no lineage list"
        );
    }

    for index in occupied.iter().copied().filter(|index| alive(*index)) {
        // The child count is the birth tally's analogue: it has to agree with a scan.
        let children = occupied
            .iter()
            .copied()
            .filter(|other| alive(*other) && pool.parent(*other) == Ancestor::Tree(index))
            .count();
        assert_eq!(
            pool.children(index) as usize,
            children,
            "pool slot {index} counts children that do not name it, or misses ones that do"
        );
        // A pledge is always an ancestor, and everything between the cell and its pledge is
        // pledged at least as shallow — which is what keeps a grandparent's bundle from borrowing
        // bytes an intermediate reclaimed.
        let Some(pledge) = pool.pledge(index) else {
            continue;
        };
        let floor = pool.ancestor_depth(pledge);
        assert!(
            floor < pool.depth(index),
            "pool slot {index} pledged into itself or below"
        );
        if let Ancestor::Tree(target) = pledge {
            assert!(
                alive(target),
                "pool slot {index} pledged into a cell that is gone"
            );
            assert_eq!(
                pool.ancestry(index, target),
                crate::tree::Ancestry::Above,
                "pool slot {index} pledged into a cell that is not an ancestor"
            );
        }
        let mut at = pool.parent(index);
        while let Ancestor::Tree(intermediate) = at {
            if pool.depth(intermediate) <= floor {
                break;
            }
            let held = pool
                .pledge(intermediate)
                .expect("an intermediate below a pledge is pledged too");
            assert!(
                pool.ancestor_depth(held) <= floor,
                "pool slot {intermediate} lies below a pledge it does not carry"
            );
            at = pool.parent(intermediate);
        }
    }

    // A root's tree-child count is the same tally, over the cells whose chain tops out at it.
    for slot in 0..graph.cap {
        let children = occupied
            .iter()
            .copied()
            .filter(|index| {
                alive(*index) && pool.parent(*index) == Ancestor::Root && pool.root(*index) == slot
            })
            .count();
        assert_eq!(
            graph.tree_children_of(graph.occupant(slot)) as usize,
            children,
            "slot {slot} counts tree children that do not name it, or misses ones that do"
        );
    }
}

/// Drive one generated run to its end — every verb, then a wind-down that releases everything —
/// checking the invariants after every step. Reports the merges the run performed, which is what
/// tells a generated corpus that reaches all three shapes from one that only claims to.
fn run(verbs: &[Verb], verdict: impl FnMut(Prices) -> Verdict + 'static) -> Merges {
    let mut graph: CellGraph<Borrowed> = CellGraph::new(CAP, verdict);
    let mut minted: Vec<SlabHandle> = Vec::new();
    // Every tree cell the run created, in creation order. A generated index may name one that has
    // since died, which is the point: the doors have to refuse it.
    let mut grown: Vec<TreeHandle> = Vec::new();
    // Every value put to rest, beside the cell it was kept in and the number it carries — so a
    // redeem that answers can be checked against what it was supposed to hand back.
    let mut kept: Vec<(CellHandle, Dormant<Number>, u32)> = Vec::new();
    let mut next_value: u32 = 0;
    // Nothing has been priced yet, so no sealed cell may carry a memo.
    let mut memoized: Vec<SealedId> = Vec::new();

    for step in verbs {
        match *step {
            Verb::Create { parent } => {
                let parent = parent.and_then(|index| minted.get(index).copied());
                if let Ok(handle) = graph.create(parent, None) {
                    minted.push(handle);
                }
            }
            Verb::Hold { holder, held } => {
                if let (Some(holder), Some(held)) =
                    (minted.get(holder).copied(), minted.get(held).copied())
                    && graph.is_live(holder)
                {
                    let _ = graph.enter(holder, |context| context.hold(held));
                }
            }
            Verb::Place { producer, consumer } => {
                if let (Some(producer), Some(consumer)) =
                    (minted.get(producer).copied(), minted.get(consumer).copied())
                    && graph.is_live(producer)
                {
                    let _ = graph.enter(producer, |context| {
                        let value = number_here(context, 1);
                        context
                            .alloc_into::<Number, Number>(
                                consumer,
                                &[operand_at(&value, 1)],
                                |writer, views| take(&views[0], writer),
                            )
                            .map(|_| ())
                    });
                }
            }
            // A continuation kept over a value homed elsewhere takes an entry of the cell's
            // reach table, interned on its reach like any other keep.
            Verb::Continue { cell, over } => {
                if let (Some(cell), Some(over)) =
                    (minted.get(cell).copied(), minted.get(over).copied())
                    && graph.is_live(cell)
                {
                    let _ = graph.enter(cell, |context| {
                        if let Ok(value) =
                            context.alloc_into::<Number, Number>(over, &[], |w, _| one(w, 1))
                        {
                            let captured = context
                                .alloc_here(&[operand_at(&value, 1)], |writer, views| {
                                    take(&views[0], writer)
                                });
                            context.store_successor(captured);
                        }
                    });
                }
            }
            // A value put to rest in the cell that built it. Its mask lives in that cell's
            // reach table from here on, where every merge and every seal has to maintain it.
            Verb::Keep { cell } => {
                if let Some(cell) = minted.get(cell).copied()
                    && graph.is_live(cell)
                {
                    let carried = next_value;
                    next_value += 1;
                    let dormant = graph
                        .enter(cell, |context| {
                            let value = number_here(context, carried);
                            context.keep(value)
                        })
                        .unwrap();
                    kept.push((CellHandle::Slab(cell), dormant, carried));
                }
            }
            // The door back. The outcome is predicted from the graph's state before the call —
            // where the home's dormant carriers live now, and whether this cell has a claim on them
            // — and a successful redeem has to hand back the number that was kept, which is what
            // says a mask forwarded through a merge still names the right storage.
            Verb::Redeem { cell, index } => {
                if let Some(cell) = minted.get(cell).copied()
                    && graph.is_live(cell)
                    && !kept.is_empty()
                {
                    let (home, dormant, carried) = kept[index % kept.len()];
                    let expected = expected_redeem(&graph, cell.slot(), home);
                    let outcome = graph
                        .enter(cell, |context| check_redeem(context, dormant, carried))
                        .unwrap();
                    assert_eq!(
                        outcome, expected,
                        "the redeem door disagreed with the relations that entitle it"
                    );
                }
            }
            // A tree cell under a slab cell or under another tree cell. The pool takes no cap, so
            // the only refusal is a parent whose death was already declared.
            Verb::CreateTree { parent, under_tree } => {
                // A tree parent when the run has one and the draw asks for it, and the slab
                // otherwise. A run with no slab cell yet takes one now: a tree cell is meaningless
                // without a root, so the alternative is a verb that can never fire.
                if minted.is_empty()
                    && let Ok(handle) = graph.create(None, None)
                {
                    minted.push(handle);
                }
                let parent: Option<CellHandle> = under_tree
                    .then(|| wrapped(&grown, parent).map(CellHandle::Tree))
                    .flatten()
                    .or_else(|| wrapped(&minted, parent).map(CellHandle::Slab));
                if let Some(parent) = parent
                    && let Ok(handle) = graph.create_tree(parent, None)
                {
                    grown.push(handle);
                }
            }
            // A placement out of a tree step, into either kind. Where the destination sits decides
            // whether the operand pins — pledging the producer's chain — or is copied outright.
            Verb::PlaceFromTree {
                producer,
                consumer,
                into_tree,
            } => {
                let consumer: Option<CellHandle> = match into_tree {
                    true => wrapped(&grown, consumer).map(CellHandle::Tree),
                    false => wrapped(&minted, consumer).map(CellHandle::Slab),
                };
                if let (Some(producer), Some(consumer)) = (wrapped(&grown, producer), consumer)
                    && graph.is_live(producer)
                {
                    let _ = graph.enter(producer, |context| {
                        let value = number_here(context, 1);
                        context
                            .alloc_into::<Number, Number>(
                                consumer,
                                &[operand_at(&value, 1)],
                                |writer, views| take(&views[0], writer),
                            )
                            .map(|_| ())
                    });
                }
            }
            // A value put to rest in a tree cell. It interns nothing — the reach is the root —
            // but it does make the cell nameable, which is what decides whether its death leaves
            // a tombstone behind.
            Verb::KeepTree { cell } => {
                if let Some(cell) = wrapped(&grown, cell)
                    && graph.is_live(cell)
                {
                    let carried = next_value;
                    next_value += 1;
                    let dormant = graph
                        .enter(cell, |context| {
                            let value = number_here(context, carried);
                            context.keep(value)
                        })
                        .unwrap();
                    kept.push((CellHandle::Tree(cell), dormant, carried));
                }
            }
            // The same door from inside the tree, where entitlement is root identity rather than a
            // row reading, and a tree home resolves through however many splices its bytes have
            // been through since the keep.
            Verb::RedeemInTree { cell, index } => {
                if let Some(cell) = wrapped(&grown, cell)
                    && graph.is_live(cell)
                    && !kept.is_empty()
                {
                    let (home, dormant, carried) = kept[index % kept.len()];
                    let root = graph.trees().root(cell.index());
                    let expected = expected_redeem(&graph, root, home);
                    let outcome = graph
                        .enter(cell, |context| check_redeem(context, dormant, carried))
                        .unwrap();
                    assert_eq!(
                        outcome, expected,
                        "the redeem door disagreed with the root that entitles it"
                    );
                }
            }
            // Death in any order: a cell released before its children waits dead-but-undisposed,
            // and the last child's disposal cascades up through every ancestor it unblocks.
            Verb::ReleaseTree { cell } => {
                if let Some(cell) = wrapped(&grown, cell) {
                    let _ = graph.release_tree(cell);
                }
            }
            // Reading the kept continuation back is the re-anchor: the value comes out at the
            // step brand over whatever the tier has since done to the regions it captured. The
            // mask it was stored with stays in the slot, where the sweep checks it.
            Verb::Read { cell } => {
                if let Some(cell) = minted.get(cell).copied()
                    && graph.is_live(cell)
                {
                    graph
                        .enter(cell, |context| {
                            let _ = context.continuation();
                        })
                        .unwrap();
                }
            }
            // Both dispositions are generated, so a run reaches the sealed shapes a merge would
            // otherwise have collapsed as well as the merges themselves.
            Verb::Release { cell, refuse } => {
                let absorption = if refuse {
                    ReleaseAbsorption::Refused
                } else {
                    ReleaseAbsorption::IntoHolder
                };
                if let Some(cell) = minted.get(cell).copied() {
                    let before = live_bytes(&graph, CAP);
                    let _ = graph.release(cell, absorption);
                    // A merge moves bytes between live cells and a seal moves them out, but nothing
                    // moves them back in: storage that has sealed never re-enters the live tier.
                    assert!(
                        live_bytes(&graph, CAP) <= before,
                        "a release grew the live tier, so sealed storage re-entered it"
                    );
                }
            }
            // Pricing is read-only: it changes no hold, and the invariant sweep after every step
            // is what says so. What it does write is a memo, and the sweep re-derives every one.
            Verb::Price { index } => {
                let ids = sorted_ids(&graph);
                if !ids.is_empty() {
                    // The candidate the index picks is priced alone too, so a run reaches the
                    // whole-closure answer as well as the shared partition.
                    let alone = ids[index % ids.len()];
                    let slices = graph.unique_retentions(&ids);
                    let mut total = 0;
                    for (id, slice) in ids.iter().zip(&slices) {
                        let slice = slice.expect("every id came out of the tier");
                        // One candidate shares its closure with nobody, so its slice is the whole.
                        let whole = graph
                            .unique_retentions(&[*id])
                            .remove(0)
                            .expect("the id came out of the tier");
                        assert!(
                            slice.bytes <= whole.bytes,
                            "the unique slice of {id:?} outprices its whole closure"
                        );
                        assert_eq!(slice.frozen, whole.frozen);
                        total += slice.bytes;
                    }
                    assert!(
                        graph.unique_retentions(&[alone]).remove(0).is_some(),
                        "the id {alone:?} the sweep just walked priced as absent"
                    );
                    // The slices partition part of one graph, so together they cannot outprice it.
                    assert!(
                        total <= graph.occupancy().retained_bytes + live_bytes(&graph, CAP),
                        "the unique slices together outprice both tiers"
                    );
                }
            }
        }
        check_invariants(&graph, &mut memoized, matches!(*step, Verb::Price { .. }));
    }

    // Winding the run down: once every cell's death is declared, the cascade returns every slot,
    // and the tier retains only what a ring no merge met tied together. The pool first: a root with
    // an undisposed tree child under it waits dead-but-undisposed exactly as one with a live
    // descendant does, so the slab cannot finish until the trees have.
    for handle in &grown {
        let _ = graph.release_tree(*handle);
    }
    for handle in &minted {
        let _ = graph.release(*handle, ReleaseAbsorption::IntoHolder);
    }
    check_invariants(&graph, &mut memoized, false);
    assert!(
        graph.trees().occupied().next().is_none(),
        "a wound-down run left a tree cell or a tombstone in the pool"
    );
    for slot in 0..CAP {
        assert_eq!(graph.slots[slot as usize].state, SlabState::Free);
    }
    // Every surviving sealed cell is a ring by the invariants above; this says which rings can
    // survive. A region no more than one hold set ever named is one a merge reaches — its sole
    // holder either absorbs it, seals and folds it in, or drops the last hold — so a survivor was
    // shared once.
    for id in graph.sealed.ids() {
        let sealed_cell = graph.sealed.get(id).unwrap();
        assert!(
            sealed_cell.peak_holders >= 2,
            "sealed cell {id:?} survived the wind-down having never had a second holder"
        );
    }
    assert_eq!(graph.is_empty(), graph.sealed.is_empty());
    graph.merges
}

proptest! {
    // No failure-persistence file: a regression file would record generated cases into the source
    // tree, and resolving its path calls `getcwd`, which Miri's isolation refuses. Under Miri the
    // case count drops to what a slate run can afford — the shapes are what matter there, not the
    // breadth, which the native run already covers.
    #![proptest_config(ProptestConfig {
        cases: if cfg!(miri) { 8 } else { ProptestConfig::default().cases },
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    #[test]
    fn no_interleaving_strands_a_slot_or_desynchronizes_the_two_tiers(
        verbs in proptest::collection::vec(verb(), 1..40)
    ) {
        run(&verbs, alternating());
    }
}

/// The generated corpus reaches every locality merge, rather than only being able to.
///
/// An interleaving invariant test is only worth what its runs cover: without this, a merge that
/// never fired would look exactly like a merge that always held. Miri skips the assertion — four
/// cases is what a slate run affords, and that is too few to reach all three shapes reliably.
#[test]
fn each_merge_fires_across_generated_interleavings() {
    use proptest::strategy::{Strategy, ValueTree};
    use proptest::test_runner::TestRunner;

    let cases = if cfg!(miri) { 4 } else { 256 };
    let strategy = proptest::collection::vec(merge_verb(), 1..40);
    let mut runner = TestRunner::deterministic();
    let mut total = Merges::default();

    for _ in 0..cases {
        let verbs = strategy.new_tree(&mut runner).unwrap().current();
        // Always-pin: a merge is a release-path shape, and it is pins that build the chains one
        // needs. A verdict that severs a third of them only thins the corpus this sweep is
        // measuring, without putting any merge out of reach in principle.
        let merges = run(&verbs, pin);
        total.into_cell += merges.into_cell;
        total.at_seal += merges.at_seal;
        total.into_namer += merges.into_namer;
    }

    if !cfg!(miri) {
        assert!(
            total.into_cell > 0,
            "no run absorbed a cell into its holder"
        );
        assert!(
            total.at_seal > 0,
            "no run absorbed a count-1 sealed cell at a seal"
        );
        assert!(total.into_namer > 0, "no run sealed a cell into its namer");
    }
}
