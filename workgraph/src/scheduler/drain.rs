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

use super::nodes::NodeWork;
use super::workload::{DeliveredTerminal, SealedTerminal};
use super::{EdgeId, NodeId, Scheduler, Workload};
use crate::witnessed::SealedPinned;

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
pub struct Step<W: Workload> {
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
    /// Errs with the deadlock report when slots are still parked after the drain — the backstop
    /// for the acyclicity invariant [`install_deps`](Self::install_deps) asserts.
    pub fn drain<'work>(
        &mut self,
        mut step: impl FnMut(&mut Self, Step<W>) -> StepVerdict<'work, W>,
    ) -> Result<(), DrainDeadlock<W>> {
        while let Some(id) = self.pop_next() {
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

    /// Runs exactly once per slot, from the one verdict arm that retires it.
    fn release_retiring(&mut self, anchor: &W::Frame) {
        for edge in W::retiring(anchor) {
            self.release_edge(edge);
        }
    }
}
