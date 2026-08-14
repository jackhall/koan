//! The Koan driver over the workload-independent [`Scheduler`](crate::scheduler::Scheduler): the
//! run loop ([`KoanRuntime::execute`]) pops ready slots and hands each to [`run_step`](KoanRuntime::run_step),
//! which brackets the step's ambient frame context end-to-end and applies the [`NodeStep`] it returns
//! through the scheduler's method contract. The scheduler stores and hands back opaque per-node state;
//! all Koan semantics — the per-call region lift, the return-contract enforcement, the lexical-chain
//! assembly — live here.
//!
//! See design/execution/README.md and design/memory-model.md.

use std::rc::Rc;

use crate::machine::ProducerId;
use crate::machine::core::bindings::{WriteGate, WriteOp};
use crate::machine::core::scope_frame;
use crate::machine::core::{FrameStorage, KoanRegionExt, KoanStorageProfile};
use crate::machine::{
    CarrierWitness, FrameCoverage, Installer, KError, KErrorKind, KoanRegion, NodeId,
};
use crate::scheduler::SealedTerminal;
use crate::witnessed::{Delivered, RegionHandleFamily};

use super::dispatch::SchedulerView;
use super::finalize::{NodeFinalize, finalize_error};
use super::nodes::{ChainOp, NodePayload, NodeScope, NodeStep, StoredWork};
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
    KoanRegion::yoke_branded::<DestHandleFamily, _>(dest_frame, |b| b.handle())
}

impl<'run> KoanRuntime<'run> {
    /// On `Done` with a frame, the return `Value` references the per-call region that's
    /// about to drop, so it must be lifted into the captured scope's region before the
    /// frame is released. See design/memory-model.md.
    pub fn execute(&mut self) -> Result<(), KError> {
        while let Some(id) = self.sched.pop_next() {
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

    /// Release the edges this slot owns, and first drop any pending binding arm still naming one.
    /// Runs wherever the slot stops being able to release them — every terminal, and the splice
    /// that retires the slot as an alias. A successful binder's write path finalizes its own claim
    /// in place, so the clear usually finds nothing; the release is owed either way, and the clear
    /// is what keeps a table from ever holding a [`ProducerId`] whose edge is gone — the tables know
    /// these names as producers, so the release list is read back as such. `take` empties the
    /// anchor, so a slot's edges are retired exactly once.
    fn retire_slot_edges(
        &mut self,
        scope: &crate::machine::Scope<'_>,
        anchor: &super::nodes::SlotFrame,
    ) {
        let edges = anchor.take_owned_edges();
        if edges.is_empty() {
            return;
        }
        let producers: Vec<ProducerId> = edges
            .iter()
            .copied()
            .map(ProducerId::from_scheduler_edge)
            .collect();
        scope.clear_placeholders_for_producers(&producers, &mut WriteGate::for_run_loop());
        for edge in edges {
            self.sched.release_edge(edge);
        }
    }

    /// The unified node handler, owning one slot step start to finish: collect the resolved dep
    /// terminals, then bracket the step's ambient frame context around running the continuation
    /// against a read-only [`SchedulerView`], reclaiming the owned-dep suffix, and applying the
    /// [`Outcome`] into a [`NodeStep`], before realizing the step. The step's ambient context is
    /// bracketed by [`KoanRuntime::with_slot_step`] inside the `open`, so no exit path — return or
    /// unwind — leaves it installed.
    ///
    /// The step tail runs inside one rank-2 `for<'b>` brand standing in for the step lifetime: the
    /// owned tier's one open verb ([`SealedPinned::open`](crate::witnessed::SealedPinned::open))
    /// opens the sealed continuation and the active scope operand together at `'b`, witnessed by the
    /// seal's own bundled anchor pin plus `combined` (the held cart `Rc` unioned with every region
    /// the step's deps reach), which the bracket keeps live across the run. Dep terminals ride the
    /// step as plain lifetime-free delivery envelopes, not through this open. The consumer `dest`
    /// region is the opened scope's own region. The closure's result cannot name `'b`, so the
    /// `Outcome<'b>` and the finalized `Carried<'b>` are erased into the slot store *before* return:
    /// a value born at `'b` never has to launder to `'run` to cross the bracket exit, and nothing
    /// branded escapes. The step's cart clone is confined to this call and dropped at return; a
    /// `FreshTail` placement for the next iteration mints an entirely fresh cart, so nothing aliases
    /// across the boundary.
    fn run_step(
        &mut self,
        id: NodeId,
        work: StoredWork<KoanWorkload>,
        anchor: Rc<super::nodes::SlotFrame>,
        handoff: Option<Rc<super::nodes::SlotFrame>>,
    ) {
        // Source the step's context off the scheduler-held anchor: the cart, the slot's scope
        // handle, and its lexical chain. Read as values up front so nothing holds a scope borrow
        // across the step's `&mut self` work or a tail hop's frame swap.
        let cart = Rc::clone(&anchor.cart);
        // A second handle to the same anchor, for the apply that runs inside the step open: the
        // harness records the edges it wires there onto the slot that owns them.
        let step_anchor = Rc::clone(&anchor);
        let node_scope = anchor.payload.scope;
        let prev_chain_carrier = anchor.payload.chain.clone();
        let (deps, sealed_continuation, _carrier) = work.into_run_parts();
        // The step's open witness: a step-confined cart clone, dropped at return. The tail open
        // re-anchors the step's carriers to the brand `'b` this witness pins, and owns that reattach.
        let continuation_witness = Rc::clone(&cart);
        // **Step start is a read, not a graph walk.** Every dep was delivered into its edge's
        // destination when its producer finalized, so each resident is duplicated straight off the
        // edge — a `Copy` cell whose pointee already lives in a region this step covers. An errored
        // dep short-circuits into the slice.
        let residents: Vec<Result<SealedTerminal<KoanWorkload>, KError>> = deps
            .all_ids()
            .map(|e| self.sched.edge_resident(e))
            .collect();
        // The slot is done with its dep edges the moment their residents are in hand: the values live
        // in the destination regions, not in the edges, so releasing here frees the slab entries
        // while the step still runs. A `Replace` installs fresh edges for the next incarnation, so
        // nothing is released twice.
        for edge in deps.all_ids() {
            self.sched.release_edge(edge);
        }
        // The step's open witness — the **step's coverage**: the anchor's projected region owner,
        // which pins the continuation and the dest region plus their ancestor backings via the
        // storage `outer` chain. That is the whole of it, and it already covers every dep: an owned
        // dep's edge is destined at this slot's own anchor region, and a park's inherits the region
        // its source named — a scope region this anchor's `outer` chain pins. Assembled before the
        // open so it outlives `'b`, and held across it, so re-anchoring the zipped carriers to `'b`
        // cannot dangle. Sourced off the scheduler-returned anchor, not a `storage_rc()` of the cart
        // the scheduler already holds.
        let mut combined: FrameCoverage = FrameCoverage::of(Rc::clone(anchor.owner()));
        // A framed tail replace's retiring incarnation, held for this step alone. It covers the
        // splice cells this slot's previous incarnation rested into that region — the reads
        // `SchedulerView::lift_spliced` takes — and is released when the coverage drops at return,
        // ordering the retiring region's free after the adoption.
        if let Some(retiring) = &handoff {
            combined.absorb(FrameCoverage::of(Rc::clone(retiring.owner())));
        }
        // Re-brand each delivered resident **once**, here, against the step's coverage: a retained
        // cell proves no liveness of its own, and `combined` is exactly the pin covering every
        // region a dep landed in. From this point the step's readers open pin-free — a dep value
        // rides no shared step brand and needs no envelope of its own.
        let dep_sources: Vec<Result<DepTerminal<'_>, KError>> = residents
            .iter()
            .map(|resident| match resident {
                Ok(cell) => Ok(DepTerminal {
                    cell: cell.brand_with(&combined),
                }),
                Err(error) => Err(error.clone()),
            })
            .collect();
        // The active scope as a carrier, per node-scope shape: `Yoked` takes the start cart's own
        // child-scope carrier; `YokedChild` reuses the carrier it already holds. `combined` pins both.
        let scope_carrier = match node_scope {
            NodeScope::Yoked => continuation_witness.scope_sealed(),
            NodeScope::YokedChild(carrier) => carrier,
        };
        // Open the owned-tier continuation beside the active-scope operand at one rank-2 `for<'b>`
        // brand: the seal carries its own anchor pin, and `combined` witnesses the operand (see the
        // doc comment for why nothing branded escapes). The `Within` token's declared `'run: 'b` is
        // what lets `rt.program` — a live borrow-checked `ProgramBrand<'run>`, not a sealed
        // carrier — be stored in the view at its own `'program = 'run`, discharging the
        // `SchedulerView`'s `'program: 'step` bound without shortening the brand.
        sealed_continuation.open(
            scope_carrier,
            &combined,
            |_within: crate::witnessed::Within<'_, 'run>,
             continuation: super::outcome::NodeContinuation<'_>,
             scope| {
                // The step's binding-write sink: every `Action` the step interprets deposits its
                // `WriteOp`s here through `run_action`, and the drain below applies them against
                // the step scope. Declared inside the step brand because a `WriteOp` carries seals
                // branded to `scope`'s region — so "nothing crosses steps" is the borrow checker's
                // rule here, not a convention.
                let step_effects: std::cell::RefCell<Vec<WriteOp<'_>>> =
                    std::cell::RefCell::new(Vec::new());
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
                                Installer::Statement(anchor.statement()),
                                &step_effects,
                                rt.program,
                            ),
                            deps.results(&dep_sources),
                            id,
                        );
                        // Apply the step's binding writes against the step scope, in the order the
                        // bodies decided them. This is the **only** path that mutates a published
                        // binding table: it runs after the continuation returned — so no koan frame
                        // holds a competing borrow — and before the outcome is realized, so the
                        // writes land while the scope is still open and before any graph edge an
                        // errored step would strand is installed. On the first failure the
                        // remaining ops are dropped and the step becomes the node's error terminal,
                        // so the finalize arms below drop the producer's pending arms and
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
                        // a `ForwardReady` relocated below into this same `dest`. The step scope's
                        // brand rides along: an owned dep the outcome names may still have to bump
                        // its own dispatch node (an aggregate literal's elements, a body block's
                        // statements) into this region as it is realized.
                        rt.apply_outcome(outcome, scope.brand(), id, &step_anchor)
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
                        // `finalize_terminal` hands the envelope on whole; the delivery walk reads
                        // its coverage to adopt into each destination, so no consumer re-derives the
                        // reach and no call site here names one.
                        let finalized = self.finalize_terminal(
                            envelope,
                            anchor.owner(),
                            frame.and(post.obligation.as_ref()),
                        );
                        self.retire_slot_edges(scope, &anchor);
                        match finalized {
                            Ok(delivered) => self.sched.finalize(id, Ok(delivered)),
                            Err(error) => self.sched.finalize(id, Err(error)),
                        }
                    }
                    NodeStep::Error(error) => {
                        // An error finalizes bare (no value, no witness); the frame-gated
                        // obligation still labels it with the callee's trace frame.
                        let error = finalize_error(error, frame.and(post.obligation.as_ref()));
                        self.retire_slot_edges(scope, &anchor);
                        // A terminal error carries no value and so reaches nothing; the producer
                        // frame still retains until its (short-circuiting) destinations pull.
                        self.sched.finalize(id, Err(error));
                    }
                    NodeStep::ForwardReady(edge) => {
                        // The slot's result *is* the value already resting on `edge`. Delivery does
                        // the rest: the scheduler lifts that resident back into an envelope under
                        // its own destination's owner and runs this slot's ordinary walk over it, so
                        // a forward costs one relocation per distinct onward destination and no
                        // contract re-check (the producer enforced its own).
                        //
                        // Ordered before the retirement: the forward's own classification edge is on
                        // this slot's owned list, and the read above goes through it.
                        self.sched.finalize_forward(id, edge);
                        self.retire_slot_edges(scope, &anchor);
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
                                // The claims and the statement identity belong to the slot, not to
                                // the anchor it happens to be wearing, so the incoming anchor takes
                                // them over from the retiring one: the terminal that eventually
                                // retires the edges still finds them, and a binding installed after
                                // the hop is stamped with the same statement as one installed before.
                                // `opening` rather than `replacing` because `f` is a cart minted for
                                // this slot: the slot opens its scope here and closes it at its own
                                // finish.
                                let fresh = super::nodes::SlotFrame::opening(
                                    f,
                                    NodeScope::Yoked,
                                    new_chain,
                                    anchor.as_ref(),
                                );
                                self.sched.replace(id, new_work, Some(fresh));
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
                                    Some(super::nodes::SlotFrame::replacing(
                                        Rc::clone(&prev_frame),
                                        scope,
                                        new_chain,
                                        anchor.as_ref(),
                                    ))
                                } else {
                                    None
                                };
                                self.sched.replace(id, new_work, anchor_arg);
                            }
                        }
                    }
                    NodeStep::Alias(edge) => {
                        // The slot spliced itself out as a bare-name forward: move its consumers onto
                        // the producer behind `edge` and alias it for reads — not re-queued; that
                        // producer's fire wakes them. See `scheduler::splice`. An alias never
                        // terminalizes, so this is where its owned edges are retired.
                        self.sched.splice_forward_from(id, edge);
                        self.retire_slot_edges(scope, &anchor);
                    }
                }
            },
        );
    }
}
