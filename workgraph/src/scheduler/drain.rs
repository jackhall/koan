//! **The run protocol, reified**: [`Scheduler::drain`] owns the pop → read-deps → step → apply
//! loop, and [`StepVerdict`] is the closed set of things a step can decide. The embedder's whole
//! contract is one callback: it receives a [`Step`] plus `&mut Scheduler` for the mid-step wiring a
//! step legitimately does, and returns the verdict the drain applies.
//! See [design/dag-scheduler.md § The drain protocol](../../design/dag-scheduler.md#the-drain-protocol).
//!
//! What owning the loop makes structural:
//!
//! - a popped slot always gets a verdict, and the verdict is always applied — a slot can never sit
//!   `Running` past its step;
//! - each dep edge is read and released exactly once, at step start, inside the drain — the embedder
//!   never touches a dep edge;
//! - [`Workload::retiring`] runs exactly once per slot, at the one point the slot stops being able
//!   to release its edges: before the delivery walk on a terminal (`Done`), after the forward read
//!   (`Forward`), and after the splice re-point (`Alias`) — a `Replace` retires nothing, since the
//!   slot lives on and keeps its claims.

use std::rc::Rc;

use bumpalo::Bump;

use super::nodes::NodeWork;
use super::workload::{DeliveredTerminal, SealedTerminal};
use super::{EdgeId, NodeId, Scheduler, Workload};
use crate::witnessed::{BumpAllocator, SealedPinned};

/// The closed protocol between the embedder's step callback and the scheduler's apply. Every arm
/// maps onto exactly one internal transition, so returning a verdict *is* applying it.
pub enum StepVerdict<'work, W: Workload> {
    /// Deliver the terminal (or the error) to every waiting edge and reclaim the slot.
    Done(Result<DeliveredTerminal<W>, W::Error>),
    /// The slot's result *is* the value already resting on this edge — deliver that resident
    /// onward through the slot's own walk.
    Forward(EdgeId),
    /// Tail-replace the slot with new work; `anchor` is the reinstalled incarnation's memory anchor
    /// at a framed replace (`None` keeps the slot's current one). The embedder relocates whatever
    /// the next incarnation reads into the new anchor's region *before* returning this verdict.
    Replace {
        work: NodeWork<'work, W>,
        anchor: Option<Rc<W::Frame>>,
    },
    /// The slot spliced itself out as a bare-name forward onto the producer behind this (parked)
    /// source edge: its waiting edges are re-pointed there and the slot reclaims.
    Alias(EdgeId),
}

/// One popped slot's step, as the drain hands it to the embedder's callback: the deps are already
/// read (and their edges released), and the continuation arrives still sealed.
pub struct Step<'scratch, W: Workload> {
    /// **The step's scratch arena**, over a `Bump` the drain owns and resets at every pop. A
    /// staging buffer that is built, read and dropped inside one step belongs here rather than on
    /// the global heap or in a frame region — a frame region is reclaimed when the *frame* dies, so
    /// step-transient staging put there accumulates across a tail-replacing slot's steps.
    ///
    /// `'scratch` is the drain's per-pop borrow, and the step callback's bound quantifies over it,
    /// so a buffer allocated through this handle cannot reach a `StepVerdict` — a `Replace`
    /// continuation is built at `'work`, fixed before the bump exists. That is a borrow-check fact,
    /// not a convention.
    pub scratch: BumpAllocator<'scratch>,
    /// The name a mid-step install wires deps onto.
    pub id: NodeId,
    /// A clone of the slot's memory anchor (the row keeps its own).
    pub anchor: Rc<W::Frame>,
    /// Opened by the embedder at its own step brand, beside its own operands, under the anchor pin
    /// the seal bundles.
    pub continuation: SealedPinned<W::Continuation, Rc<W::Frame>>,
    /// Each dep edge's delivered resident, in dep order, an errored dep in its slot. The edges
    /// themselves are already released — the values live in their destination regions, not in the
    /// edges.
    pub dep_results: Vec<Result<SealedTerminal<W>, W::Error>>,
}

/// Slots still parked after the queues drained, each waiting on a dependency that can no longer
/// fire. `sample` is the first stuck slot's memory anchor — the workload's own
/// [`Workload::Frame`] — so the embedder renders the report off per-slot data it wrote itself
/// rather than off a diagnostic string the scheduler carried for it.
pub struct DrainDeadlock<W: Workload> {
    pub pending: usize,
    pub sample: Rc<W::Frame>,
}

// Hand-written because `W::Frame` is the embedder's own anchor type and carries no `Debug` bound.
impl<W: Workload> std::fmt::Debug for DrainDeadlock<W> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DrainDeadlock")
            .field("pending", &self.pending)
            .finish_non_exhaustive()
    }
}

impl<W: Workload> Scheduler<W> {
    /// **The run loop**, until the queues drain. `step` receives `&mut Self` for the wiring a step
    /// body legitimately does mid-step (realizing dep requests via
    /// [`alloc_node`](Self::alloc_node), minting source edges,
    /// [`install_deps`](Self::install_deps)); the verdict application is the drain's alone.
    ///
    /// `'work` is the lifetime a `Replace` verdict's live continuation is built at — one lifetime
    /// for the whole drain, independent of the per-call `&mut Self` borrow, since the work is
    /// sealed against its anchor the moment the verdict is applied.
    ///
    /// The loop also owns the **step scratch arena** it hands out on [`Step::scratch`]. Its
    /// lifetime in the `FnMut` bound is elided, hence higher-ranked: `'work` is fixed before the
    /// bump exists, so no verdict can carry a scratch-hosted value out of the step that built it.
    ///
    /// Errs with the deadlock report when slots are still parked after the drain — the backstop
    /// for the acyclicity invariant [`install_deps`](Self::install_deps) asserts.
    pub fn drain<'work>(
        &mut self,
        mut step: impl FnMut(&mut Self, Step<'_, W>) -> StepVerdict<'work, W>,
    ) -> Result<(), DrainDeadlock<W>> {
        let mut scratch = Bump::new();
        while let Some(id) = self.pop_next() {
            // Once per pop, structurally: the drain loop is the only place a step is invoked, and
            // neither `apply`'s self-recursion nor a mid-step submission re-enters it. The previous
            // iteration's `&scratch` borrow died with its `step(...)` call, which is both what lets
            // this `&mut` reset borrow-check and why nothing scratch-hosted survives a pop.
            // `reset` retains the largest chunk, so steady state takes no allocator syscall.
            scratch.reset();
            let (work, anchor) = self.take_for_run(id);
            // Step start is a read, not a graph walk: every dep was delivered into its edge's
            // destination when its producer finalized. The slot is done with its dep edges once
            // their residents are in hand, so they release before the step runs; a `Replace`
            // installs fresh edges onto the row this take just emptied, so nothing releases twice.
            let dep_edges = self.deps.take_deps(id);
            let dep_results: Vec<Result<SealedTerminal<W>, W::Error>> = dep_edges
                .all_ids()
                .map(|edge| self.edge_resident(edge))
                .collect();
            for edge in dep_edges.all_ids() {
                self.release_edge(edge);
            }
            let verdict = step(
                self,
                Step {
                    scratch: BumpAllocator::over(&scratch),
                    id,
                    anchor: Rc::clone(&anchor),
                    continuation: work.continuation,
                    dep_results,
                },
            );
            match verdict {
                StepVerdict::Done(output) => {
                    // Retire before the walk: a released owned edge on this slot's own notify list
                    // is skipped and recycled rather than delivered into.
                    self.release_retiring(&anchor);
                    self.finalize(id, output);
                }
                StepVerdict::Forward(edge) => {
                    // Retire after the read: the edge the verdict names is on the slot's owned
                    // list, and the read goes through it.
                    self.finalize_forward(id, edge);
                    self.release_retiring(&anchor);
                }
                StepVerdict::Replace {
                    work,
                    anchor: new_anchor,
                } => {
                    // The displaced anchor falls here: the embedder relocated everything the next
                    // incarnation reads before returning the verdict, so the retiring region has
                    // nothing left to cover.
                    let _displaced = self.replace(id, work, new_anchor);
                }
                StepVerdict::Alias(edge) => {
                    // An alias never terminalizes, so its owned edges retire here — after the
                    // splice has re-pointed the slot's waiting edges at the producer.
                    self.splice_forward_from(id, edge);
                    self.release_retiring(&anchor);
                }
            }
        }
        match self.unresolved() {
            Some((pending, sample)) => Err(DrainDeadlock { pending, sample }),
            None => Ok(()),
        }
    }

    fn release_retiring(&mut self, anchor: &W::Frame) {
        for edge in W::retiring(anchor) {
            self.release_edge(edge);
        }
    }
}

/// Run `guard` with a step-scratch allocator over a fresh bump, at a brand the caller cannot name —
/// the drain's per-pop hand-out, minus the drain. The test fixture for scratch-hosted code that has
/// no scheduler to run under.
///
/// The `for<'s>` is the escape pin, and the reason this is a fixture rather than a `Bump` a test
/// makes itself: a buffer built over the handed-out allocator cannot be stashed anywhere whose
/// lifetime the caller chose.
///
/// ```
/// use workgraph::scheduler::drive_scratch;
/// use workgraph::witnessed::BumpVec;
///
/// let sum: u8 = drive_scratch(|scratch| {
///     let mut staged: BumpVec<'_, u8> = BumpVec::with_capacity_in(4, scratch);
///     staged.extend([1u8, 2, 3]);
///     staged.iter().sum()
/// });
/// assert_eq!(sum, 6);
/// ```
///
/// The same buffer may not leave:
///
/// ```compile_fail
/// use workgraph::scheduler::drive_scratch;
/// use workgraph::witnessed::BumpVec;
///
/// let mut escaped: Option<BumpVec<'_, u8>> = None;
/// drive_scratch(|scratch| {
///     let mut staged = BumpVec::with_capacity_in(4, scratch);
///     staged.push(1u8);
///     escaped = Some(staged);
/// });
/// ```
#[doc(hidden)]
pub fn drive_scratch<R>(guard: impl for<'s> FnOnce(BumpAllocator<'s>) -> R) -> R {
    let scratch = Bump::new();
    guard(BumpAllocator::over(&scratch))
}
