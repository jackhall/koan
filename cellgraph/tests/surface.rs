//! The whole public surface, exercised from outside the crate.
//!
//! Everything the substrate means an embedder to reach is named here, and nothing else is
//! reachable to name: the reach model, the price queries, and the ring walk are all crate-private,
//! so an item that slips back out of `pub(crate)` shows up as an item this file never uses, and an
//! item that goes missing shows up as a compile error in it.
//!
//! The test is an integration test on purpose. A unit test lives inside the crate, where
//! `pub(crate)` is indistinguishable from `pub`; only a caller outside it sees the real surface.

use cellgraph::{
    Active, CellRef, CellTable, CreateError, CrossedOperand, Dormant, DropFree, EnterError, Erased,
    Handle, Operand, Prices, Reattachable, RedeemError, ReleaseAbsorption, ReleaseError,
    ReleaseTreeError, Resident, Stale, StepContext, TreeHandle, Verdict, Writer, reattachable,
};

/// The continuation family: a step's successor is a plain owned string, so nothing it holds lives
/// in a region.
struct Work;

/// Three value families, each borrowing its cell's region storage through the one lifetime the
/// [`reattachable`] contract allows.
struct Number;
struct Numbers;
struct Text;

reattachable!(
    Work => String,
    Number => &'r u32,
    Numbers => &'r [u32],
    Text => &'r str,
);

impl DropFree for Number {}
impl DropFree for Numbers {}
impl DropFree for Text {}

fn build_number<'r>(writer: Writer<'r>) -> &'r u32 {
    writer.value(7)
}

fn build_slice<'r>(writer: Writer<'r>) -> &'r [u32] {
    writer.slice(&[1, 2, 3])
}

fn build_text<'r>(writer: Writer<'r>) -> &'r str {
    writer.text("koan")
}

/// An embedder's own helper over carriers, which is the one reason [`Erased`] is nameable from
/// outside: the read door's `Copy` bound is on the erased form, so a caller that wants to be
/// generic over the value family has to write that bound too.
fn read_first<'s, 'b, V>(
    context: &'s StepContext<'b, Work>,
    carrier: &'s Dormant<'b, V>,
) -> V::At<'s>
where
    V: Reattachable + DropFree,
    Erased<V>: Copy,
{
    context.read(carrier).into_value()
}

/// The embedder's crossing verdict, taken at the table's construction and consulted once per
/// operand of every placement over operands.
///
/// Every field the substrate ships is named here — both prices, both tiers' occupancy, and the
/// destination's own size — and the threshold over them is the embedder's alone. This one copies
/// only where the embedder has said copying is cheap and the slab is under pressure.
fn weigh(prices: Prices) -> Verdict {
    let pressure = prices.occupied * 2 >= prices.cap
        || prices.records > 0
        || prices.retained_bytes > 0
        || prices.destination_bytes > 1 << 20;
    if pressure && prices.copy_bytes < prices.pin_bytes {
        Verdict::Copy
    } else {
        Verdict::Pin
    }
}

/// An operand the embedder is unwilling to copy: at a cost above anything a pin can price, the
/// verdict above always pins it.
fn pinned_operand<'a, 'b, V: Reattachable + DropFree>(
    carrier: &'a Dormant<'b, V>,
) -> Operand<'a, 'b, V> {
    Operand {
        carrier,
        copy_bytes: usize::MAX,
    }
}

fn name_redeem_error(error: RedeemError) -> &'static str {
    match error {
        RedeemError::Gone => "gone",
        RedeemError::Unheld => "unheld",
    }
}

/// The two refusals this file cannot provoke: a cell is entered and released only from outside a
/// step, so neither door ever finds one executing. The matches are exhaustive, so a variant that
/// went missing from either enum is a compile error here.
fn name_enter_error(error: EnterError) -> &'static str {
    match error {
        EnterError::Stale(_) => "stale",
        EnterError::AlreadyExecuting => "already executing",
    }
}

fn name_release_error(error: ReleaseError) -> &'static str {
    match error {
        ReleaseError::Stale(_) => "stale",
        ReleaseError::Executing => "executing",
    }
}

/// The tree pool's one enum of refusals, matched exhaustively for the same reason. `create_tree`
/// refuses only a stale parent — the pool takes no cap, so its error is the bare stale name — and
/// `enter` is the same door over both kinds. `release_tree` cannot meet an executing cell from
/// outside a step.
fn name_release_tree_error(error: ReleaseTreeError) -> &'static str {
    match error {
        ReleaseTreeError::Stale(_) => "stale",
        ReleaseTreeError::Executing => "executing",
    }
}

#[test]
fn every_public_door_answers_from_outside_the_crate() {
    let mut table: CellTable<Work> = CellTable::new(4, weigh);

    // Creation, with and without a parent, and with or without a continuation at birth.
    let root: Handle = table.create(None, Some(String::from("root"))).unwrap();
    let child = table.create(Some(root), None).unwrap();
    let doomed = table.create(Some(root), None).unwrap();
    assert_eq!(root.slot(), 0);
    assert_eq!(child.generation(), 0);
    assert!(table.is_live(child));
    assert!(!table.is_empty());

    table
        .release(doomed, ReleaseAbsorption::IntoHolder)
        .unwrap();

    let mut kept: Option<Resident<Number>> = None;
    let carried = table
        .enter(child, |context| {
            assert_eq!(context.cell(), CellRef::Slab(child));

            // Placement: into the running cell, and into a named one with an operand embedded.
            let number = context.alloc::<Number>(build_number);
            let numbers = context.alloc::<Numbers>(build_slice);
            let text = context.alloc::<Text>(build_text);
            let pushed = context
                .alloc_into::<Number, Number>(root, &[pinned_operand(&number)], |writer, views| {
                    match views[0] {
                        CrossedOperand::Pinned(value) => writer.value(*value + 1),
                        CrossedOperand::Copied(value) => writer.value(*value + 1),
                    }
                })
                .unwrap();
            // The same door under the other verdict: an operand the embedder prices cheap to copy
            // crosses severed, so the only thing the build can do with it is copy it in deeply.
            let copied = context
                .alloc_into::<Numbers, Number>(
                    root,
                    &[Operand {
                        carrier: &number,
                        copy_bytes: 0,
                    }],
                    |writer, views| match views[0] {
                        CrossedOperand::Copied(value) => writer.slice(&[*value, *value]),
                        CrossedOperand::Pinned(value) => writer.slice(&[*value, *value]),
                    },
                )
                .unwrap();
            assert_eq!(read_first(context, &copied), &[7, 7]);

            // A bare hold, and the refusal a handle kept past a declared death earns.
            context.hold(root).unwrap();
            assert_eq!(context.hold(doomed).unwrap_err().name(), doomed);

            // Reading, by copy and by move, directly and through the embedder's own helper.
            assert_eq!(*context.read(&number).value(), 7);
            assert_eq!(read_first(context, &numbers), &[1, 2, 3]);
            assert_eq!(read_first(context, &text), "koan");
            assert_eq!(*read_first(context, &pushed), 8);

            // The continuation slot: this cell was born without one, and leaves with one that
            // captures a value living in another cell's region.
            assert!(context.continuation().is_none());
            context.store_successor(String::from("plain"));
            context.store_successor_capturing(&[pinned_operand(&pushed)], |_writer, views| {
                match views[0] {
                    CrossedOperand::Pinned(value) => value.to_string(),
                    CrossedOperand::Copied(value) => value.to_string(),
                }
            });

            // Put a value to rest, so it outlives the step that built it. What comes back is
            // opaque: a resident has no read, and the redeem door is its only exit.
            kept = Some(context.keep(pushed));

            String::from("done")
        })
        .unwrap();
    assert_eq!(carried, "done");

    // The value kept in the last step redeems in this one: the child holds root, whose region the
    // value lives in, so the door hands it back with reach derived from the table.
    let kept = kept.unwrap();
    let redeemed = table
        .enter(child, |context| {
            *context.read(&context.redeem(kept).unwrap()).value()
        })
        .unwrap();
    assert_eq!(redeemed, 8);

    // A cell with no claim on the home is refused, and the refusal says which of the two it is.
    let refused = table
        .enter(root, |context| context.redeem(kept).map(|_| ()))
        .unwrap();
    assert!(refused.is_ok(), "the home cell redeems its own resident");
    let bystander = table.create(None, None).unwrap();
    let error = table
        .enter(bystander, |context| match context.redeem(kept) {
            Err(error) => error,
            Ok(_) => panic!("a cell with no claim on the home must be refused"),
        })
        .unwrap();
    assert_eq!(name_redeem_error(error), "unheld");
    table
        .release(bystander, ReleaseAbsorption::IntoHolder)
        .unwrap();

    // The successor comes back re-anchored at the next step's brand.
    let echoed = table
        .enter(child, |context| {
            context.continuation().map(Active::into_value)
        })
        .unwrap();
    assert_eq!(echoed.as_deref(), Some("8"));

    // The cell born with a continuation still has it.
    let born_with = table
        .enter(root, |context| {
            context.continuation().map(Active::into_value)
        })
        .unwrap();
    assert_eq!(born_with.as_deref(), Some("root"));

    // Both dispositions of a death: fold into a unique holder, or seal rather than merge.
    table.release(child, ReleaseAbsorption::IntoHolder).unwrap();
    table.release(root, ReleaseAbsorption::Refused).unwrap();
    assert!(!table.is_live(root));
}

#[test]
fn the_refusals_hand_back_the_handle_that_went_stale() {
    let mut full: CellTable<Work> = CellTable::new(1, weigh);
    let taken = full.create(None, None).unwrap();
    assert_eq!(full.create(None, None), Err(CreateError::SlabFull));

    // Every door refuses a handle kept past the death the embedder declared itself, and every
    // refusal hands the handle back rather than swallowing it.
    full.release(taken, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(full.is_empty());

    let Err(CreateError::StaleParent(stale)) = full.create(Some(taken), None) else {
        panic!("a dead parent must refuse");
    };
    let stale: Stale<Handle> = stale;
    assert_eq!(stale.name(), taken);

    let Err(error) = full.enter(taken, |_| ()) else {
        panic!("a dead cell must refuse the step");
    };
    assert_eq!(name_enter_error(error), "stale");

    let Err(error) = full.release(taken, ReleaseAbsorption::Refused) else {
        panic!("a second release names a death already declared");
    };
    assert_eq!(name_release_error(error), "stale");
}

#[test]
fn the_tree_pool_answers_from_outside_the_crate() {
    let mut table: CellTable<Work> = CellTable::new(1, weigh);
    let root: Handle = table.create(None, None).unwrap();
    assert_eq!(table.create(None, None), Err(CreateError::SlabFull));

    // The pool takes no cap: a chain deeper than the slab is ordinary, and none of it is a slot.
    let outer: TreeHandle = table.create_tree(root, None).unwrap();
    let inner = table
        .create_tree(outer, Some(String::from("resume")))
        .unwrap();
    assert_eq!(outer.index(), 0);
    assert_eq!(inner.generation(), 0);
    assert!(table.is_live(inner));

    let mut kept: Option<Resident<Number>> = None;
    let carried = table
        .enter(inner, |context| {
            assert_eq!(context.cell(), CellRef::Tree(inner));
            let value = context.alloc::<Number>(build_number);

            // Into the cell's own tree parent: an upward pin, priced at the splice and pledging
            // this cell's bump to the parent's bundle.
            let up = context
                .alloc_into::<Number, Number>(outer, &[pinned_operand(&value)], |writer, views| {
                    match views[0] {
                        CrossedOperand::Pinned(value) => writer.value(*value + 1),
                        CrossedOperand::Copied(value) => writer.value(*value + 1),
                    }
                })
                .unwrap();
            kept = Some(context.keep(up));

            // A carrier homed in a tree cell reaches its root, so the root is what a hold from
            // inside the subtree lands on.
            context.hold(root).unwrap();
            let resumed = context.continuation().map(Active::into_value);
            context.store_successor(String::from("done"));
            resumed
        })
        .unwrap();
    assert_eq!(carried.as_deref(), Some("resume"));

    // The value kept in the parent redeems from anywhere under the same root.
    let redeemed = table
        .enter(root, |context| {
            *context
                .read(&context.redeem(kept.unwrap()).unwrap())
                .value()
        })
        .unwrap();
    assert_eq!(redeemed, 8);

    // Release takes no argument: where the bytes go was settled at the placement door.
    table.release_tree(inner).unwrap();
    assert!(!table.is_live(inner));

    let Err(error) = table.release_tree(inner) else {
        panic!("a second release names a death already declared");
    };
    assert_eq!(name_release_tree_error(error), "stale");
    let stale: Stale<TreeHandle> = match error {
        ReleaseTreeError::Stale(stale) => stale,
        ReleaseTreeError::Executing => unreachable!("the cell is not executing"),
    };
    assert_eq!(stale.name(), inner);

    let Err(error) = table.enter(inner, |_| ()) else {
        panic!("a dead tree cell must refuse the step");
    };
    assert_eq!(name_enter_error(error), "stale");
    let EnterError::Stale(stale) = error else {
        unreachable!("the cell is not executing")
    };
    let stale: Stale<CellRef> = stale;
    assert_eq!(stale.name(), CellRef::Tree(inner));

    let Err(stale) = table.create_tree(inner, None) else {
        panic!("a dead tree parent must refuse");
    };
    let stale: Stale<CellRef> = stale;
    assert_eq!(stale.name(), CellRef::Tree(inner));

    table.release_tree(outer).unwrap();
    table.release(root, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(table.is_empty());
}
