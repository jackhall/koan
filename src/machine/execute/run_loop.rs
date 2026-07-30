//! The Koan driver over the workload-independent [`Scheduler`](crate::scheduler::Scheduler): the
//! run loop ([`KoanRuntime::execute`]) pops ready slots and hands each to [`run_step`](KoanRuntime::run_step),
//! which brackets the step's ambient frame context end-to-end and applies the [`NodeStep`] it returns
//! through the scheduler's method contract. The scheduler stores and hands back opaque per-node state;
//! all Koan semantics — the per-call region lift, the return-contract enforcement, the lexical-chain
//! assembly — live here.
//!
//! See design/execution/README.md and design/memory-model.md.

use std::rc::Rc;

use crate::machine::core::bindings::{WriteGate, WriteOp};
use crate::machine::core::scope_frame;
use crate::machine::core::{FrameStorage, KoanRegionExt, KoanStorageProfile};
use crate::machine::model::CarriedFamily;
use crate::machine::{
    CarrierWitness, FrameCoverage, KError, KErrorKind, KoanRegion, NodeHandle, NodeId,
};
use crate::witnessed::{
    erase_to_static, reattachable, Delivered, RegionHandleFamily, SealedExtern,
};

use super::dispatch::SchedulerView;
use super::finalize::{finalize_error, NodeFinalize};
use super::nodes::{ChainOp, NodePayload, NodeScope, NodeStep, NodeWork};
use super::outcome::{DepTerminal, Outcome};
use super::runtime::{KoanRuntime, KoanWorkload};
use crate::scheduler::Anchor;

#[cfg(test)]
mod run_tests;
#[cfg(test)]
mod tests;

/// Koan's destination-region operand family: the library's [`RegionHandleFamily`] fixed to
/// [`KoanStorageProfile`] — the carrier a [`ForwardReady`](NodeStep::ForwardReady) relocation feeds
/// to [`Delivered::transfer_into`](crate::witnessed::Delivered::transfer_into) to re-anchor the
/// relocated value at the destination's lifetime, allocating the copy through the handle. The
/// library discharges `HasRegionHandle` for this family's live form itself (the base impl for
/// `RegionHandle` alone), so koan carries no impl of its own for it.
pub(in crate::machine::execute) type DestHandleFamily = RegionHandleFamily<KoanStorageProfile>;

/// The destination operand for a relocation: `dest_frame`'s handle `yoke`d into that frame's own
/// region, witnessed by it, and sealed into the delivery envelope the composition verbs take — its
/// residence is that same frame, so the product the composition builds inherits it. Co-located by
/// construction rather than paired with an asserted singleton. A bare handle reaches nothing beyond
/// its own region, so the operand's coverage is empty.
pub(in crate::machine::execute) fn dest_brand(
    dest_frame: Rc<FrameStorage>,
) -> Delivered<DestHandleFamily, CarrierWitness, FrameStorage> {
    let handle =
        KoanRegion::yoke_branded::<DestHandleFamily, _>(Rc::clone(&dest_frame), |b| b.handle());
    Delivered::seal(handle, dest_frame, FrameCoverage::empty())
}

/// `Reattachable` family for the step's **dep slice** — the producer terminals read out, erased, and
/// zipped into the step `open` so they arrive at the brand `'b` alongside the continuation. In-band
/// opening is the only sound route to the unbounded `'b`: a value opened here rides the audited
/// [`Erased::reattach`](crate::witnessed) the open already owns, where a witness-*borrow*-bounded
/// reattach would cap the produced lifetime at the borrow. The held step witness keeps the sources
/// alive across the open; the brand confines the values to it. Each cell is a
/// [`DepTerminal`](super::outcome::DepTerminal) — resolved value plus its `reach` set — so the reach
/// rides the slice to the construction site without a parallel channel.
/// Layout-invariant: a `Vec<Result<DepTerminal<'r>, KError>>` is a `Vec` of cells whose representation
/// never depends on `'r` (`FrameReach` / `KError` are lifetime-free), so the `reattachable!` macro
/// discharges the obligation.
pub(in crate::machine::execute) struct DepResultsFamily;

reattachable!(DepResultsFamily => Vec<Result<DepTerminal<'r>, KError>>);

impl<'run> KoanRuntime<'run> {
    /// On `Done` with a frame, the return `Value` references the per-call region that's
    /// about to drop, so it must be lifted into the captured scope's region before the
    /// frame is released. See design/memory-model.md.
    pub fn execute(&mut self) -> Result<(), KError> {
        while let Some(idx) = self.sched.pop_next() {
            let id = NodeId(idx);
            // A framed tail replace's retiring incarnation frame rides into the step as part of its
            // coverage: the reinstalled incarnation adopts the carried arguments here
            // (`extract_carried_args`), reading them out of the retiring region — where the
            // dispatching step rested them — which must stay live until it does. `None` for any
            // non-reinstalled step, or a frameless replace, which turns over no region.
            let (work, anchor, handoff) = self.sched.take_for_run(id);
            self.run_step(id, work, anchor, handoff);
        }
        // Slots still parked after drain are on a dependency that can never fire —
        // surface the cycle rather than panic on the top-level result read.
        if let Some((pending, sample)) = self.sched.unresolved() {
            return Err(KError::new(KErrorKind::SchedulerDeadlock {
                pending,
                sample,
            }));
        }
        Ok(())
    }

    /// The unified node handler, owning one slot step start to finish: collect the resolved dep
    /// terminals, then bracket the step's ambient frame context around running the continuation
    /// against a read-only [`SchedulerView`], reclaiming the owned-dep suffix, and applying the
    /// [`Outcome`] into a [`NodeStep`], before realizing the step. The step's ambient context is
    /// bracketed by [`KoanRuntime::with_slot_step`] inside the `open`, so no exit path — return or
    /// unwind — leaves it installed.
    ///
    /// The step tail runs inside one rank-2 `for<'b>` brand standing in for the step lifetime:
    /// [`SealedExtern::open`] opens the continuation, active scope, and dep slice together at `'b`,
    /// witnessed by `combined` (the held cart `Rc` unioned with the dep `pin`), which the bracket keeps
    /// live across the run. The consumer `dest` region is the opened scope's own
    /// region. The closure's result cannot name `'b`, so the `Outcome<'b>` and the finalized
    /// `Carried<'b>` are erased into the slot store *before* return: a value born at `'b` never has to
    /// launder to `'run` to cross the bracket exit, and nothing branded escapes. The step's cart clone
    /// is confined to this call and dropped at return; a `FreshTail` placement for the next iteration
    /// mints an entirely fresh cart, so nothing aliases across the boundary.
    fn run_step(
        &mut self,
        id: NodeId,
        work: NodeWork<KoanWorkload>,
        anchor: Rc<super::nodes::SlotFrame>,
        handoff: Option<Rc<super::nodes::SlotFrame>>,
    ) {
        let idx = id.index();
        // The step's binding-write sink: every `Action` the step interprets deposits its `WriteOp`s
        // here through `run_action`, and the drain below applies them against the step scope. Owned
        // by the run loop and confined to this call, so nothing crosses steps — a wake-time finish
        // runs inside its own step and fills that step's sink.
        let step_effects: std::cell::RefCell<Vec<WriteOp>> = std::cell::RefCell::new(Vec::new());
        // Source the step's context off the scheduler-held anchor: the cart, the slot's scope
        // handle, and its lexical chain. Read as values up front so nothing holds a scope borrow
        // across the step's `&mut self` work or a tail hop's frame swap.
        let cart = Rc::clone(&anchor.cart);
        let node_scope = anchor.payload.scope;
        let prev_chain_carrier = anchor.payload.chain.clone();
        let (deps, erased_continuation, _carrier) = work.into_run_parts();
        // The step's open witness: a step-confined cart clone, dropped at return. The tail open
        // re-anchors the step's carriers to the brand `'b` this witness pins, and owns that reattach.
        let continuation_witness = Rc::clone(&cart);
        // Consumer-pull: read each dep's terminal out of its producer frame so it can be re-anchored
        // in this consumer's own scope region, where the value dies with the consumer and no surviving
        // producer copy outlives its dying frame. The `dest` region is the scope's own region (right
        // even for a transparent USING window). The lift delivers deps *un-relocated*; the copy into
        // `dest` runs inside a construction fold's witnessed transfer, not here — the catch channel
        // duplicates the watched carrier instead of copying.
        let owned_indices: Vec<usize> = deps.owned().iter().map(|d| d.index()).collect();
        // Read each producer terminal out (borrow-bounded) into the dep slice — value plus its `reach`.
        // The slice erases into one carrier opened in-band at `'b` (see `DepResultsFamily`).
        let dep_sources: Vec<Result<DepTerminal<'static>, KError>> = deps
            .all_ids()
            .map(|d| {
                // The producer's own carrier bundled with its retained producer-frame owner as one
                // delivery envelope (duplicated so a construction finish folds the dep witnessed),
                // plus the live value erased out of it. One slot read; an errored slot
                // short-circuits here. `Delivered::open` reads under the retained frame owner
                // (`None` == frameless / run-region, externally pinned) rather than the carrier's own
                // witness, so it stays sound once the carrier collapses to reach-only.
                let delivered = self.sched.dep_delivered(d).map_err(|e| e.clone())?;
                let value = delivered.open(|live| erase_to_static::<CarriedFamily>(live));
                Ok(DepTerminal { value, delivered })
            })
            .collect();
        // The step's open witness — the **step's coverage**: the anchor's projected region owner
        // (pinning the continuation and dest region, plus their ancestor backings via the storage
        // `outer` chain) widened by every region this step's deps reach. Assembled before the open so
        // it outlives `'b`, and held across it, so re-anchoring the zipped carriers to `'b` cannot
        // dangle. Each dep contributes its envelope's own coverage — its retained producer frame
        // (sourced from the retention hold, since the reference-only carrier carries no pin of its
        // own) riding as an ordinary member alongside every other region the value reaches —
        // redundantly with the duplicated envelope held across the whole open in `dep_sources` (see
        // the struct doc above). An errored dep carries no value the step reads, so it contributes
        // nothing. The coverage is *only* a liveness pin: every value terminal rides `DoneWitnessed`
        // with its own carrier naming its reach, so no terminal reads it. Sourced off the
        // scheduler-returned anchor, not a `storage_rc()` of the cart the scheduler already holds.
        let mut combined: FrameCoverage = FrameCoverage::of(Rc::clone(anchor.owner()));
        for terminal in dep_sources.iter().flatten() {
            combined.absorb(terminal.delivered.coverage().clone());
        }
        // A framed tail replace's retiring incarnation, held for this step alone. It covers the
        // splice cells this slot's previous incarnation rested into that region — the reads
        // `SchedulerView::lift_spliced` takes — and is released when the coverage drops at return,
        // ordering the retiring region's free after the adoption.
        if let Some(retiring) = &handoff {
            combined.absorb(FrameCoverage::of(Rc::clone(retiring.owner())));
        }
        // Open the three externally-witnessed carriers — continuation, active scope, dep slice —
        // together at one rank-2 `for<'b>` brand witnessed by `combined` (see the doc comment for why
        // nothing branded escapes).
        let continuation = SealedExtern::seal(erased_continuation);
        // The active scope as a carrier, per node-scope shape: `Yoked` takes the start cart's own
        // child-scope carrier; `YokedChild` reuses the carrier it already holds. `combined` pins both.
        let scope_carrier = match node_scope {
            NodeScope::Yoked => continuation_witness.scope_sealed(),
            NodeScope::YokedChild(carrier) => carrier,
        };
        let dep_carrier = SealedExtern::<DepResultsFamily>::erase(dep_sources);
        continuation.zip(scope_carrier).zip(dep_carrier).open(
            &combined,
            |((continuation, scope), dep_sources)| {
                // `scope` is now live at `'b` and the `dest` region is its own region; deps arrive
                // un-relocated. A `ForwardReady` relocation below builds its destination carrier
                // from this same scope's brand.
                //
                // Bracket the step's ambient frame/payload — restored on every exit path,
                // including unwinds, by `with_slot_step` itself. The step's continuation deposits
                // its own return obligation into the ambient slot, surfaced back out on `post`.
                let (step, post) = self.with_slot_step(
                    cart,
                    NodePayload {
                        scope: node_scope,
                        chain: prev_chain_carrier.clone(),
                    },
                    |rt| {
                        let outcome = continuation(
                            &SchedulerView::new(
                                &rt.sched,
                                &rt.ambient,
                                scope,
                                scope_frame(scope),
                                NodeHandle {
                                    run: rt.run,
                                    node: id,
                                },
                                &step_effects,
                                &combined,
                            ),
                            deps.results(&dep_sources),
                            idx,
                        );
                        rt.sched.reclaim_deps(idx, owned_indices);
                        // Apply the step's binding writes against the step scope, in the order the
                        // bodies decided them. This is the **only** path that mutates a published
                        // binding table: it runs after the continuation returned — so no koan frame
                        // holds a competing borrow — and before the outcome is realized, so the
                        // writes land while the scope is still open and before any graph edge an
                        // errored step would strand is installed. On the first failure the
                        // remaining ops are dropped and the step becomes the node's error terminal,
                        // so the finalize arms below clear the producer's placeholders and
                        // attribute the error exactly as for an in-step error. A body that errors
                        // before deciding its write installs nothing at all: the writes are outcome
                        // data, and an error terminal carries none.
                        let mut gate = WriteGate::for_run_loop();
                        let outcome = match step_effects
                            .borrow_mut()
                            .drain(..)
                            .try_for_each(|op| op.apply(scope, &mut gate))
                        {
                            Ok(()) => outcome,
                            Err(error) => Outcome::Done(Err(error)),
                        };
                        // Realize the outcome into a `NodeStep`; a ready `Outcome::Forward` becomes
                        // a `ForwardReady` relocated below into this same `dest`.
                        rt.apply_outcome(outcome, idx)
                    },
                );
                // The producer's per-call frame, gated to a *dying* producer (a frameless / run-frame
                // producer folds in nothing): it gates the per-call return obligation (the contract
                // label and the finalize fold) and selects a `ForwardReady` relocation's destination
                // pin. Retention seeds independently — the scheduler reads the slot's own anchor owner
                // at finalize, so `non_dying` makes no memory decision.
                let frame = (!post.prev_frame.non_dying()).then_some(&post.prev_frame);
                match step {
                    NodeStep::DoneWitnessed(carrier) => {
                        // Seal the value terminal into a delivery envelope pinned by the anchor's own
                        // region owner — the same owner the scheduler seeds as the slot's retention
                        // host — before the Done-boundary hook runs. [`StepCarried::seal_at_step`] is
                        // the sole exit from the step brand: it discharges `'b` into the lifetime-free
                        // envelope. The already-witnessed carrier names its reach; the obligation the
                        // step deposited (`post.obligation`) is the slot's declared return, dropped by
                        // the `frame` gate for a frameless / run-frame producer. `finalize_terminal`
                        // re-stamps an obligation-coarsened value into the obligation's home region
                        // through the received envelope.
                        let envelope = carrier.seal_at_step(Rc::clone(anchor.owner()));
                        // `finalize_terminal` surfaces the terminal's owned foreign bundle alongside
                        // the sealed carrier; the scheduler seeds the retention hold with it, so no
                        // pull re-derives the reach.
                        match self.finalize_terminal(
                            envelope,
                            anchor.owner(),
                            frame.and(post.obligation.as_ref()),
                        ) {
                            Ok((witnessed, foreign)) => {
                                self.sched.finalize(idx, Ok(witnessed), foreign);
                            }
                            Err(error) => {
                                scope.clear_placeholders_for_producer(
                                    id,
                                    &mut WriteGate::for_run_loop(),
                                );
                                self.sched.finalize(idx, Err(error), FrameCoverage::empty());
                            }
                        }
                    }
                    NodeStep::Error(error) => {
                        // An error finalizes bare (no value, no witness); the frame-gated
                        // obligation still labels it with the callee's trace frame.
                        let error = finalize_error(error, frame.and(post.obligation.as_ref()));
                        scope.clear_placeholders_for_producer(id, &mut WriteGate::for_run_loop());
                        // A terminal error carries no value and no witness, so its retention hold's
                        // foreign bundle is empty; the producer frame still retains until its
                        // (short-circuiting) destinations pull.
                        self.sched.finalize(idx, Err(error), FrameCoverage::empty());
                    }
                    NodeStep::ForwardReady(producer) => {
                        // Relocate `producer`'s terminal into this slot's region via merge-transfer;
                        // no contract re-check (the producer enforced its own). Framed: the dest
                        // brand is `yoke`d into the anchor's own region owner, witnessed by it.
                        // Frameless: the dest region is externally pinned for the step, so a confined
                        // empty-set `resident` carries it. A ready-but-errored producer relocates to
                        // an `Err`.
                        let dest = match frame {
                            Some(_) => dest_brand(Rc::clone(anchor.owner())),
                            None => scope.seal_resident_delivered(
                                scope.resident::<DestHandleFamily>(scope.brand().handle()),
                                FrameCoverage::empty(),
                            ),
                        };
                        // The relocation threads back the relocated terminal's owned foreign bundle;
                        // seed the retention hold with it so no pull re-derives the reach.
                        match self.relocate_terminal(producer, dest) {
                            Ok((witnessed, foreign)) => {
                                self.sched.finalize(idx, Ok(witnessed), foreign);
                            }
                            Err(error) => {
                                scope.clear_placeholders_for_producer(
                                    id,
                                    &mut WriteGate::for_run_loop(),
                                );
                                self.sched.finalize(idx, Err(error), FrameCoverage::empty());
                            }
                        }
                    }
                    NodeStep::Replace {
                        work: new_work,
                        frame: new_frame,
                        chain,
                        overlay_scope,
                    } => {
                        let prev_frame = post.prev_frame;
                        // The frame the body runs in: a freshly installed cart, else the slot's
                        // current one (a `FramePlacement::Inherit` FN-body re-enters the cart a prior
                        // `Continue` installed). The `ChainOp` reads it to walk the body's lexical chain.
                        let body_frame: &crate::machine::core::CallFrame =
                            new_frame.as_deref().unwrap_or(&prev_frame);
                        // Read the chain-reshape variant before `apply` consumes it: a frameless
                        // re-entry mints a fresh anchor iff the chain (or the overlay scope) changed —
                        // an `Inherit` FN-body re-entry is frameless yet reshapes the chain
                        // (`AssembleBody`), so the gate keys on the variant, not on `frame.is_some()`.
                        let chain_changed = !matches!(chain, ChainOp::Unchanged);
                        let new_chain = chain.apply(prev_chain_carrier, body_frame);
                        match new_frame {
                            Some(f) => {
                                // A framed tail re-projects `Yoked` from its own cart; the overlay
                                // scope is the frameless (`Inherit`) path.
                                debug_assert!(
                                    overlay_scope.is_none(),
                                    "a framed tail-replace carries no overlay scope"
                                );
                                // The slot's scope is always this `f` cart's own child, so mint a
                                // payload-less `NodeScope::Yoked` re-projected at the read boundary —
                                // no persisted `&'run` to dangle across the tail hop. The scheduler
                                // parks the displaced incarnation as the reinstalled slot's handoff, so
                                // the retiring region outlives the adoption of the carried arguments
                                // (wired by the TCO handoff). `prev_frame` (the retiring cart) drops at
                                // the end of this arm: its storage stays pinned by `combined` until the
                                // step open above exits, and by the loop-carried argument carriers
                                // beyond that.
                                self.sched.replace(
                                    id,
                                    new_work,
                                    Some(super::nodes::SlotFrame::new(
                                        f,
                                        NodeScope::Yoked,
                                        new_chain,
                                    )),
                                );
                            }
                            None => {
                                // A frameless Replace keeps the prior cart. A tail entering an overlay
                                // without a fresh frame (USING) installs the overlay as the slot's
                                // scope — a `YokedChild` whose `outer` chain pins the overlay's
                                // cart-ancestor region — otherwise the slot keeps its scope. Mint a
                                // fresh anchor only when the overlay scope or the chain changed; a pure
                                // `ParkThenContinue` (same cart, scope, and chain) keeps the anchor.
                                let scope = overlay_scope.map_or(node_scope, NodeScope::YokedChild);
                                let anchor_arg = if overlay_scope.is_some() || chain_changed {
                                    Some(super::nodes::SlotFrame::new(
                                        Rc::clone(&prev_frame),
                                        scope,
                                        new_chain,
                                    ))
                                } else {
                                    None
                                };
                                self.sched.replace(id, new_work, anchor_arg);
                            }
                        }
                    }
                    NodeStep::Alias(producer) => {
                        // The slot spliced itself out as a bare-name forward: move its consumers onto
                        // `producer` and alias it for reads — not re-queued; `producer`'s fire wakes
                        // them. See `scheduler::splice`.
                        self.sched.splice_forward(id, producer);
                    }
                }
            },
        );
    }
}
