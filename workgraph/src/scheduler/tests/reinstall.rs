//! Miri slate (tree borrows) for **delivery at replace**: what happens to the retiring incarnation's
//! region when a slot is reinstalled over a fresh anchor.
//!
//! Each test is a *timeline* — what is alive when, and for how long — rather than a value check.
//! Under Miri they fail on UB; run natively they fail on a liveness probe.
//!
//! A reinstall is the one boundary where a slot turns over its own memory. The scheduler holds
//! nothing across it: `replace` hands the displaced anchor straight back, and whatever the next
//! incarnation reads is the embedder's to relocate into the new anchor's region *before* it
//! replaces. So the ordering — free the retiring region only after the relocation reads it — is a
//! local variable in the caller, and the two shapes below are the two verdicts that ordering has to
//! serve:
//!
//! 1. a **copy** verdict frees the retiring region at the replace, and
//! 2. a **pin** verdict transfers it by hold into the new incarnation's anchor bundle.
//!
//! The fixtures come from the parent module: `TestAnchor` owns a real `Region`, `TestWorkload`
//! relocates by copy and `PinWorkload` by pin, so the verdict is the only thing that varies.

use std::rc::Rc;
use std::rc::Weak;

use super::super::nodes::NodeWork;
use super::super::{NodeId, ResolvedDeps, Scheduler, Workload};
use super::*;
use crate::witnessed::doctest_fixture::RegionCart;
use crate::witnessed::{NoPins, RegionHandleFamily, SealedExtern};

/// Relocate a loop-carried argument into `into`'s region through the workload's own delivery hook —
/// the embedder's act, run on the deciding side of a replace, and the one place the verdict is
/// asked. Hands back the reinstalled incarnation's work: a continuation reading the relocated value
/// back out of the cell it rests in, which is the read the retiring region must not be freed ahead
/// of.
fn carry_into<'a, W>(argument: &DeliveredTerminal<W>, into: &'a TestAnchor) -> NodeWork<'a, W>
where
    W: Workload<
            Value = U32Value,
            Profile = FixtureProfile,
            Frame = TestAnchor,
            Continuation = DynContinuation,
        >,
{
    let handle = into.handle();
    let dest = handle.deliver_resident::<RegionHandleFamily<FixtureProfile>>(handle);
    let cell = W::deliver(argument, dest).rest_into(handle);
    let continuation: Box<dyn FnOnce() -> u32 + 'a> = Box::new(move || cell.open(|v| *v));
    NodeWork::new(ResolvedDeps::new(), continuation, None)
}

/// Run the reinstalled incarnation's step: take its work and open the continuation the way the run
/// loop does, under the anchor the seal bundled. The read is the point — every strong hold on the
/// retiring region is gone by the time this runs.
fn run_reinstalled<W>(sched: &mut Scheduler<W>, id: NodeId) -> u32
where
    W: Workload<Continuation = DynContinuation>,
{
    let (work, _anchor) = sched.take_for_run(id);
    let (_deps, sealed, _carrier) = work.into_run_parts();
    sealed.open(
        SealedExtern::<UnitOperand>::erase(()),
        &NoPins,
        |_within, continuation: Box<dyn FnOnce() -> u32 + '_>, ()| continuation(),
    )
}

/// **Timeline 6 — a copy verdict frees the retiring region at the replace.** The relocation rebuilds
/// the argument in the incoming anchor's region and claims nothing of the outgoing one, so the
/// displaced anchor `replace` hands back is the last hold on it: the region dies where that local
/// falls, and the reinstalled incarnation still reads its argument.
#[test]
fn copy_verdict_frees_the_retiring_region_at_the_replace() {
    let mut sched: Scheduler<TestWorkload> = Scheduler::new();
    let (id, retiring_anchor, retiring_region) = alloc_slot(&mut sched);
    // The loop-carried argument, produced by the incarnation that is about to retire and therefore
    // living in the region that is about to go.
    let argument = terminal::<TestWorkload>(&retiring_anchor, 41);
    let incoming = TestAnchor::fresh();
    // The deciding side: relocate before installing, so the install has nothing left to order.
    let work = carry_into::<TestWorkload>(&argument, &incoming);

    // The test's own holds on the retiring incarnation go first, so the scheduler's row is the only
    // one left and the displaced anchor below is genuinely the last.
    drop(argument);
    drop(retiring_anchor);
    assert!(
        retiring_region.upgrade().is_some(),
        "the slot's anchor row is still holding the retiring region",
    );

    let displaced = sched.replace(id, work, Some(Rc::clone(&incoming)));
    assert!(
        displaced.is_some(),
        "a framed replace hands its displaced anchor back rather than parking it",
    );
    drop(displaced);
    assert!(
        retiring_region.upgrade().is_none(),
        "a copy-verdict relocation leaves nothing claiming the retiring region, so it frees where \
         the displaced anchor falls",
    );

    assert_eq!(
        run_reinstalled(&mut sched, id),
        41,
        "the reinstalled incarnation reads its argument out of its own region",
    );
}

/// **Timeline 7 — a pin verdict transfers the retiring region by hold.** The relocation hands the
/// outgoing region's borrow through and claims it, so the mint retains that region's owner in the
/// incoming anchor's union bundle: the retiring region outlives the replace that dropped its anchor,
/// the reinstalled incarnation reads *through* the pin, and the region dies exactly when the
/// incarnation holding it does.
#[test]
fn pin_verdict_transfers_the_retiring_region_into_the_new_anchor_bundle() {
    let mut sched: Scheduler<PinWorkload> = Scheduler::new();
    let (id, retiring_anchor, retiring_region) = alloc_slot(&mut sched);
    let argument = terminal::<PinWorkload>(&retiring_anchor, 42);
    let incoming = TestAnchor::fresh();
    let incoming_region: Weak<RegionCart> = Rc::downgrade(incoming.owner());
    let work = carry_into::<PinWorkload>(&argument, &incoming);

    drop(argument);
    drop(retiring_anchor);

    drop(sched.replace(id, work, Some(Rc::clone(&incoming))));
    assert!(
        retiring_region.upgrade().is_some(),
        "a pin-verdict relocation claims the source, so the incoming anchor's bundle holds it",
    );

    assert_eq!(
        run_reinstalled(&mut sched, id),
        42,
        "the reinstalled incarnation reads its argument through the pin",
    );

    // Tear the new incarnation down: its union bundle is the last hold on the retiring region.
    drop(sched);
    drop(incoming);
    assert!(
        incoming_region.upgrade().is_none(),
        "nothing outlives the incarnation that took the region over",
    );
    assert!(
        retiring_region.upgrade().is_none(),
        "the retiring region dies with the incarnation whose bundle held it",
    );
}
