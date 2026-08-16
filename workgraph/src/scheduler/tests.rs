//! The scheduler's white-box slates and the fixtures they share.
//!
//! Every fixture here names only stand-in types — a `Cart`-backed anchor, a boxed `dyn FnOnce`
//! continuation — never a koan type, so the slates exercise the generic scheduler on its own terms.
//!
//! - [`continuation`] — the owned-tier continuation slot under Miri (tree borrows).
//! - [`delivery`] — the finalize walk's timelines: copy and pin verdicts, a consumer that dies
//!   before its producer fires, a late wire onto a delivered edge, and root-edge release.
//! - [`drain`] — the run protocol: each verdict arm's application, the retirement hook's
//!   exactly-once contract and ordering, the step-start dep reads, and the deadlock report.
//! - [`edges`] — the edge slab's alloc/release recycling and the install door's branches.
//! - [`reinstall`] — the replace boundary's timelines: a loop-carried argument relocated into the
//!   incoming anchor's region, under both verdicts.

use std::rc::Rc;
use std::rc::Weak;

use super::nodes::NodeWork;
use super::workload::{DeliveredTerminal, DeliveryDestination};
use super::{Anchor, NodeId, Scheduler, Workload};
use crate::witnessed::doctest_fixture::{FixtureProfile, RegionCart};
use crate::witnessed::reattachable;

mod continuation;
mod delivery;
mod drain;
mod edges;
mod reinstall;

/// The inter-node value family: a **borrow into a region**, which is what makes a delivery verdict
/// observable — a plain `u32` would carry no region and copy-or-pin would be unobservable.
struct U32Value;
/// The trivial extern operand a step with nothing to zip passes to the owned tier's one open verb.
struct UnitOperand;
reattachable! {
    U32Value => &'r u32,
    UnitOperand => (),
}

/// The stored-continuation shape: a boxed `dyn FnOnce` over a captured region borrow — a **fat**
/// pointer whose `At<'static>` needs drop, so it rests only on the owned tier.
struct DynContinuation;
reattachable!(droppable DynContinuation => Box<dyn FnOnce() -> u32 + 'r>);

/// The per-slot memory anchor: an `Rc<RegionCart>` whose bump is the region a continuation borrows
/// into and a delivery lands in. Sealing the continuation against `Rc<TestAnchor>` therefore
/// transitively pins that backing for the seal's whole dormant life.
struct TestAnchor(Rc<RegionCart>);

impl TestAnchor {
    /// A fresh anchor over its own region — one per slot, so a test can watch a producer's region
    /// die (or survive) independently of its consumer's.
    fn fresh() -> Rc<Self> {
        Rc::new(TestAnchor(crate::witnessed::doctest_fixture::fresh_cart()))
    }

    /// The anchor's region handle — the allocation door a fixture builds its borrowed values through.
    fn handle(&self) -> crate::witnessed::RegionHandle<'_, FixtureProfile> {
        crate::witnessed::RegionHandle::from_owner(&*self.0)
    }
}

impl Anchor for TestAnchor {
    type Owner = RegionCart;
    fn owner(&self) -> &Rc<Self::Owner> {
        &self.0
    }
}

/// The delivery hook shared by both workloads below, parameterized on the verdict: `keep` is what
/// the relocation claims it still borrows from the source, and `relocate` is the act that claim
/// describes. Written once so the two workloads differ in exactly the verdict and nothing else.
fn deliver_with<W>(
    terminal: &DeliveredTerminal<W>,
    dest: DeliveryDestination<W>,
    keep: bool,
) -> DeliveredTerminal<W>
where
    W: Workload<Value = U32Value, Profile = FixtureProfile, Frame = TestAnchor>,
{
    terminal.transfer_into::<crate::witnessed::RegionHandleFamily<FixtureProfile>, U32Value, FixtureProfile>(
        dest,
        move |_product, _region| keep,
        move |value, _handle, placement| {
            if keep {
                // Pin: the product *is* the source borrow, so the source region must stay alive —
                // which is what the `true` claim above buys.
                value
            } else {
                // Copy: rebuild at the destination, claiming nothing of the source.
                placement.handle().allocator().value(*value)
            }
        },
    )
}

/// The **copy-verdict** workload: delivery rebuilds the value at each destination and claims nothing
/// of the source, so a producer's region is free to die at its own finalize.
struct TestWorkload;
impl Workload for TestWorkload {
    type Value = U32Value;
    type Error = ();
    type Profile = FixtureProfile;
    type Frame = TestAnchor;
    type Continuation = DynContinuation;

    fn deliver(
        terminal: &DeliveredTerminal<Self>,
        dest: DeliveryDestination<Self>,
    ) -> DeliveredTerminal<Self> {
        deliver_with::<Self>(terminal, dest, false)
    }
}

/// The **pin-verdict** workload: delivery hands the source borrow through verbatim and claims it,
/// so the producer's region transfers by hold into each destination's union bundle and lives as long
/// as the destination does. Identical to [`TestWorkload`] in every other associated type, so the two
/// slates differ in the verdict alone.
struct PinWorkload;
impl Workload for PinWorkload {
    type Value = U32Value;
    type Error = ();
    type Profile = FixtureProfile;
    type Frame = TestAnchor;
    type Continuation = DynContinuation;

    fn deliver(
        terminal: &DeliveredTerminal<Self>,
        dest: DeliveryDestination<Self>,
    ) -> DeliveredTerminal<Self> {
        deliver_with::<Self>(terminal, dest, true)
    }
}

/// Allocate one dep-free slot over a **fresh region of its own**, handing back the slot, its anchor,
/// and a weak probe on the region owner — so a test can drop every strong hold it has and ask
/// whether the region is still alive.
fn alloc_slot<W>(sched: &mut Scheduler<W>) -> (NodeId, Rc<TestAnchor>, Weak<RegionCart>)
where
    W: Workload<
            Value = U32Value,
            Profile = FixtureProfile,
            Frame = TestAnchor,
            Continuation = DynContinuation,
        >,
{
    let anchor = TestAnchor::fresh();
    let probe = Rc::downgrade(anchor.owner());
    let continuation: Box<dyn FnOnce() -> u32> = Box::new(|| 0);
    let id = sched.alloc_node(
        NodeWork::new(continuation, None),
        &[],
        Rc::clone(&anchor),
        false,
    );
    (id, anchor, probe)
}

/// Build the terminal `id`'s step would produce: a `u32` bumped into that slot's own region and
/// enveloped as a resident of it. This is the value delivery relocates out.
fn terminal<W>(anchor: &TestAnchor, payload: u32) -> DeliveredTerminal<W>
where
    W: Workload<Value = U32Value, Profile = FixtureProfile, Frame = TestAnchor>,
{
    let handle = anchor.handle();
    let resident: &u32 = handle.allocator().value(payload);
    handle.deliver_resident::<U32Value>(resident)
}
