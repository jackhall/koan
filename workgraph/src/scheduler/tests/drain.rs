//! White-box slate for the **drain protocol**: each [`StepVerdict`] arm's application, the
//! step-start dep reads (residents handed pre-read, edges released exactly once), the
//! [`Workload::retiring`] hook's exactly-once contract and per-arm ordering, and the deadlock
//! report. Runs under Miri alongside the rest of the lib slate.
//!
//! The fixtures come from the parent module; `RetireWorkload` adds an anchor that records owned
//! edges and counts retirement calls, so the hook's contract is observable.

use std::cell::{Cell, RefCell};

use super::super::{EdgeId, InstalledEdge, StepVerdict};
use super::*;
use crate::witnessed::{RegionHandle, StepCoverage};

/// Read the `u32` resting on `edge`, copied out from inside the read.
fn read<W>(sched: &Scheduler<W>, edge: EdgeId) -> u32
where
    W: Workload<Value = U32Value>,
{
    match sched.read_edge_result_with(edge, |v| *v) {
        Ok(value) => value,
        Err(_) => panic!("a value terminal, not an error"),
    }
}

/// The per-slot memory anchor for the retirement slates: a region cart plus the owned-edge record
/// the [`Workload::retiring`] impl drains, and a counter proving the hook's exactly-once contract.
struct RetireAnchor {
    cart: Rc<RegionCart>,
    owned: RefCell<Vec<EdgeId>>,
    retire_calls: Cell<u32>,
}

impl RetireAnchor {
    fn fresh() -> Rc<Self> {
        Rc::new(RetireAnchor {
            cart: crate::witnessed::doctest_fixture::fresh_cart(),
            owned: RefCell::new(Vec::new()),
            retire_calls: Cell::new(0),
        })
    }
}

impl Anchor for RetireAnchor {
    type Owner = RegionCart;
    fn owner(&self) -> &Rc<Self::Owner> {
        &self.cart
    }
}

thread_local! {
    /// Delivery-walk adopt counter — how many destinations the walk found live to deliver into.
    static DELIVERS: Cell<u32> = const { Cell::new(0) };
}

/// A copy-verdict workload whose anchors own edges: `retiring` drains the anchor's owned list and
/// counts the call, and `deliver` counts each adopt — which is what makes the retire-before-walk
/// ordering observable.
struct RetireWorkload;
impl Workload for RetireWorkload {
    type Value = U32Value;
    type Error = ();
    type Profile = FixtureProfile;
    type Frame = RetireAnchor;
    type Continuation = DynContinuation;

    fn deliver(
        terminal: &DeliveredTerminal<Self>,
        dest: DeliveryDestination<Self>,
    ) -> DeliveredTerminal<Self> {
        DELIVERS.with(|c| c.set(c.get() + 1));
        terminal
            .transfer_into::<crate::witnessed::RegionHandleFamily<FixtureProfile>, U32Value, FixtureProfile>(
                dest,
                |_product, _region| false,
                |value, _handle, placement| placement.handle().allocator().value(*value),
            )
    }

    fn retiring(anchor: &RetireAnchor) -> Vec<EdgeId> {
        anchor.retire_calls.set(anchor.retire_calls.get() + 1);
        anchor.owned.borrow_mut().drain(..).collect()
    }
}

/// Allocate one dep-free `RetireWorkload` slot over a fresh region of its own.
fn alloc_retire_slot(sched: &mut Scheduler<RetireWorkload>) -> (NodeId, Rc<RetireAnchor>) {
    let anchor = RetireAnchor::fresh();
    let continuation: Box<dyn FnOnce() -> u32> = Box::new(|| 0);
    let id = sched.alloc_node(
        NodeWork::new(continuation, None),
        &[],
        Rc::clone(&anchor),
        false,
    );
    (id, anchor)
}

/// Build the terminal a `RetireWorkload` step produces: a `u32` bumped into the step's own region.
fn retire_terminal(cart: &Rc<RegionCart>, payload: u32) -> DeliveredTerminal<RetireWorkload> {
    let handle = RegionHandle::from_owner(&**cart);
    let resident: &u32 = handle.allocator().value(payload);
    handle.deliver_resident::<U32Value>(resident)
}

/// **`Done` applies as a finalize**: the terminal is delivered to the waiting edge and the slot
/// reclaims. A dep-free slot's step arrives with an empty dep slice.
#[test]
fn done_verdict_delivers_and_reclaims_the_slot() {
    let mut sched: Scheduler<TestWorkload> = Scheduler::new();
    let (producer, anchor, _) = alloc_slot(&mut sched);
    let dest = TestAnchor::fresh();
    let edge = sched.install_edge(producer, dest.owner());
    drop(anchor);

    sched
        .drain(|_sched, step| {
            assert!(
                step.dep_results.is_empty(),
                "a dep-free slot's step reads no deps",
            );
            StepVerdict::Done(Ok(terminal::<TestWorkload>(&step.anchor, 41)))
        })
        .expect("an acyclic run drains clean");

    assert_eq!(read(&sched, edge), 41, "the terminal landed on the waiter");
    assert_eq!(sched.free_list_len(), 1, "the finalized slot reclaimed");
}

/// **Dep residents arrive pre-read, and the drain releases each dep edge exactly once.** The
/// consumer's callback holds a retained cell it re-brands under its own coverage; by drain end
/// every slab index has been recycled — a second release of any edge would be loud in debug.
#[test]
fn dep_residents_arrive_pre_read_and_their_edges_release_once() {
    let mut sched: Scheduler<TestWorkload> = Scheduler::new();
    let (producer, _producer_anchor, _) = alloc_slot(&mut sched);
    let consumer_anchor = TestAnchor::fresh();
    let source = sched.install_edge(producer, consumer_anchor.owner());
    let continuation: Box<dyn FnOnce() -> u32> = Box::new(|| 0);
    let consumer = sched.alloc_node(
        NodeWork::new(continuation, None),
        &[source],
        Rc::clone(&consumer_anchor),
        true,
    );
    sched.release_edge(source);

    let seen: Cell<Option<u32>> = Cell::new(None);
    sched
        .drain(|_sched, step| {
            if step.id == consumer {
                assert_eq!(
                    step.dep_results.len(),
                    1,
                    "one dep, one result, in dep order"
                );
                let coverage = StepCoverage::of(Rc::clone(consumer_anchor.owner()));
                let value = step.dep_results[0]
                    .as_ref()
                    .expect("the producer delivered a value")
                    .brand_with(&coverage)
                    .open(|v| *v);
                seen.set(Some(value));
                StepVerdict::Done(Err(()))
            } else {
                StepVerdict::Done(Ok(terminal::<TestWorkload>(&step.anchor, 23)))
            }
        })
        .expect("an acyclic run drains clean");

    assert_eq!(seen.get(), Some(23), "the dep's resident reached the step");
    assert_eq!(
        sched.edge_free_list_len(),
        sched.edge_slab_len(),
        "every edge was released and recycled exactly once",
    );
}

/// **`Replace` reinstalls the slot and reruns it**: the second incarnation's step produces the
/// terminal the waiter receives.
#[test]
fn replace_verdict_reinstalls_and_reruns_the_slot() {
    let mut sched: Scheduler<TestWorkload> = Scheduler::new();
    let (slot, _anchor, _) = alloc_slot(&mut sched);
    let dest = TestAnchor::fresh();
    let edge = sched.install_edge(slot, dest.owner());

    let steps = Cell::new(0u32);
    sched
        .drain(|_sched, step| {
            steps.set(steps.get() + 1);
            if steps.get() == 1 {
                let continuation: Box<dyn FnOnce() -> u32> = Box::new(|| 0);
                StepVerdict::Replace {
                    work: NodeWork::new(continuation, None),
                    anchor: None,
                }
            } else {
                StepVerdict::Done(Ok(terminal::<TestWorkload>(&step.anchor, 7)))
            }
        })
        .expect("an acyclic run drains clean");

    assert_eq!(steps.get(), 2, "the replaced slot ran again");
    assert_eq!(
        read(&sched, edge),
        7,
        "the second incarnation's terminal landed"
    );
}

/// **`Forward` applies as a forward-finalize**: the resident already resting on the named edge is
/// delivered onward through the slot's own walk.
#[test]
fn forward_verdict_delivers_the_forwarded_resident() {
    let mut sched: Scheduler<TestWorkload> = Scheduler::new();
    let (producer, _producer_anchor, _) = alloc_slot(&mut sched);
    let (forwarder, _forwarder_anchor, _) = alloc_slot(&mut sched);
    let dest = TestAnchor::fresh();
    let source = sched.install_edge(producer, dest.owner());
    let wait = sched.install_edge(forwarder, dest.owner());

    sched
        .drain(|sched, step| {
            if step.id == producer {
                StepVerdict::Done(Ok(terminal::<TestWorkload>(&step.anchor, 7)))
            } else {
                // The classification probe: the producer has delivered, so it fills.
                let InstalledEdge::Filled(probe) = sched.install_edge_from(source) else {
                    panic!("the producer has delivered, so the probe fills");
                };
                StepVerdict::Forward(probe)
            }
        })
        .expect("an acyclic run drains clean");

    assert_eq!(
        read(&sched, wait),
        7,
        "the forwarded resident reached the waiter"
    );
}

/// **`Alias` applies as a splice**: the slot's waiting edges are re-pointed at the producer behind
/// the source, whose later fire delivers into them directly.
#[test]
fn alias_verdict_splices_waiters_onto_the_producer() {
    let mut sched: Scheduler<TestWorkload> = Scheduler::new();
    let (forwarder, _forwarder_anchor, _) = alloc_slot(&mut sched);
    let (producer, _producer_anchor, _) = alloc_slot(&mut sched);
    let dest = TestAnchor::fresh();
    let source = sched.install_edge(producer, dest.owner());
    let wait = sched.install_edge(forwarder, dest.owner());

    sched
        .drain(|sched, step| {
            if step.id == forwarder {
                // The classification probe parks: the producer is pre-terminal, so the slot
                // splices out rather than forwarding a value.
                let InstalledEdge::Parked(probe) = sched.install_edge_from(source) else {
                    panic!("the producer is pre-terminal, so the probe parks");
                };
                sched.release_edge(probe);
                StepVerdict::Alias(source)
            } else {
                StepVerdict::Done(Ok(terminal::<TestWorkload>(&step.anchor, 9)))
            }
        })
        .expect("the spliced slot reclaims, so nothing is unresolved");

    assert_eq!(
        read(&sched, wait),
        9,
        "the re-pointed waiter received the producer's delivery"
    );
}

/// **Retirement runs exactly once, before the delivery walk.** The slot's one owned edge — a
/// placeholder claim parked on the slot itself — is released by the hook before `finalize`, so the
/// walk meets a released entry, skips it, and never invokes the workload's deliver.
#[test]
fn retiring_runs_once_and_before_the_delivery_walk() {
    DELIVERS.with(|c| c.set(0));
    let mut sched: Scheduler<RetireWorkload> = Scheduler::new();
    let (slot, anchor) = alloc_retire_slot(&mut sched);
    let owned = sched.install_edge(slot, anchor.owner());
    anchor.owned.borrow_mut().push(owned);

    sched
        .drain(|_sched, step| StepVerdict::Done(Ok(retire_terminal(&step.anchor.cart, 5))))
        .expect("an acyclic run drains clean");

    assert_eq!(anchor.retire_calls.get(), 1, "retiring ran exactly once");
    DELIVERS.with(|c| {
        assert_eq!(
            c.get(),
            0,
            "the owned edge was released before the walk, so no live destination remained",
        )
    });
    assert_eq!(
        sched.edge_free_list_len(),
        1,
        "the walk recycled the released owned edge",
    );
}

/// **A `Replace` retires nothing**: the slot lives on and keeps its claims, so the hook fires only
/// at the terminal step.
#[test]
fn replace_does_not_retire_the_slot() {
    let mut sched: Scheduler<RetireWorkload> = Scheduler::new();
    let (_slot, anchor) = alloc_retire_slot(&mut sched);

    let steps = Cell::new(0u32);
    sched
        .drain(|_sched, _step| {
            steps.set(steps.get() + 1);
            if steps.get() == 1 {
                let continuation: Box<dyn FnOnce() -> u32> = Box::new(|| 0);
                StepVerdict::Replace {
                    work: NodeWork::new(continuation, None),
                    anchor: None,
                }
            } else {
                StepVerdict::Done(Err(()))
            }
        })
        .expect("an acyclic run drains clean");

    assert_eq!(steps.get(), 2);
    assert_eq!(
        anchor.retire_calls.get(),
        1,
        "only the terminal step retired; the replace kept the slot's claims",
    );
}

/// **Retirement runs after the forward read.** The forwarder owns its classification probe and
/// forwards on it; the drain reads the resident off the probe first and releases it after — a
/// release-first application would panic on the read of a released edge.
#[test]
fn retiring_runs_after_the_forward_read() {
    let mut sched: Scheduler<RetireWorkload> = Scheduler::new();
    let (producer, _producer_anchor) = alloc_retire_slot(&mut sched);
    let (forwarder, forwarder_anchor) = alloc_retire_slot(&mut sched);
    let dest = RetireAnchor::fresh();
    let source = sched.install_edge(producer, dest.owner());
    let wait = sched.install_edge(forwarder, dest.owner());

    let record = Rc::clone(&forwarder_anchor);
    sched
        .drain(|sched, step| {
            if step.id == producer {
                StepVerdict::Done(Ok(retire_terminal(&step.anchor.cart, 7)))
            } else {
                let InstalledEdge::Filled(probe) = sched.install_edge_from(source) else {
                    panic!("the producer has delivered, so the probe fills");
                };
                record.owned.borrow_mut().push(probe);
                StepVerdict::Forward(probe)
            }
        })
        .expect("an acyclic run drains clean");

    assert_eq!(
        read(&sched, wait),
        7,
        "the forward read ran ahead of the probe's release"
    );
    assert_eq!(
        forwarder_anchor.retire_calls.get(),
        1,
        "retiring ran exactly once"
    );
}

/// **A spliced-out slot retires exactly once, after the re-point**: its owned probe is released by
/// the hook, and the producer's later walk recycles it.
#[test]
fn alias_retires_the_slot_after_the_splice() {
    let mut sched: Scheduler<RetireWorkload> = Scheduler::new();
    let (forwarder, forwarder_anchor) = alloc_retire_slot(&mut sched);
    let (producer, _producer_anchor) = alloc_retire_slot(&mut sched);
    let dest = RetireAnchor::fresh();
    let source = sched.install_edge(producer, dest.owner());
    let wait = sched.install_edge(forwarder, dest.owner());

    let record = Rc::clone(&forwarder_anchor);
    sched
        .drain(|sched, step| {
            if step.id == forwarder {
                let InstalledEdge::Parked(probe) = sched.install_edge_from(source) else {
                    panic!("the producer is pre-terminal, so the probe parks");
                };
                record.owned.borrow_mut().push(probe);
                StepVerdict::Alias(source)
            } else {
                StepVerdict::Done(Ok(retire_terminal(&step.anchor.cart, 3)))
            }
        })
        .expect("the spliced slot reclaims, so nothing is unresolved");

    assert_eq!(
        forwarder_anchor.retire_calls.get(),
        1,
        "the spliced-out slot retired once"
    );
    assert_eq!(
        read(&sched, wait),
        3,
        "the re-pointed waiter received the producer's delivery"
    );
}

/// **The deadlock backstop carries the pending count and the carrier sample.** The cyclic wait is
/// wired at the primitive tier — the install door's debug assertion forecloses a cycling park, so
/// the backstop is reachable only by the invariant breach it exists to report.
#[test]
fn deadlock_surfaces_pending_count_and_carrier_sample() {
    let mut sched: Scheduler<TestWorkload> = Scheduler::new();
    let (a, _a_anchor, _) = alloc_slot(&mut sched);
    let (b, _b_anchor, _) = alloc_slot(&mut sched);
    let dest = TestAnchor::fresh();

    let error = sched
        .drain(|sched, step| {
            // Each slot parks on the other, below the asserting door.
            let other = if step.id == a { b } else { a };
            let edge = sched
                .edges
                .install_parked(other, Some(step.id), dest.owner());
            sched.deps.wire_parked(other, edge, Some(step.id));
            let continuation: Box<dyn FnOnce() -> u32> = Box::new(|| 0);
            let carrier = (step.id == a).then(|| "STUCK-A".to_string());
            StepVerdict::Replace {
                work: NodeWork::new(continuation, carrier),
                anchor: None,
            }
        })
        .expect_err("a cyclic wait leaves both slots unresolved");

    assert_eq!(error.pending, 2, "both slots are stuck");
    assert_eq!(
        error.sample, "STUCK-A",
        "the carrier-bearing stuck slot out-renders the generic tag",
    );
}

/// **A cycling park is loud at the install door**: the debug assertion catches a self-park, the
/// smallest breach of the lexically-backward invariant.
#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "lexically backward")]
fn a_cycling_park_is_loud_at_the_install_door() {
    let mut sched: Scheduler<TestWorkload> = Scheduler::new();
    let (slot, anchor, _) = alloc_slot(&mut sched);
    let source = sched.install_edge(slot, anchor.owner());
    let _ = sched.install_deps(slot, &[source]);
}
