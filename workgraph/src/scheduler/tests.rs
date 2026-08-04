//! Miri slate (tree borrows) for the scheduler's **owned-tier continuation slot**: a droppable
//! continuation rests as `SealedPinned<W::Continuation, Rc<W::Frame>>`, sealed against the slot's
//! anchor at install and opened once per step. The two shapes that matter are the ones the tier
//! exists for — a parked slot dropped *unopened* (the seal's glue must run while its own pin still
//! holds the region the continuation borrows into) and the park → wake → open → run round trip
//! (the erased continuation re-anchors and reads a real borrow after every direct handle drops).
//!
//! Names only stand-in types — a `Cart`-backed anchor and a boxed `dyn FnOnce` continuation —
//! never a koan type. Fails on UB, not values.

use std::cell::Cell;
use std::rc::Rc;

use super::nodes::NodeWork;
use super::{Anchor, ResolvedDeps, Scheduler, Workload};
use crate::witnessed::doctest_fixture::Cart;
use crate::witnessed::{reattachable, NoPins, SealedExtern};

/// A lifetime-free `Reattachable` family for the trivial test value.
struct U32Value;
/// The trivial extern operand a step with nothing to zip passes to the owned tier's one open verb.
struct UnitOperand;
reattachable! {
    U32Value => u32,
    UnitOperand => (),
}

/// The stored-continuation shape: a boxed `dyn FnOnce` over a captured region borrow — a **fat**
/// pointer whose `At<'static>` needs drop, so it rests only on the owned tier.
struct DynContinuation;
reattachable!(droppable DynContinuation => Box<dyn FnOnce() -> u32 + 'r>);

/// A capture whose destructor dereferences a region borrow — so a continuation holding one is sound
/// to drop unopened only while the region is still pinned.
struct DropProbe<'r> {
    last: &'r u32,
    seen: Rc<Cell<u32>>,
}

impl Drop for DropProbe<'_> {
    fn drop(&mut self) {
        self.seen.set(*self.last);
    }
}

/// The per-slot memory anchor: an `Rc<Cart>` whose backing `Vec` is the region a continuation
/// borrows into. Sealing the continuation against `Rc<TestAnchor>` therefore transitively pins that
/// backing for the seal's whole dormant life.
struct TestAnchor(Rc<Cart>);

impl Anchor for TestAnchor {
    type Owner = Cart;
    fn owner(&self) -> &Rc<Self::Owner> {
        &self.0
    }
}

struct TestWorkload;
impl Workload for TestWorkload {
    type Value = U32Value;
    type Error = ();
    type Frame = TestAnchor;
    type Continuation = DynContinuation;
}

/// **The parked droppable continuation, dropped unopened.** The scheduler is torn down with the
/// slot still `PreRun`, and by then the seal's bundled pin is the last `Rc` on the cart the
/// continuation's captured probe dereferences in its destructor. `Scheduler`'s field order drops
/// `deps` (which holds the slot's anchor row) before `store`, so the seal's own pin is what keeps
/// the region alive while the glue runs — the whole point of co-locating the pin at the erase.
/// Fails on UB, not values; the assertions only confirm the glue ran and the region then died.
#[test]
fn parked_continuation_drops_under_its_own_pin() {
    let cart = Rc::new(Cart(vec![41, 42]));
    let anchor = Rc::new(TestAnchor(Rc::clone(&cart)));
    let seen = Rc::new(Cell::new(0u32));
    let alive = Rc::downgrade(&cart);

    let mut sched: Scheduler<TestWorkload> = Scheduler::new();
    {
        let probe = DropProbe {
            last: &cart.0[1],
            seen: Rc::clone(&seen),
        };
        let continuation: Box<dyn FnOnce() -> u32 + '_> = Box::new(move || *probe.last);
        sched.alloc_node(
            NodeWork::new(ResolvedDeps::new(), continuation, None),
            Rc::clone(&anchor),
            false,
        );
    }
    // The scheduler's own holds are now the only ones on the cart.
    drop(cart);
    drop(anchor);
    assert!(
        alive.upgrade().is_some(),
        "the parked slot's pins are the sole holders",
    );

    drop(sched);
    assert_eq!(seen.get(), 42, "the continuation's glue ran");
    assert!(alive.upgrade().is_none(), "teardown released the region");
}

/// **Park → wake → open → run.** The installed continuation is erased to `'static` behind the
/// dormant union slot, popped off the ready queue, re-anchored by the owned tier's one open verb
/// beside a trivial extern operand, and invoked inside the brand — reading its captured region
/// borrow after every direct handle on that region is gone. Fails on UB; the value assertion
/// confirms the read landed on the right cell.
#[test]
fn parked_continuation_opens_and_runs_after_its_handles_drop() {
    let cart = Rc::new(Cart(vec![7, 8, 9]));
    let anchor = Rc::new(TestAnchor(Rc::clone(&cart)));

    let mut sched: Scheduler<TestWorkload> = Scheduler::new();
    let id = {
        let captured: &u32 = &cart.0[2];
        let continuation: Box<dyn FnOnce() -> u32 + '_> = Box::new(move || *captured);
        sched.alloc_node(
            NodeWork::new(ResolvedDeps::new(), continuation, None),
            Rc::clone(&anchor),
            false,
        )
    };
    // Only the scheduler's holds remain — the seal's bundled pin and the slot's anchor row.
    drop(cart);
    drop(anchor);

    let ready = sched.pop_next().expect("a dep-free slot is ready");
    assert_eq!(
        ready,
        id.index(),
        "the ready slot is the one just installed"
    );

    let (work, _anchor, _handoff) = sched.take_for_run(id);
    let (_deps, sealed, _carrier) = work.into_run_parts();
    let got = sealed.open(
        SealedExtern::<UnitOperand>::erase(()),
        &NoPins,
        |continuation: Box<dyn FnOnce() -> u32 + '_>, ()| continuation(),
    );
    assert_eq!(got, 9);
}
