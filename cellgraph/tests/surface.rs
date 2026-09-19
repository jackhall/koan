//! The whole public surface, exercised from outside the crate.
//!
//! Everything the substrate means an embedder to reach is named here, and nothing else is
//! reachable to name: the reach model, the price queries, and the ring walk are all crate-private,
//! so an item that slips back out of `pub(crate)` shows up as an item this file never uses, and an
//! item that goes missing shows up as a compile error in it.
//!
//! The test is an integration test on purpose. A unit test lives inside the crate, where
//! `pub(crate)` is indistinguishable from `pub`; only a caller outside it sees the real surface.

use std::marker::PhantomData;

use cellgraph::{
    Active, CellGraph, CellHandle, Config, CreateError, CrossedOperand, DeliverError, Delivered,
    Delivery, Dormant, DropFree, EnterError, Erased, NoDelivery, NoScratch, Operand, Prices, Prose,
    Ready, Reattachable, Receipt, ReceiptError, RedeemError, RegisterError, ReleaseAbsorption,
    ReleaseError, ReleaseTenantError, ReleaseTreeError, Run, SlabHandle, Stale, StepContext,
    TenantHandle, ThinRun, TreeHandle, Verdict, Writer, reattachable,
};

/// The continuation family: a step's successor is a plain owned string, so nothing it holds lives
/// in a region.
struct Work;

/// A second continuation family, borrowing, so a successor can capture a reference at `'here` and
/// come back re-anchored at the next step's.
struct Resumed;

/// A third continuation family, naming the graph lifetime itself, so a successor can capture a
/// borrow of storage the embedder owns outside the graph.
struct Script<'graph>(PhantomData<&'graph str>);

/// Three value families, each borrowing its cell's region storage through `'cell`, the lifetime the
/// [`reattachable`] contract retypes.
struct Number;
struct Numbers;
struct Text;

/// A value nesting a borrow of storage the embedder owns outside the graph under a borrow of a
/// cell's region — the shape `'graph` exists for.
#[derive(Clone, Copy)]
struct Entry<'graph, 'cell> {
    program: &'graph str,
    count: &'cell u32,
}

/// The family of an [`Entry`] behind a region borrow.
struct Listing;

reattachable!(
    Work => String,
    Resumed => &'cell u32,
    Script<'graph> => &'graph str,
    Number => &'cell u32,
    Numbers => &'cell [u32],
    Text => &'cell str,
    Listing => &'cell Entry<'graph, 'cell>,
);

impl DropFree for Number {}
impl DropFree for Numbers {}
impl DropFree for Text {}
impl DropFree for Listing {}

/// The writer's one run verb, at length one — the shape an embedder derives a single-value write
/// from, since the substrate ships no such verb.
fn one<'cell, T>(writer: Writer<'cell>, value: T) -> &'cell T {
    let mut value = Some(value);
    &writer.fill(1, |_| value.take().expect("a run of one fills once"))[0]
}

fn build_number<'cell>(writer: Writer<'cell>) -> &'cell u32 {
    one(writer, 7)
}

fn build_slice<'cell>(writer: Writer<'cell>) -> &'cell [u32] {
    writer.fill(3, |index| index as u32 + 1)
}

/// The run behind one thin pointer: a handle a word wide, whose length rides at the front of the
/// allocation instead of beside the pointer.
fn build_thin_run<'cell>(writer: Writer<'cell>) -> &'cell [u32] {
    let run: ThinRun<'cell, u32> = writer.thin_run(4, |index| index as u32 * 2);
    assert!(!run.is_empty() && run.len() == 4);
    run.as_slice()
}

fn build_text<'cell>(writer: Writer<'cell>) -> &'cell str {
    writer.text("koan")
}

/// The producer-decided run: a filter settles the width only once the elements exist, so there is
/// no length to hand `fill`.
fn build_run<'cell>(writer: Writer<'cell>) -> &'cell [u32] {
    let mut run: Run<'cell, u32> = writer.run();
    run.extend((1..8u32).filter(|value| value % 3 == 0));
    run.push(99);
    assert!(!run.is_empty() && run.len() == 3);
    run.finish()
}

/// The same shape for text: a rendering whose length no caller knows up front, formatted straight
/// into the region.
fn build_prose<'cell>(writer: Writer<'cell>) -> &'cell str {
    use std::fmt::Write;

    let mut prose: Prose<'cell> = writer.prose();
    for value in build_run(writer) {
        write!(prose, "{value};").expect("a region sink never fails");
    }
    prose.finish()
}

/// An embedder's own helper over carriers, which is the one reason [`Erased`] is nameable from
/// outside: the read door's `Copy` bound is on the erased form, so a caller that wants to be
/// generic over the value family has to write that bound too.
fn read_first<'graph, 'cell, 'step, V>(
    context: &'cell StepContext<'graph, 'step, '_, '_, Work>,
    carrier: &'cell Ready<'graph, 'step, V>,
) -> V::At<'cell>
where
    V: Reattachable<'graph> + DropFree,
    Erased<'graph, V>: Copy,
{
    context.read(carrier).into_value()
}

/// The embedder's crossing verdict, taken at the graph's construction and consulted once per
/// operand of every placement over operands.
///
/// Every field the substrate ships is named here — both prices, both tiers' occupancy, and the
/// destination's own size — and the threshold over them is the embedder's alone. This one copies
/// only where the embedder has said copying is cheap and the slab is under pressure.
fn weigh(prices: Prices) -> Verdict {
    let pressure = prices.occupied * 2 >= prices.cap
        || prices.sealed_cells > 0
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
fn pinned_operand<'graph, 'a, 'step, V: Reattachable<'graph> + DropFree>(
    carrier: &'a Ready<'graph, 'step, V>,
) -> Operand<'graph, 'a, 'step, V> {
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

/// Every refusal `release_tenant` can give, matched by name for the same reason.
fn name_release_tenant_error(error: ReleaseTenantError) -> &'static str {
    match error {
        ReleaseTenantError::Stale(_) => "stale",
        ReleaseTenantError::Executing => "executing",
    }
}

/// The scratch state's family, named apart from the continuation's so the second parameter of the
/// graph's type is spelled from outside the crate. It is of the two-lifetime contract, and this
/// form holds scratch alone.
struct Worklist;
reattachable!(both Worklist => &'scratch [&'scratch u32]);

/// The bundle of the two families a graph's cells deliver: a note built in the consumer's own
/// scratch habitat, and a `Number` carrier filed at rest.
struct Push;

impl<'graph> Delivery<'graph> for Push {
    type Scratch = Number;
    type Carrier = Number;
}

/// Which arm a receipt came back on — a `Receipt` holds a family's form, so nothing bounds it
/// `Debug`.
fn name_receipt<'graph, D: Delivery<'graph>>(
    receipt: &Receipt<'graph, '_, '_, D, 1>,
) -> &'static str {
    match receipt {
        Receipt::Empty => "empty",
        Receipt::Value(_) => "value",
        Receipt::Carrier(Ok(_)) => "carrier",
        Receipt::Carrier(Err(_)) => "refused",
    }
}

#[test]
fn the_delivery_doors_answer_from_outside_the_crate() {
    // The default bundle is one an embedder can name, and a graph over it delivers nothing.
    let mut plain: CellGraph<'static, Work, NoScratch, NoDelivery> = CellGraph::new(1, weigh);
    let alone = plain.create(None).unwrap();
    plain
        .enter(alone, |context| assert_eq!(context.receipt_count(), None))
        .unwrap();
    plain.release(alone, ReleaseAbsorption::IntoHolder).unwrap();

    let mut graph: CellGraph<'static, Work, NoScratch, Push> = CellGraph::new(2, weigh);
    let consumer = graph.create(None).unwrap();
    let producer = graph.create(None).unwrap();

    // The consumer parks on a two-slot run.
    graph
        .enter(consumer, |context| {
            assert!(matches!(context.receipt(0), Err(ReceiptError::NoRun)));
            context.register_receipts(2).unwrap();
        })
        .unwrap();

    // One slot takes a note built in the consumer's scratch habitat, the other a carrier the
    // producer placed into the consumer's region and put to rest.
    graph
        .enter(producer, |context| {
            assert_eq!(
                context
                    .deliver_scratch(consumer, 0, |writer, _| Active::new(build_number(writer)))
                    .unwrap(),
                Delivered::Outstanding
            );
            let value: Ready<'static, '_, Number> = context.lift(one(context.writer(), 7u32));
            let placed = context
                .alloc_into::<Number, Number>(
                    consumer,
                    &[pinned_operand(&value)],
                    |writer, views| {
                        Active::new(match &views[0] {
                            CrossedOperand::Pinned(value) => *value,
                            CrossedOperand::Copied(value) => one(writer, **value),
                        })
                    },
                )
                .unwrap();
            let carrier: Dormant<'static, Number> = context.keep(placed);
            assert_eq!(
                context.deliver_carrier(consumer, 1, carrier).unwrap(),
                Delivered::Complete
            );
            // Both refusals of a filled run, from outside the crate.
            let refused = context
                .deliver_scratch(consumer, 1, |writer, _| Active::new(build_number(writer)))
                .unwrap_err();
            assert_eq!(refused, DeliverError::Filled);
            assert_eq!(
                context
                    .deliver_scratch(consumer, 2, |writer, _| Active::new(build_number(writer)))
                    .unwrap_err(),
                DeliverError::OutOfRange
            );
        })
        .unwrap();

    let read = graph
        .enter(consumer, |context| {
            assert_eq!(context.receipt_count(), Some(2));
            assert_eq!(
                context.register_receipts(1),
                Err(RegisterError::Undrained),
                "a registration over an undrained run is refused"
            );
            let note = context.receipt(0).unwrap();
            assert_eq!(name_receipt(&note), "value");
            let Receipt::Value(note) = note else {
                panic!("the producer filed a note");
            };
            let filed = context.receipt(1).unwrap();
            assert_eq!(name_receipt(&filed), "carrier");
            let Receipt::Carrier(Ok(carrier)) = filed else {
                panic!("the producer filed a carrier the consumer keeps");
            };
            assert!(matches!(context.receipt(1).unwrap(), Receipt::Empty));
            *note + *context.read(&carrier).value()
        })
        .unwrap();
    assert_eq!(read, 14);

    graph.release(producer, ReleaseAbsorption::Refused).unwrap();
    graph
        .release(consumer, ReleaseAbsorption::IntoHolder)
        .unwrap();
    assert!(graph.is_empty());
}

#[test]
fn a_tenant_and_the_scratch_habitat_answer_from_outside_the_crate() {
    let mut graph: CellGraph<'static, Resumed, Worklist> = CellGraph::new(1, weigh);
    let host: SlabHandle = graph.create(None).unwrap();
    let tenant: TenantHandle = graph.create_tenant(host, None).unwrap();
    assert_eq!((tenant.index(), tenant.generation()), (0, 0));
    assert!(graph.is_live(tenant));

    // A tenant step writes its host's region at `'here` and its host's scratch at `'scratch`,
    // and parks something in each slot.
    let ran_in = graph
        .enter(tenant, |context| {
            let kept = one(context.writer(), 41u32);
            let worklist: &[&u32] = context.scratch_writer().fill(2, |_| kept);
            context.store_successor(kept);
            context.store_scratch_state(worklist);
            context.cell()
        })
        .unwrap();
    assert_eq!(ran_in, CellHandle::Tenant(tenant));
    assert_eq!(CellHandle::from(tenant), ran_in);

    let read = graph
        .enter(tenant, |context| {
            let kept = context.continuation().expect("the continuation was stored");
            let worklist = context
                .scratch_state()
                .expect("the scratch state was stored");
            *kept + *worklist[1]
        })
        .unwrap();
    assert_eq!(read, 82);

    // A tenant named as a host means its host, and a released host waits on its tenants.
    let second = graph.create_tenant(tenant, None).unwrap();
    graph.release(host, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(!graph.is_empty());
    graph.release_tenant(tenant).unwrap();
    let Err(error) = graph.release_tenant(tenant) else {
        panic!("a second release names a death already declared");
    };
    assert_eq!(name_release_tenant_error(error), "stale");
    let ReleaseTenantError::Stale(stale) = error else {
        panic!("a stale release names the handle it refused");
    };
    let stale: Stale<TenantHandle> = stale;
    assert_eq!(stale.name(), tenant);
    let widened: Stale<CellHandle> = stale.into();
    assert_eq!(widened.name(), CellHandle::Tenant(tenant));

    graph.release_tenant(second).unwrap();
    assert!(graph.is_empty());
}

#[test]
fn every_public_door_answers_from_outside_the_crate() {
    let mut graph: CellGraph<'static, Work> = CellGraph::new(4, weigh);

    // Creation, with and without a continuation at birth.
    let root: SlabHandle = graph.create(Some(String::from("root"))).unwrap();
    let child = graph.create(None).unwrap();
    let doomed = graph.create(None).unwrap();
    assert_eq!(root.slot(), 0);
    assert_eq!(child.generation(), 0);
    assert!(graph.is_live(child));
    assert!(!graph.is_empty());

    graph
        .release(doomed, ReleaseAbsorption::IntoHolder)
        .unwrap();

    let mut kept: Option<Dormant<'static, Number>> = None;
    let carried = graph
        .enter(child, |context| {
            assert_eq!(context.cell(), CellHandle::Slab(child));

            // The own-region write: the cell's own writer, at `'here`, then the bridge that
            // makes each value a carrier.
            let number = context.lift::<Number>(build_number(context.writer()));
            let numbers = context.lift::<Numbers>(build_slice(context.writer()));
            let text = context.lift::<Text>(build_text(context.writer()));
            let thin = context.lift::<Numbers>(build_thin_run(context.writer()));
            assert_eq!(read_first(context, &thin), &[0, 2, 4, 6]);
            let filtered = context.lift::<Numbers>(build_run(context.writer()));
            let rendered = context.lift::<Text>(build_prose(context.writer()));
            assert_eq!(read_first(context, &filtered), &[3, 6, 99]);
            assert_eq!(read_first(context, &rendered), "3;6;99;");
            let pushed = context
                .alloc_into::<Number, Number>(root, &[pinned_operand(&number)], |writer, views| {
                    Active::new(match views[0] {
                        CrossedOperand::Pinned(value) => one(writer, *value + 1),
                        CrossedOperand::Copied(value) => one(writer, *value + 1),
                    })
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
                    |writer, views| {
                        Active::new(match views[0] {
                            CrossedOperand::Copied(value) => writer.fill(2, |_| *value),
                            CrossedOperand::Pinned(value) => writer.fill(2, |_| *value),
                        })
                    },
                )
                .unwrap();
            assert_eq!(read_first(context, &copied), &[7, 7]);

            // A bare hold, and the refusal a handle kept past a declared death earns.
            context.hold(root).unwrap();
            assert_eq!(context.hold(doomed).unwrap_err().name(), doomed);

            // The own-cell crossing: two operands of the same carrier, one the embedder refuses to
            // copy and one it prices free. A pinned view arrives at `'here` and may leave
            // the build, which is what the pin bought; a copied one is severed and can only be
            // read or written again.
            let (here, severed): (&u32, u32) = context.alloc_here(
                &[
                    pinned_operand(&pushed),
                    Operand {
                        carrier: &pushed,
                        copy_bytes: 0,
                    },
                ],
                |writer, views| {
                    let here = match views[0] {
                        CrossedOperand::Pinned(value) => value,
                        CrossedOperand::Copied(value) => one(writer, *value),
                    };
                    let severed = match views[1] {
                        CrossedOperand::Pinned(value) | CrossedOperand::Copied(value) => *value,
                    };
                    (here, severed)
                },
            );
            assert_eq!(*here, 8);
            assert_eq!(severed, 8);

            // Reading, by copy and by move, directly and through the embedder's own helper.
            let opened: Active<'static, '_, Number> = context.read(&number);
            assert_eq!(*opened.value(), 7);
            assert_eq!(read_first(context, &numbers), &[1, 2, 3]);
            assert_eq!(read_first(context, &text), "koan");
            assert_eq!(*read_first(context, &pushed), 8);

            // The continuation slot: this cell was born without one, and leaves with one built
            // over the reference the own-cell crossing pinned into its holds. One door, and
            // storing twice keeps the last.
            assert!(context.continuation().is_none());
            context.store_successor(String::from("plain"));
            context.store_successor(here.to_string());

            // Put a value to rest, so it outlives the step that built it. What comes back is
            // opaque: a dormant carrier has no read, and the redeem door is its only exit.
            kept = Some(context.keep(pushed));

            String::from("done")
        })
        .unwrap();
    assert_eq!(carried, "done");

    // The value kept in the last step redeems in this one: the child's pin row names root, whose
    // region the value lives in, so the door hands it back with reach derived from the reach table.
    let kept = kept.unwrap();
    let redeemed = graph
        .enter(child, |context| {
            *context.read(&context.redeem(kept).unwrap()).value()
        })
        .unwrap();
    assert_eq!(redeemed, 8);

    // A cell with no claim on the home is refused, and the refusal says which of the two it is.
    let refused = graph
        .enter(root, |context| context.redeem(kept).map(|_| ()))
        .unwrap();
    assert!(
        refused.is_ok(),
        "the home cell redeems its own dormant carrier"
    );
    let bystander = graph.create(None).unwrap();
    let error = graph
        .enter(bystander, |context| match context.redeem(kept) {
            Err(error) => error,
            Ok(_) => panic!("a cell with no claim on the home must be refused"),
        })
        .unwrap();
    assert_eq!(name_redeem_error(error), "unheld");
    graph
        .release(bystander, ReleaseAbsorption::IntoHolder)
        .unwrap();

    // The successor comes back re-anchored at the next step's `'here`.
    let echoed = graph
        .enter(child, |context| context.continuation())
        .unwrap();
    assert_eq!(echoed.as_deref(), Some("8"));

    // The cell born with a continuation still has it.
    let born_with = graph.enter(root, |context| context.continuation()).unwrap();
    assert_eq!(born_with.as_deref(), Some("root"));

    // Both dispositions of a death: fold into a unique holder, or seal rather than merge.
    graph.release(child, ReleaseAbsorption::IntoHolder).unwrap();
    graph.release(root, ReleaseAbsorption::Refused).unwrap();
    assert!(!graph.is_live(root));
}

#[test]
fn a_successor_captures_the_cell_brand_and_comes_back_re_anchored() {
    let mut graph: CellGraph<'static, Resumed> = CellGraph::new(2, weigh);
    let cell = graph.create(None).unwrap();
    let other = graph.create(None).unwrap();

    // A capture out of the cell's own region: written through the cell's writer, stored through
    // the one successor door, and handed back at the next step's `'here`.
    graph
        .enter(cell, |context| {
            context.store_successor(one(context.writer(), 11));
        })
        .unwrap();
    let own = graph
        .enter(cell, |context| *context.continuation().unwrap())
        .unwrap();
    assert_eq!(own, 11);

    // And a capture homed elsewhere: the own-cell crossing prices it and mints its reach into this
    // cell's holds, which is what makes the borrow nameable at `'here` at all. The store
    // itself prices nothing.
    graph
        .enter(cell, |context| {
            let foreign = context
                .alloc_into::<Number, Number>(other, &[], |writer, _| Active::new(one(writer, 23)))
                .unwrap();
            let held = context.alloc_here(&[pinned_operand(&foreign)], |writer, views| match views
                [0]
            {
                CrossedOperand::Pinned(value) => value,
                CrossedOperand::Copied(value) => one(writer, *value),
            });
            context.store_successor(held);
        })
        .unwrap();

    // The held cell dies and seals rather than reclaiming, so the capture still reads it.
    graph.release(other, ReleaseAbsorption::Refused).unwrap();
    let across = graph
        .enter(cell, |context| *context.continuation().unwrap())
        .unwrap();
    assert_eq!(across, 23);
}

#[test]
fn a_graph_is_built_with_the_spare_lists_bound_chosen() {
    // The default is what `new` builds with, and every field is the embedder's to set.
    let defaults = Config::new(2);
    assert_eq!((defaults.cap, defaults.spare_proportion), (2, 2));
    let config = Config {
        spare_proportion: 0,
        spare_window_shift: 3,
        ..defaults
    };
    let mut graph: CellGraph<'static, Work> = CellGraph::with_config(config, weigh);
    let cell = graph.create(None).unwrap();
    graph.release(cell, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(graph.is_empty());
}

#[test]
fn the_refusals_hand_back_the_handle_that_went_stale() {
    let mut full: CellGraph<'static, Work> = CellGraph::new(1, weigh);
    let taken = full.create(None).unwrap();
    assert_eq!(full.create(None), Err(CreateError::SlabFull));

    // Every door refuses a handle kept past the death the embedder declared itself, and every
    // refusal hands the handle back rather than swallowing it.
    full.release(taken, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(full.is_empty());

    let Err(error) = full.enter(taken, |_| ()) else {
        panic!("a dead cell must refuse the step");
    };
    assert_eq!(name_enter_error(error), "stale");

    let Err(error) = full.release(taken, ReleaseAbsorption::Refused) else {
        panic!("a second release names a death already declared");
    };
    assert_eq!(name_release_error(error), "stale");
    let ReleaseError::Stale(stale) = error else {
        panic!("a stale release names the handle it refused");
    };
    let stale: Stale<SlabHandle> = stale;
    assert_eq!(stale.name(), taken);
}

#[test]
fn the_tree_pool_answers_from_outside_the_crate() {
    let mut graph: CellGraph<'static, Work> = CellGraph::new(1, weigh);
    let root: SlabHandle = graph.create(None).unwrap();
    assert_eq!(graph.create(None), Err(CreateError::SlabFull));

    // The pool takes no cap: a chain deeper than the slab is ordinary, and none of it is a slot.
    let outer: TreeHandle = graph.create_tree(root, None).unwrap();
    let inner = graph
        .create_tree(outer, Some(String::from("resume")))
        .unwrap();
    assert_eq!(outer.index(), 0);
    assert_eq!(inner.generation(), 0);
    assert!(graph.is_live(inner));

    let mut kept: Option<Dormant<'static, Number>> = None;
    let carried = graph
        .enter(inner, |context| {
            assert_eq!(context.cell(), CellHandle::Tree(inner));
            let value = context.lift::<Number>(build_number(context.writer()));

            // Into the cell's own tree parent: an upward pin, priced at the splice and pledging
            // this cell's bump to the parent's bundle.
            let up = context
                .alloc_into::<Number, Number>(outer, &[pinned_operand(&value)], |writer, views| {
                    Active::new(match views[0] {
                        CrossedOperand::Pinned(value) => one(writer, *value + 1),
                        CrossedOperand::Copied(value) => one(writer, *value + 1),
                    })
                })
                .unwrap();
            kept = Some(context.keep(up));

            // A carrier homed in a tree cell reaches its root, so the root is what a hold from
            // inside the subtree lands on.
            context.hold(root).unwrap();
            let resumed = context.continuation();
            context.store_successor(String::from("done"));
            resumed
        })
        .unwrap();
    assert_eq!(carried.as_deref(), Some("resume"));

    // The value kept in the parent redeems from anywhere under the same root.
    let redeemed = graph
        .enter(root, |context| {
            *context
                .read(&context.redeem(kept.unwrap()).unwrap())
                .value()
        })
        .unwrap();
    assert_eq!(redeemed, 8);

    // Release takes no argument: where the bytes go was settled at the placement door.
    graph.release_tree(inner).unwrap();
    assert!(!graph.is_live(inner));

    let Err(error) = graph.release_tree(inner) else {
        panic!("a second release names a death already declared");
    };
    assert_eq!(name_release_tree_error(error), "stale");
    let stale: Stale<TreeHandle> = match error {
        ReleaseTreeError::Stale(stale) => stale,
        ReleaseTreeError::Executing => unreachable!("the cell is not executing"),
    };
    assert_eq!(stale.name(), inner);

    let Err(error) = graph.enter(inner, |_| ()) else {
        panic!("a dead tree cell must refuse the step");
    };
    assert_eq!(name_enter_error(error), "stale");
    let EnterError::Stale(stale) = error else {
        unreachable!("the cell is not executing")
    };
    let stale: Stale<CellHandle> = stale;
    assert_eq!(stale.name(), CellHandle::Tree(inner));

    let Err(stale) = graph.create_tree(inner, None) else {
        panic!("a dead tree parent must refuse");
    };
    let stale: Stale<CellHandle> = stale;
    assert_eq!(stale.name(), CellHandle::Tree(inner));

    graph.release_tree(outer).unwrap();
    graph.release(root, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(graph.is_empty());
}

#[test]
fn a_graph_borrow_crosses_a_forced_copy_verbatim() {
    // Storage the embedder owns outside the graph, which the graph may borrow but not outlive.
    let program = String::from("program text");
    let mut graph: CellGraph<'_, Work> = CellGraph::new(1, weigh);
    let root = graph.create(None).unwrap();
    let left = graph.create_tree(root, None).unwrap();
    let right = graph.create_tree(root, None).unwrap();
    graph
        .enter(left, |context| {
            let count = one(context.writer(), 41);
            let entry = one(
                context.writer(),
                Entry {
                    program: &program,
                    count,
                },
            );
            let source = context.lift::<Listing>(entry);
            // A sibling is neither on the home's chain nor under it: the crossing is a forced copy.
            let copied = context
                .alloc_into::<Listing, Listing>(
                    right,
                    &[pinned_operand(&source)],
                    |writer, views| {
                        let CrossedOperand::Copied(entry) = views[0] else {
                            panic!("a sibling crossing is a forced copy");
                        };
                        // The `'graph` borrow embeds as it is; only the region part is written again.
                        Active::new(one(
                            writer,
                            Entry {
                                program: entry.program,
                                count: one(writer, *entry.count),
                            },
                        ))
                    },
                )
                .unwrap();
            let read = read_first(context, &copied);
            assert!(std::ptr::eq(read.program, program.as_str()));
            assert!(!std::ptr::eq(read.count, count));
            assert_eq!(*read.count, 41);
        })
        .unwrap();
    graph.release_tree(right).unwrap();
    graph.release_tree(left).unwrap();
    graph.release(root, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(graph.is_empty());
}

#[test]
fn a_graph_borrow_is_captured_kept_and_redeemed_after_its_home_is_released() {
    let program = String::from("program text");
    let mut graph: CellGraph<'_, Script<'_>> = CellGraph::new(2, weigh);
    let producer = graph.create(None).unwrap();
    let consumer = graph.create(None).unwrap();
    let dormant: Dormant<'_, Listing> = graph
        .enter(producer, |context| {
            let count = one(context.writer(), 41);
            let entry = one(
                context.writer(),
                Entry {
                    program: &program,
                    count,
                },
            );
            let carrier = context.lift::<Listing>(entry);
            context.keep(carrier)
        })
        .unwrap();
    graph
        .enter(consumer, |context| {
            context.hold(producer).unwrap();
            // The successor captures the borrow of storage outside the graph, and prices nothing.
            context.store_successor(program.as_str());
        })
        .unwrap();
    // The consumer holds the producer, so its region seals rather than reclaims.
    graph.release(producer, ReleaseAbsorption::Refused).unwrap();
    graph
        .enter(consumer, |context| {
            let captured = context.continuation().expect("the successor was stored");
            assert!(std::ptr::eq(captured, program.as_str()));
            let redeemed = context
                .redeem(dormant)
                .expect("the consumer holds the sealed producer");
            let read = context.read(&redeemed).value();
            assert!(std::ptr::eq(read.program, program.as_str()));
            assert_eq!(*read.count, 41);
        })
        .unwrap();
    graph
        .release(consumer, ReleaseAbsorption::IntoHolder)
        .unwrap();
    assert!(graph.is_empty());
}
