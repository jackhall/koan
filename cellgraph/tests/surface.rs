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
    Absorption, CellTable, CreateError, DropFree, EnterError, Erased, Handle, Opened, Reattachable,
    ReleaseError, Sealed, StaleHandle, StepContext, Writer, reattachable,
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
    carrier: &'s Sealed<'b, V>,
) -> V::At<'s>
where
    V: Reattachable + DropFree,
    Erased<V>: Copy,
{
    context.read(carrier).into_value()
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

#[test]
fn every_public_door_answers_from_outside_the_crate() {
    let mut table: CellTable<Work> = CellTable::new(4);

    // Creation, with and without a parent, and with or without a continuation at birth.
    let root: Handle = table.create(None, Some(String::from("root"))).unwrap();
    let child = table.create(Some(root), None).unwrap();
    let doomed = table.create(Some(root), None).unwrap();
    assert_eq!(root.slot(), 0);
    assert_eq!(child.generation(), 0);
    assert!(table.is_live(child));
    assert!(!table.is_empty());

    table.release(doomed, Absorption::IntoHolder).unwrap();

    let carried = table
        .enter(child, |context| {
            assert_eq!(context.handle(), child);

            // Placement: into the running cell, and into a named one with an operand embedded.
            let number = context.alloc::<Number>(build_number);
            let numbers = context.alloc::<Numbers>(build_slice);
            let text = context.alloc::<Text>(build_text);
            let pushed = context
                .alloc_into::<Number, Number>(root, &[&number], |writer, views| {
                    writer.value(*views[0] + 1)
                })
                .unwrap();

            // A bare hold, and the refusal a handle kept past a declared death earns.
            context.hold(root).unwrap();
            assert_eq!(context.hold(doomed).unwrap_err().handle(), doomed);

            // Reading, by copy and by move, directly and through the embedder's own helper.
            assert_eq!(*context.read(&number).value(), 7);
            assert_eq!(read_first(context, &numbers), &[1, 2, 3]);
            assert_eq!(read_first(context, &text), "koan");
            assert_eq!(*read_first(context, &pushed), 8);

            // The continuation slot: this cell was born without one, and leaves with one that
            // captures a value living in another cell's region.
            assert!(context.continuation().is_none());
            context.store_successor(String::from("plain"));
            context.store_successor_capturing(&[&pushed], |_writer, views| views[0].to_string());

            String::from("done")
        })
        .unwrap();
    assert_eq!(carried, "done");

    // The successor comes back re-anchored at the next step's brand.
    let echoed = table
        .enter(child, |context| {
            context.continuation().map(Opened::into_value)
        })
        .unwrap();
    assert_eq!(echoed.as_deref(), Some("8"));

    // The cell born with a continuation still has it.
    let born_with = table
        .enter(root, |context| {
            context.continuation().map(Opened::into_value)
        })
        .unwrap();
    assert_eq!(born_with.as_deref(), Some("root"));

    // Both dispositions of a death: fold into a unique holder, or seal rather than merge.
    table.release(child, Absorption::IntoHolder).unwrap();
    table.release(root, Absorption::Refused).unwrap();
    assert!(!table.is_live(root));
}

#[test]
fn the_refusals_hand_back_the_handle_that_went_stale() {
    let mut full: CellTable<Work> = CellTable::new(1);
    let taken = full.create(None, None).unwrap();
    assert_eq!(full.create(None, None), Err(CreateError::SlabFull));

    // Every door refuses a handle kept past the death the embedder declared itself, and every
    // refusal hands the handle back rather than swallowing it.
    full.release(taken, Absorption::IntoHolder).unwrap();
    assert!(full.is_empty());

    let Err(CreateError::StaleParent(stale)) = full.create(Some(taken), None) else {
        panic!("a dead parent must refuse");
    };
    let stale: StaleHandle = stale;
    assert_eq!(stale.handle(), taken);

    let Err(error) = full.enter(taken, |_| ()) else {
        panic!("a dead cell must refuse the step");
    };
    assert_eq!(name_enter_error(error), "stale");

    let Err(error) = full.release(taken, Absorption::Refused) else {
        panic!("a second release names a death already declared");
    };
    assert_eq!(name_release_error(error), "stale");
}
