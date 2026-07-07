//! The Koan driver over the workload-independent [`Scheduler`](crate::scheduler::Scheduler): the
//! run loop ([`KoanRuntime::execute`]) pops ready slots and hands each to [`run_step`](KoanRuntime::run_step),
//! which brackets the step's ambient frame context end-to-end and applies the [`NodeStep`] it returns
//! through the scheduler's method contract. The scheduler stores and hands back opaque per-node state;
//! all Koan semantics — the per-call region lift, the return-contract enforcement, the lexical-chain
//! assembly — live here.
//!
//! See design/execution/README.md and design/memory-model.md.

use std::rc::Rc;

use crate::machine::core::kfunction::action::scope_frame;
use crate::machine::core::kfunction::body::ErasedContract;
use crate::machine::core::{FrameStorage, KoanRegionExt};
use crate::machine::model::values::CarriedFamily;
use crate::machine::{CarrierWitness, FrameSet, KError, KErrorKind, KoanRegion, NodeId, RegionBrand};
use crate::witnessed::{erase_to_static, reattachable, seal_option, SealedExtern, Witnessed};

use super::dispatch::SchedulerView;
use super::finalize::{finalize_error, NodeFinalize};
use super::nodes::{Node, NodeFrame, NodePayload, NodeScope, NodeStep};
use super::outcome::DepTerminal;
use super::runtime::{KoanRuntime, KoanWorkload};

#[cfg(test)]
mod run_tests;
#[cfg(test)]
mod tests;

/// `Reattachable` family for a destination region's [`RegionBrand`] — the carrier a
/// [`ForwardReady`](NodeStep::ForwardReady) relocation feeds to
/// [`Sealed::transfer_into`](crate::witnessed::Sealed::transfer_into) to re-anchor the relocated value
/// at the destination's lifetime, allocating the copy through the brand.
/// Layout-invariant: a [`RegionBrand`] is a thin pointer whose representation never depends on `'r`,
/// so the shared `reattachable!` macro discharges the obligation.
pub(in crate::machine::execute) struct RegionRefFamily;

reattachable!(RegionRefFamily => RegionBrand<'r>);

/// The destination-region carrier for a relocation: `dest_frame`'s brand `yoke`d into that frame's
/// own region, witnessed by it — co-located by construction rather than paired with an asserted
/// singleton.
pub(in crate::machine::execute) fn dest_brand(
    dest_frame: Rc<FrameStorage>,
) -> Witnessed<RegionRefFamily, CarrierWitness> {
    KoanRegion::yoke_branded::<RegionRefFamily, _>(dest_frame, |b| b)
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
/// never depends on `'r` (`FrameSet` / `KError` are lifetime-free), so the `reattachable!` macro
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
            let node = self.sched.take_for_run(id);
            self.run_step(id, node);
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
    /// [`SealedExtern::open`] opens the continuation, return contract, active scope, and dep slice
    /// together at `'b`, witnessed by `combined` (the held cart `Rc` unioned with the dep `pin`), which
    /// the bracket keeps live across the run. The consumer `dest` region is the opened scope's own
    /// region. The closure's result cannot name `'b`, so the `Outcome<'b>` and the finalized
    /// `Carried<'b>` are erased into the slot store *before* return: a value born at `'b` never has to
    /// launder to `'run` to cross the bracket exit, and nothing branded escapes. The cart clone is
    /// confined to this call and dropped at return — before the next iteration's `try_reset_for_tail`
    /// resets a *different* cart — so it does not contend with the TCO `Rc::get_mut` uniqueness gate.
    fn run_step(&mut self, id: NodeId, node: Node<KoanWorkload>) {
        let idx = id.index();
        // Read the scope as a value up front: nothing holds a scope borrow across the step's
        // `&mut self` work or the in-step TCO frame reset.
        let node_scope = node.payload.scope;
        let NodeFrame {
            cart,
            reserve,
            contract: prev_contract,
        } = node.frame;
        let prev_chain_carrier = node.payload.chain;
        let (deps, erased_continuation, _carrier) = node.work.into_run_parts();
        // The step's open witness: a step-confined cart clone, dropped at return so it does not
        // contend with the TCO `Rc::get_mut` gate. The tail open re-anchors the step's carriers to the
        // brand `'b` this witness pins, and owns that reattach.
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
                // The producer slot's own `Sealed` carrier (duplicated so a construction finish folds
                // the dep witnessed) plus the live value erased out of it. One slot read; an errored
                // slot short-circuits here.
                let carrier = self.sched.dep_carrier(d).map_err(|e| e.clone())?;
                let value = carrier.open(|live| erase_to_static::<CarriedFamily>(live));
                Ok(DepTerminal { value, carrier })
            })
            .collect();
        // The consumer-step **pin**: the union of every region this step's deps reach (an errored dep
        // has no `DepTerminal`, so its witness is re-read). Assembled before the open so it outlives
        // `'b`, then unioned into `combined` — the witness the open re-anchors carriers against, keeping
        // every dep source alive past `reclaim_deps`. It is *only* a liveness pin: every value terminal
        // rides `DoneWitnessed` with its own carrier naming its reach, so no terminal reads `pin`.
        let pin: FrameSet = dep_sources.iter().zip(deps.all_ids()).fold(
            FrameSet::empty(),
            |acc, (src, d)| match src {
                Ok(t) => FrameSet::union(&acc, &t.carrier.witness().to_liveness_frameset()),
                Err(_) => {
                    FrameSet::union(&acc, &self.sched.dep_witness(d).to_liveness_frameset())
                }
            },
        );
        // The open witness: the start cart (pinning the continuation, contract, and dest region — plus
        // their ancestor backings via its `outer` chain) unioned with `pin` (every dep source). Held
        // across the open, so re-anchoring the zipped carriers to `'b` cannot dangle. A plain `FrameSet`
        // (§ the run-loop step-open witness is a plain frame set): a severed dep's owned node isn't a
        // frame and doesn't ride it — it is pinned instead by the dep's duplicated `Sealed` carrier held
        // across the whole open in `dep_sources` (see the struct doc above), never by this set.
        let combined: FrameSet = FrameSet::union(
            &FrameSet::singleton(continuation_witness.storage_rc()),
            &pin,
        );
        // Open the four externally-witnessed carriers — continuation, frame-gated contract, active
        // scope, dep slice — together at one rank-2 `for<'b>` brand witnessed by `combined` (see the
        // doc comment for why nothing branded escapes).
        let continuation = SealedExtern::seal(erased_continuation);
        let contract = seal_option(prev_contract);
        // The active scope as a carrier, per node-scope shape: `Yoked` takes the start cart's own
        // child-scope carrier; `YokedChild` reuses the carrier it already holds. `combined` pins both.
        let scope_carrier = match node_scope {
            NodeScope::Yoked => continuation_witness.scope_sealed(),
            NodeScope::YokedChild(carrier) => carrier,
        };
        let dep_carrier = SealedExtern::<DepResultsFamily>::erase(dep_sources);
        continuation
            .zip(contract)
            .zip(scope_carrier)
            .zip(dep_carrier)
            .open(
                &combined,
                |(((continuation, live_contract), scope), dep_sources)| {
                    // `scope` is now live at `'b` and the `dest` region is its own region; deps arrive
                    // un-relocated. A `ForwardReady` relocation below builds its destination carrier
                    // from this same scope's brand.
                    //
                    // Bracket the step's ambient frame/reserve/payload and contract-chain flag —
                    // whether this slot is a tail call within an established contract chain, so a
                    // deferred-return FN dispatched here skips resolving its own return type
                    // (keep-first discards it anyway) — restored on every exit path, including
                    // unwinds, by `with_slot_step` itself.
                    let (step, post) = self.with_slot_step(
                        cart,
                        reserve,
                        NodePayload {
                            scope: node_scope,
                            chain: prev_chain_carrier.clone(),
                        },
                        prev_contract.is_some(),
                        |rt| {
                            let outcome = continuation(
                                &SchedulerView::new(
                                    &rt.sched,
                                    &rt.ambient,
                                    scope,
                                    scope_frame(scope),
                                ),
                                deps.results(&dep_sources),
                                idx,
                            );
                            rt.sched.reclaim_deps(idx, owned_indices);
                            // Realize the outcome into a `NodeStep`; a ready `Outcome::Forward` becomes
                            // a `ForwardReady` relocated below into this same `dest`.
                            rt.apply_outcome(outcome, idx)
                        },
                    );
                    // Drain re-entrant writes against the step scope (unchanged by the step).
                    scope.drain_pending();
                    // The producer's per-call frame, gated to a *dying* producer (a frameless / run-frame
                    // producer folds in nothing): folded into a value terminal's witness and the
                    // destination pin for a `ForwardReady` relocation.
                    let frame = (!post.prev_frame.non_dying()).then_some(&post.prev_frame);
                    match step {
                        NodeStep::DoneWitnessed(carrier) => {
                            // The value terminal arrives already witnessed, naming its reach. The
                            // contract opened at the brand is a live `ReturnContract<'b>`; the `frame`
                            // gate drops it for a frameless / run-frame producer (no per-call return
                            // obligation). `finalize_terminal` folds the producing frame into the witness
                            // and re-stamps a contract-coarsened value into the contract's home region.
                            let live_contract = frame.and(live_contract);
                            let result = self.finalize_terminal(carrier, frame, live_contract);
                            if result.is_err() {
                                scope.clear_placeholders_for_producer(id);
                            }
                            self.sched.finalize(idx, result);
                        }
                        NodeStep::Error(error) => {
                            // An error finalizes bare (no value, no witness); the frame-gated contract
                            // still labels it with the callee's trace frame.
                            let live_contract = frame.and(live_contract);
                            let error = finalize_error(error, frame, live_contract);
                            scope.clear_placeholders_for_producer(id);
                            self.sched.finalize(idx, Err(error));
                        }
                        NodeStep::ForwardReady(producer) => {
                            // Relocate `producer`'s terminal into this slot's region via merge-transfer;
                            // no contract re-check (the producer enforced its own). Framed: the dest
                            // brand is `yoke`d into its owning frame, witnessed by it. Frameless: the
                            // dest region is externally pinned for the step, so a confined empty-set
                            // `resident` carries it. A ready-but-errored producer relocates to an `Err`.
                            let dest = match frame {
                                Some(f) => dest_brand(f.storage_rc()),
                                None => Witnessed::<RegionRefFamily, CarrierWitness>::resident(
                                    scope.brand(),
                                ),
                            };
                            let result = self.relocate_terminal(producer, dest);
                            if result.is_err() {
                                scope.clear_placeholders_for_producer(id);
                            }
                            self.sched.finalize(idx, result);
                        }
                        NodeStep::Replace {
                            work: new_work,
                            frame: new_frame,
                            contract: new_contract,
                            chain,
                            overlay_scope,
                        } => {
                            let prev_frame = post.prev_frame;
                            let post_step_reserve = post.post_step_reserve;
                            // Keep the **first** contract of a tail chain: a nested tail call does not
                            // overwrite an established contract, so the chain checks the original caller's
                            // declared return, not the tail-most callee's. (`prev_contract` is `Copy`, so
                            // opening it into `live_contract` above left the erased original intact.)
                            let next_contract: Option<ErasedContract> =
                                prev_contract.or(new_contract);
                            // The frame the body runs in: a freshly installed cart, else the slot's
                            // current one (a `FramePlacement::Inherit` FN-body re-enters the cart a prior
                            // `Continue` installed). The `ChainOp` reads it to walk the body's lexical chain.
                            let body_frame: &crate::machine::core::CallFrame =
                                new_frame.as_deref().unwrap_or(&prev_frame);
                            let new_chain = chain.apply(prev_chain_carrier, body_frame);
                            match new_frame {
                                Some(f) => {
                                    // A framed tail re-projects `Yoked` from its own cart; the overlay
                                    // scope is the frameless (`Inherit`) path.
                                    debug_assert!(
                                        overlay_scope.is_none(),
                                        "a framed tail-replace carries no overlay scope"
                                    );
                                    // Rotate the ping-pong reserve: drop the superseded post-step reserve.
                                    drop(post_step_reserve);
                                    // The non-dying run frame is not a reusable per-call region; parking
                                    // it as the reserve would mis-time a real frame's drop. Treat it as no
                                    // reserve — the run scope is re-reached via the scheduler's `run_frame`.
                                    let new_reserve =
                                        (!prev_frame.non_dying()).then_some(prev_frame);
                                    // The slot's scope is always this `f` cart's own child, so store a
                                    // payload-less `NodeScope::Yoked` re-projected at the read boundary —
                                    // no persisted `&'run` to dangle across a TCO reset.
                                    self.sched.replace(
                                        id,
                                        Node {
                                            work: new_work,
                                            payload: NodePayload {
                                                scope: NodeScope::Yoked,
                                                chain: new_chain,
                                            },
                                            frame: NodeFrame {
                                                cart: f,
                                                reserve: new_reserve,
                                                contract: next_contract,
                                            },
                                        },
                                    );
                                }
                                None => {
                                    // A frameless Replace keeps the prior cart. A tail entering an overlay
                                    // without a fresh frame (USING) installs the overlay as the slot's
                                    // scope — a `YokedChild` whose `outer` chain pins the overlay's
                                    // cart-ancestor region — otherwise the slot keeps its scope.
                                    let scope =
                                        overlay_scope.map_or(node_scope, NodeScope::YokedChild);
                                    self.sched.replace(
                                        id,
                                        Node {
                                            work: new_work,
                                            payload: NodePayload {
                                                scope,
                                                chain: new_chain,
                                            },
                                            frame: NodeFrame {
                                                cart: prev_frame,
                                                reserve: post_step_reserve,
                                                contract: next_contract,
                                            },
                                        },
                                    );
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
