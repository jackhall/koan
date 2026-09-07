//! The crossing: the one closure a table is built with, consulted once per operand of every
//! placement over operands, and the two brands its answer hands the build
//! ([design/liveness-matrix.md § Bounding the two tiers](../../../design/liveness-matrix.md#bounding-the-two-tiers)).
//!
//! What these pin: the verdict sees both prices and both tiers' occupancy; a pin mints the
//! operand's reach into the destination and a copy mints nothing; the marginal pin price discounts
//! what the destination already holds and prices a frozen closure through its memo; and the ruled
//! loop shape — a cart plus two hop cells — runs to a bounded slab with no record and no merge.

use std::cell::RefCell;
use std::rc::Rc;

use super::super::*;
use super::{Borrowed, Number, Owned, number, operand_at, take};

/// A verdict that records every crossing it is shown and answers from `answer`.
///
/// The log is the whole point: the substrate promises one consultation per operand with the prices
/// of *that* operand, and only a recorded sequence can say so.
fn recording(
    answer: impl Fn(Prices) -> Verdict + 'static,
) -> (
    impl FnMut(Prices) -> Verdict + 'static,
    Rc<RefCell<Vec<Prices>>>,
) {
    let seen = Rc::new(RefCell::new(Vec::new()));
    let log = Rc::clone(&seen);
    let verdict = move |prices: Prices| {
        log.borrow_mut().push(prices);
        answer(prices)
    };
    (verdict, seen)
}

/// The embedder threshold these tests steer with: copy where the embedder's own figure undercuts
/// what the pin would newly retain. An operand passed at `usize::MAX` is one it will never copy.
fn cheaper(prices: Prices) -> Verdict {
    if prices.copy_bytes < prices.pin_bytes {
        Verdict::Copy
    } else {
        Verdict::Pin
    }
}

#[test]
fn the_verdict_is_consulted_once_per_operand_with_both_prices() {
    let (verdict, seen) = recording(|_| Verdict::Pin);
    let mut table: CellTable<Owned> = CellTable::new(4, verdict);
    let destination = table.create(None, None).unwrap();
    let producer = table.create(None, None).unwrap();

    table
        .enter(producer, |context| {
            // One operand homed in the destination and one homed elsewhere, at two stated copy
            // costs, in one placement.
            let there = context
                .alloc_into::<Number, Number>(destination, &[], |writer, _| writer.value(1))
                .unwrap();
            let own = context.alloc::<Number>(|writer| writer.value(2));
            context
                .alloc_into::<Number, Number>(
                    destination,
                    &[operand_at(&there, 3), operand_at(&own, 5)],
                    |writer, views| take(&views[0], writer),
                )
                .unwrap();
        })
        .unwrap();

    let seen = seen.borrow();
    assert_eq!(seen.len(), 2, "one consultation per operand, in order");
    let occupancy = table.occupancy();
    for prices in seen.iter() {
        assert_eq!(prices.occupied, occupancy.occupied);
        assert_eq!(prices.cap, occupancy.cap);
        // The cap the table was built at, not the width of the row its type fixes.
        assert_eq!(prices.cap, 4);
        assert_eq!(prices.records, occupancy.records);
        assert_eq!(prices.retained_bytes, occupancy.retained_bytes);
        assert_eq!(prices.dest_bytes, table.region_bytes(destination).unwrap());
    }
    // The embedder's own figures come through untouched, in the order the operands were passed.
    assert_eq!(seen[0].copy_bytes, 3);
    assert_eq!(seen[1].copy_bytes, 5);
    // An operand the destination is already answerable for costs a pin nothing; one homed in
    // another cell costs that cell's storage.
    assert_eq!(seen[0].pin_bytes, 0);
    assert_eq!(seen[1].pin_bytes, table.region_bytes(producer).unwrap());
}

#[test]
fn a_pin_mints_the_operands_reach_and_a_copy_does_not() {
    for verdict in [Verdict::Pin, Verdict::Copy] {
        let mut table: CellTable<Owned> = CellTable::new(4, move |_| verdict);
        let destination = table.create(None, None).unwrap();
        let producer = table.create(None, None).unwrap();

        let names_producer = table
            .enter(producer, |context| {
                let own = context.alloc::<Number>(|writer| writer.value(41));
                let placed = context
                    .alloc_into::<Number, Number>(
                        destination,
                        &[operand_at(&own, 0)],
                        |writer, views| take(&views[0], writer),
                    )
                    .unwrap();
                assert_eq!(*context.read(&placed).value(), 41);
                placed.reach().names(producer.slot())
            })
            .unwrap();

        assert_eq!(names_producer, verdict == Verdict::Pin);
        assert_eq!(table.holds(destination, producer), verdict == Verdict::Pin);

        // The whole point of the copy: the producer's column is zero, so its death is a
        // reclamation rather than a record the destination now retains. The slot comes back
        // either way — retention lives in the sealed tier, never in the slab.
        table.release(producer, Absorption::Refused).unwrap();
        assert_eq!(super::state_of(&table, producer), SlotState::Free);
        assert_eq!(table.sealed.len(), usize::from(verdict == Verdict::Pin));
    }
}

#[test]
fn a_copied_view_is_readable_and_a_pinned_one_embeddable() {
    let (verdict, seen) = recording(cheaper);
    let mut table: CellTable<Owned> = CellTable::new(4, verdict);
    let destination = table.create(None, None).unwrap();
    let producer = table.create(None, None).unwrap();

    let read = table
        .enter(producer, |context| {
            let own = context.alloc::<Number>(|writer| writer.value(41));
            // Cheap to copy, against a pin that would newly retain the producer's whole region.
            let copied = context
                .alloc_into::<Number, Number>(
                    destination,
                    &[operand_at(&own, 0)],
                    |writer, views| take(&views[0], writer),
                )
                .unwrap();
            // Unpriceable to copy, so the same operand pins and the build embeds the borrow.
            let pinned = context
                .alloc_into::<Number, Number>(
                    destination,
                    &[operand_at(&own, usize::MAX)],
                    |writer, views| take(&views[0], writer),
                )
                .unwrap();
            assert!(!copied.reach().names(producer.slot()));
            assert!(pinned.reach().names(producer.slot()));
            (
                *context.read(&copied).value(),
                *context.read(&pinned).value(),
            )
        })
        .unwrap();

    // The deep copy reads what it was copied from, and the embedded borrow reads the producer's
    // own storage — which the destination now holds.
    assert_eq!(read, (41, 41));
    assert!(table.holds(destination, producer));
    let seen = seen.borrow();
    assert_eq!(seen.len(), 2);
    assert!(
        seen[0].pin_bytes > 0,
        "a pin over this operand would have retained the producer's region"
    );
    // A copy mints nothing, so the second crossing is priced against the same hold set as the
    // first: the choice to copy leaves the destination answerable for no more than it was.
    assert_eq!(seen[1].pin_bytes, seen[0].pin_bytes);
    // What the destination did take in is the deep copy itself.
    assert!(seen[1].dest_bytes >= seen[0].dest_bytes);
}

#[test]
fn pin_price_is_marginal_against_what_the_destination_already_holds() {
    let (verdict, seen) = recording(|_| Verdict::Pin);
    let mut table: CellTable<Owned> = CellTable::new(6, verdict);
    let destination = table.create(None, None).unwrap();
    let held = table.create(None, None).unwrap();
    let head = table.create(None, None).unwrap();
    let tail = table.create(None, None).unwrap();
    let doomed = table.create(None, None).unwrap();
    let driver = table.create(None, None).unwrap();

    // The destination already answers for `held`, so a pin over it retains nothing new.
    table
        .enter(destination, |context| context.hold(held))
        .unwrap()
        .unwrap();
    // A chain the destination does not hold: a cell, a cell it holds, and a record it holds.
    table
        .enter(driver, |context| {
            context
                .alloc_into::<Number, Number>(tail, &[], |writer, _| writer.value(1))
                .unwrap();
            context
                .alloc_into::<Number, Number>(doomed, &[], |writer, _| writer.value(2))
                .unwrap();
        })
        .unwrap();
    table
        .enter(head, |context| {
            context.hold(tail).unwrap();
            context.hold(doomed)
        })
        .unwrap()
        .unwrap();
    table.release(doomed, Absorption::Refused).unwrap();
    let record = table.sealed.ids().next().unwrap();

    table
        .enter(driver, |context| {
            let near = context
                .alloc_into::<Number, Number>(held, &[], |writer, _| writer.value(3))
                .unwrap();
            let far = context
                .alloc_into::<Number, Number>(head, &[], |writer, _| writer.value(4))
                .unwrap();
            context
                .alloc_into::<Number, Number>(
                    destination,
                    &[operand_at(&near, usize::MAX), operand_at(&far, usize::MAX)],
                    |writer, views| take(&views[0], writer),
                )
                .unwrap();
        })
        .unwrap();

    let seen = seen.borrow();
    assert_eq!(seen.len(), 2);
    assert_eq!(
        seen[0].pin_bytes, 0,
        "the destination already holds this cell"
    );
    // Everything the walk reaches from the unheld cell, across both tiers, and nothing else.
    assert_eq!(
        seen[1].pin_bytes,
        table.region_bytes(head).unwrap()
            + table.region_bytes(tail).unwrap()
            + table.sealed_retained_bytes(record).unwrap()
    );
}

#[test]
fn operands_from_one_source_are_priced_against_what_the_placement_has_already_pinned() {
    let (verdict, seen) = recording(|_| Verdict::Pin);
    let mut table: CellTable<Owned> = CellTable::new(4, verdict);
    let destination = table.create(None, None).unwrap();
    let source = table.create(None, None).unwrap();
    let driver = table.create(None, None).unwrap();

    table
        .enter(driver, |context| {
            // Two values homed in the same cell, so both operands reach exactly `source`.
            let first = context
                .alloc_into::<Number, Number>(source, &[], |writer, _| writer.value(1))
                .unwrap();
            let second = context
                .alloc_into::<Number, Number>(source, &[], |writer, _| writer.value(2))
                .unwrap();
            context
                .alloc_into::<Number, Number>(
                    destination,
                    &[
                        operand_at(&first, usize::MAX),
                        operand_at(&second, usize::MAX),
                    ],
                    |writer, views| take(&views[0], writer),
                )
                .unwrap();
        })
        .unwrap();

    let seen = seen.borrow();
    assert_eq!(seen.len(), 2);
    assert_eq!(
        seen[0].pin_bytes,
        table.region_bytes(source).unwrap(),
        "the first operand from a shared source carries the shared cost"
    );
    assert_eq!(
        seen[1].pin_bytes, 0,
        "the second reaches nothing the placement has not already pinned"
    );
}

#[test]
fn a_copied_operand_leaves_the_next_one_the_whole_price() {
    // Copy where the embedder's figure undercuts the pin: the first operand is passed at zero and
    // is copied, the second at `usize::MAX` and is pinned.
    let (verdict, seen) = recording(cheaper);
    let mut table: CellTable<Owned> = CellTable::new(4, verdict);
    let destination = table.create(None, None).unwrap();
    let source = table.create(None, None).unwrap();
    let driver = table.create(None, None).unwrap();

    table
        .enter(driver, |context| {
            let first = context
                .alloc_into::<Number, Number>(source, &[], |writer, _| writer.value(1))
                .unwrap();
            let second = context
                .alloc_into::<Number, Number>(source, &[], |writer, _| writer.value(2))
                .unwrap();
            context
                .alloc_into::<Number, Number>(
                    destination,
                    &[operand_at(&first, 0), operand_at(&second, usize::MAX)],
                    |writer, views| take(&views[1], writer),
                )
                .unwrap();
        })
        .unwrap();

    let seen = seen.borrow();
    assert_eq!(seen.len(), 2);
    let source_bytes = table.region_bytes(source).unwrap();
    assert_eq!(seen[0].pin_bytes, source_bytes);
    // A copy mints nothing, so the second operand is still the first to bring `source` in.
    assert_eq!(
        seen[1].pin_bytes, source_bytes,
        "a refused pin leaves the source unheld"
    );
}

#[test]
fn a_frozen_closure_prices_through_its_memo() {
    let (verdict, seen) = recording(|_| Verdict::Pin);
    let mut table: CellTable<Owned> = CellTable::new(4, verdict);
    let destination = table.create(None, None).unwrap();
    let consumer = table.create(None, None).unwrap();
    let producer = table.create(None, None).unwrap();

    table
        .enter(consumer, |context| context.hold(producer))
        .unwrap()
        .unwrap();
    let kept = table
        .enter(producer, |context| {
            let value = context.alloc::<Number>(|writer| writer.value(41));
            context.keep(value)
        })
        .unwrap();
    table.release(producer, Absorption::Refused).unwrap();
    let record = table.sealed.ids().next().unwrap();

    // The closure is frozen, so its price is memoized once and never recomputed.
    let closure = table.unique_closures(&[record]).remove(0).unwrap();
    assert!(closure.frozen);

    table
        .enter(consumer, |context| {
            let carrier = context.redeem(kept).expect("the consumer holds the record");
            // A carrier whose reach is a record alone: the price is the record's whole closure.
            context
                .alloc_into::<Number, Number>(
                    destination,
                    &[operand_at(&carrier, usize::MAX)],
                    |writer, views| take(&views[0], writer),
                )
                .unwrap();
            // The pin above minted the record into the destination's sealed holds, so the same
            // operand is free the second time.
            context
                .alloc_into::<Number, Number>(
                    destination,
                    &[operand_at(&carrier, usize::MAX)],
                    |writer, views| take(&views[0], writer),
                )
                .unwrap();
        })
        .unwrap();

    let seen = seen.borrow();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].pin_bytes, closure.bytes);
    assert_eq!(seen[1].pin_bytes, 0);
    assert!(table.sealed_holds[destination.slot() as usize].contains(record));
}

/// Hops the ruled loop shape runs. Miri takes the shortest run that still alternates the two hop
/// cells and retires each of them twice.
const HOPS: usize = if cfg!(miri) { 4 } else { 16 };

#[test]
fn a_loop_is_two_hop_cells_and_a_cart() {
    let (verdict, seen) = recording(cheaper);
    let mut table: CellTable<Owned> = CellTable::new(4, verdict);
    let cart = table.create(None, None).unwrap();
    let mut running = table.create(None, None).unwrap();
    let mut waiting = table.create(None, None).unwrap();

    // The cart seeds both residents: the first hop's argument, built into the hop's own region,
    // and the accumulator, which lives in the cart from here on.
    let (mut argument, mut accumulated) = table
        .enter(cart, |context| {
            let first = context
                .alloc_into::<Number, Number>(running, &[], |writer, _| writer.value(1))
                .unwrap();
            let total = context.alloc::<Number>(|writer| writer.value(0));
            (context.keep(first), context.keep(total))
        })
        .unwrap();

    for hop in 0..HOPS {
        let (next_argument, next_accumulated) = table
            .enter(running, |context| {
                // The hop holds the cart, which is what entitles it to the accumulator.
                context.hold(cart).unwrap();
                let argument = context.redeem(argument).expect("the hop is its own home");
                let accumulated = context.redeem(accumulated).expect("the hop holds the cart");

                // The next hop's argument goes into the *other* hop cell, over a copy: this cell
                // is about to retire, so nothing may be left holding it.
                let passed = context
                    .alloc_into::<Number, Number>(
                        waiting,
                        &[operand_at(&argument, 0)],
                        |writer, views| writer.value(number(&views[0]) + 1),
                    )
                    .unwrap();
                // The accumulated result goes into the cart, over the cart's own value pinned —
                // free, since the cart already answers for itself — and this cell's copied.
                let total = context
                    .alloc_into::<Number, Number>(
                        cart,
                        &[
                            operand_at(&accumulated, usize::MAX),
                            operand_at(&argument, 0),
                        ],
                        |writer, views| writer.value(number(&views[0]) + number(&views[1])),
                    )
                    .unwrap();
                (context.keep(passed), context.keep(total))
            })
            .unwrap();
        argument = next_argument;
        accumulated = next_accumulated;

        // The retiring hop's column is zero: neither the cart nor the next hop took a hold on it,
        // so its death frees the slot outright — no record, no merge.
        table.release(running, Absorption::IntoHolder).unwrap();
        assert_eq!(super::state_of(&table, running), SlotState::Free);
        assert_eq!(table.sealed.len(), 0, "hop {hop} left a record behind");
        assert_eq!(table.merges, Merges::default(), "hop {hop} took a merge");
        assert!(!table.holds(cart, waiting));

        let fresh = table.create(None, None).unwrap();
        assert!(table.occupancy().occupied <= 3, "hop {hop} grew the slab");
        running = waiting;
        waiting = fresh;
    }

    // The cart is kept into once per hop and its table did not grow: every accumulator reaches
    // the cart and nothing else, so all of them intern to the entry the seed minted. This is what
    // keeps the seal transition's bound — work per holder's resident entry — a bound on a run of
    // any length rather than one that grows with it.
    assert_eq!(
        table.slots[cart.slot() as usize].residents.len(),
        1,
        "the cart took an entry per hop"
    );

    // The cart's accumulator carries the whole run, and the argument waiting in the hop that
    // never ran is the one the last hop passed on.
    let total = table
        .enter(cart, |context| {
            let total = context.redeem(accumulated).expect("the cart is the home");
            *context.read(&total).value()
        })
        .unwrap();
    let argument = table
        .enter(running, |context| {
            let argument = context.redeem(argument).expect("the hop is its own home");
            *context.read(&argument).value()
        })
        .unwrap();
    assert_eq!(total, (1..=HOPS as u32).sum::<u32>());
    assert_eq!(argument, HOPS as u32 + 1);

    // The cart accretes only the values built into it, so the size the verdict is shown of it
    // climbs monotonically while it holds neither hop cell.
    let cart_sizes: Vec<usize> = seen
        .borrow()
        .iter()
        .filter(|prices| prices.copy_bytes == usize::MAX)
        .map(|prices| prices.dest_bytes)
        .collect();
    assert_eq!(cart_sizes.len(), HOPS);
    assert!(cart_sizes.windows(2).all(|pair| pair[0] <= pair[1]));
}

#[test]
fn captures_cross_through_the_same_verdict() {
    for verdict in [Verdict::Pin, Verdict::Copy] {
        let mut table: CellTable<Borrowed> = CellTable::new(4, move |_| verdict);
        let keeper = table.create(None, None).unwrap();
        let host = table.create(None, None).unwrap();

        table
            .enter(keeper, |context| {
                let value = context
                    .alloc_into::<Number, Number>(host, &[], |writer, _| writer.value(41))
                    .unwrap();
                context.store_successor_capturing(&[operand_at(&value, 0)], |writer, views| {
                    take(&views[0], writer)
                });
            })
            .unwrap();

        // A pinned capture is the host's own storage, so the cell holds the host across the gap; a
        // copied one lives in the keeper's region and the host is free to die.
        assert_eq!(table.holds(keeper, host), verdict == Verdict::Pin);
        assert_eq!(
            super::continuation_reach(&table, keeper).names(host.slot()),
            verdict == Verdict::Pin
        );

        let read = table
            .enter(keeper, |context| *context.continuation().unwrap().value())
            .unwrap();
        assert_eq!(read, 41);
    }
}

#[test]
fn a_placement_over_no_operands_consults_nothing() {
    let (verdict, seen) = recording(|_| Verdict::Pin);
    let mut table: CellTable<Owned> = CellTable::new(2, verdict);
    let cell = table.create(None, None).unwrap();
    let other = table.create(None, None).unwrap();

    table
        .enter(cell, |context| {
            context.alloc::<Number>(|writer| writer.value(1));
            context
                .alloc_into::<Number, Number>(other, &[], |writer, _| writer.value(2))
                .unwrap();
            context.store_successor(String::new());
        })
        .unwrap();
    assert!(seen.borrow().is_empty());
}
