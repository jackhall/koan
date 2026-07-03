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
use crate::machine::{FrameSet, KError, KErrorKind, KoanRegion, NodeId, RegionBrand};
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

/// `Reattachable` family for a destination region's [`RegionBrand`] — the destination-region carrier
/// the consumer-pull lift's `read_lifted` feeds to
/// [`Sealed::transfer_into`](crate::witnessed::Sealed::transfer_into) when it re-anchors a relocated
/// value at the destination's lifetime, allocating the copy through the brand. The step's own `dest`
/// brand rides the opened scope (`scope.brand()`) rather than a separate carrier, so this family backs
/// only the relocate seam. Layout-invariant: a [`RegionBrand`] is a thin pointer whose representation
/// never depends on `'r`.
pub(in crate::machine::execute) struct RegionRefFamily;

// `RegionBrand<'r>` is one type generic only in `'r` (a thin reference newtype); its layout is
// identical for every `'r`, so the shared `reattachable!` macro discharges the obligation.
reattachable!(RegionRefFamily => RegionBrand<'r>);

/// The destination-region carrier for a relocation: `dest_frame`'s brand `yoke`d into that frame's
/// own region, witnessed by it — co-located by construction rather than paired with an asserted
/// singleton. The one owner of the `RegionRefFamily` dest-brand construction.
pub(in crate::machine::execute) fn dest_brand(
    dest_frame: Rc<FrameStorage>,
) -> Witnessed<RegionRefFamily, FrameSet> {
    KoanRegion::yoke_branded::<RegionRefFamily, _>(dest_frame, |b| b)
}

/// `Reattachable` family for the step's **dep slice** — the producer terminals read out, erased, and
/// zipped into the step `open` so they arrive at the brand `'b` alongside the continuation. This is
/// the only sound route to the unbounded `'b`: a value opened in-band rides the audited
/// [`Erased::reattach`](crate::witnessed) the open already owns, where a witness-*borrow*-bounded
/// reattach cannot — capping the produced lifetime at the witness borrow, it would demand the
/// step pin outlive a *universally* quantified `'b`, i.e. be `'static`. The held step witness keeps the
/// sources alive across the open; the brand confines the values to it. Each cell is a
/// [`DepTerminal`](super::outcome::DepTerminal) — the resolved value plus its `reach` set — so the
/// reach rides the slice to the construction site without a parallel channel. Layout-invariant: a
/// `Vec<Result<DepTerminal<'r>, KError>>` is a `Vec` of cells (two pointers + a lifetime-free
/// `FrameSet`, or an error) whose representation never depends on `'r`.
pub(in crate::machine::execute) struct DepResultsFamily;

// `Vec<Result<DepTerminal<'r>, KError>>` is one type generic only in `'r` (a `Vec` of layout-invariant
// cells; `FrameSet` / `KError` are lifetime-free), so the shared `reattachable!` macro discharges the
// obligation.
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
        // Any slot still `PreRun` after drain is parked on a dependency that can
        // no longer fire — surface the cycle rather than panic on the caller's
        // top-level result read.
        if let Some((pending, sample)) = self.sched.unresolved() {
            return Err(KError::new(KErrorKind::SchedulerDeadlock {
                pending,
                sample,
            }));
        }
        Ok(())
    }

    /// The unified node handler, owning one slot step start to finish: enter the step's ambient frame
    /// context, collect the resolved dep terminals (as owned `Result`s — an errored dep is handed
    /// through, the continuation decides), run the slot's continuation against a read-only
    /// [`SchedulerView`], reclaim the owned-dep suffix, apply the decided [`Outcome`] into a
    /// [`NodeStep`], exit the step guard, then realize that step. The continuation issues no graph
    /// write, so the reclaim lands after it and before the apply that installs the continuation's
    /// edges. The whole bracket — enter and exit — lives here, so the
    /// [`SlotStepGuard`](super::ambient::SlotStepGuard) is born and consumed without escaping.
    ///
    /// The whole step tail runs inside one rank-2 `for<'b>` brand standing in for the step lifetime
    /// `'s`: [`SealedExtern::open`] opens the continuation, the return contract, the active scope, and
    /// the dep slice together at `'b` witnessed by `combined` (the held cart `Rc` `continuation_witness`
    /// unioned with the dep `pin`), which the step guard keeps live across the continuation's run. The
    /// consumer `dest` region is the opened scope's own region (`scope.region`), and an
    /// `Outcome::Forward` pull is born at `'b` into it. The body, including the
    /// [`NodeStep::DoneWitnessed`] terminal's finalize, runs while the cart `Rc` witnesses the
    /// value-channel pull-lift. So the terminal
    /// value, born at `'b` in the consumer frame, is finalized into the slot store (where `finalize`
    /// erases it) *within* `'b`: it never has to be laundered to `'run` to cross a step-guard exit, and
    /// nothing branded escapes the closure. The cart clone is confined to this call and dropped at
    /// return — before the next iteration's `try_reset_for_tail`, which resets a *different* (the prior
    /// step's) cart — so it does not contend with the TCO `Rc::get_mut` uniqueness gate.
    fn run_step(&mut self, id: NodeId, node: Node<KoanWorkload>) {
        let idx = id.index();
        // The step reads its scope on demand (`current_scope`), and the post-step uses below
        // re-acquire it per use, so nothing holds a scope borrow across the step's `&mut self`
        // work or the in-step TCO frame reset.
        let node_scope = node.payload.scope;
        let NodeFrame {
            cart,
            reserve,
            contract: prev_contract,
        } = node.frame;
        let prev_chain_carrier = node.payload.chain;
        let (deps, erased_continuation, _carrier) = node.work.into_run_parts();
        // Hold the cart as the step's open witness across the whole step: a step-confined clone,
        // dropped at return — before the next iteration's `try_reset_for_tail`, which resets a
        // *different* (the prior step's) cart — so it does not contend with the TCO `Rc::get_mut`
        // gate. The tail `open` (below) re-anchors the step's carriers to the brand `'b` this witness
        // pins; the open owns that reattach, so no fabricated free `'_` lives here.
        let continuation_witness = Rc::clone(&cart);
        let guard = self.enter_slot_step(
            cart,
            reserve,
            NodePayload {
                scope: node_scope,
                chain: prev_chain_carrier.clone(),
            },
        );
        // Expose to the dispatch step whether this slot is a tail call within an established contract
        // chain — a deferred-return FN dispatched here skips resolving its own return type (keep-first
        // discards it anyway).
        self.ambient.active_in_contract_chain = prev_contract.is_some();
        // Consumer-pull: lift each dep's terminal out of its producer frame into this consumer's own
        // scope region, so the value dies with the consumer and the producer keeps no surviving copy
        // that would outlive its own dying frame. A frameless / run-region terminal already survives
        // and is forwarded as-is. The consumer `dest` region is the *scope's* region, derived inside
        // the step `open` from the opened scope (`scope.region`) — the right region even for a
        // transparent USING window, whose scope region differs from the active frame's. The relocation
        // that copies each dep into `dest` runs inside the consuming continuation (the
        // `relocate_values` projection `short_circuit` runs, or `catch`), not here: the lift delivers
        // deps un-relocated, so a construction finish folds their carriers and a value-copy finish
        // copies the spine.
        let owned_indices: Vec<usize> = deps.owned().iter().map(|d| d.index()).collect();
        // Read each producer terminal out (borrow-bounded) into the dep slice — the resolved value
        // plus its `reach` set (its slot witness). The slice erases into one carrier that opens
        // **in-band** at `'b` alongside the continuation (the only sound route to the unbounded brand,
        // see `DepResultsFamily`); the sources stay pinned by `combined` across the open.
        let dep_sources: Vec<Result<DepTerminal<'static>, KError>> = deps
            .all_ids()
            .map(|d| {
                // The producer slot's own `Sealed` carrier (duplicated, so a construction finish folds
                // the dep witnessed), and the live value sourced from *it* — opened at a brand and
                // erased for storage, re-anchored to the step brand by the open below for the bare
                // value-copy relocate. One slot read: an errored slot short-circuits here.
                let carrier = self.sched.dep_carrier(d).map_err(|e| e.clone())?;
                let value = carrier.open(|live| erase_to_static::<CarriedFamily>(live));
                Ok(DepTerminal { value, carrier })
            })
            .collect();
        // The consumer-step **pin**: the set union of every region this step's deps reach, read off
        // the dep terminals' `reach` above (an errored dep has no `DepTerminal`, so its witness is
        // re-read). Assembled *before* the open so it outlives the brand `'b`; unioned with the cart
        // into `combined` below, it is the witness the step open re-anchors its carriers against,
        // keeping every dep source alive past `reclaim_deps`. It must cover every read dep (each is
        // opened at `'b`), so it spans the whole dep slice. `pin` is *only* a liveness pin: no terminal
        // reads it as a witness — every value terminal rides `DoneWitnessed` with its own carrier
        // naming exactly the regions the output reaches, and an error carries no value witness. A
        // single-dep forward re-seals off the forwarded dep's own reach, not `pin`.
        let pin: FrameSet = dep_sources.iter().zip(deps.all_ids()).fold(
            FrameSet::empty(),
            |acc, (src, d)| match src {
                Ok(t) => FrameSet::union(&acc, t.carrier.witness()),
                Err(_) => FrameSet::union(&acc, &self.sched.dep_witness(d)),
            },
        );
        // The step's open witness: the start cart (which pins the continuation, the contract, and the
        // consumer `dest` region — and, via its `outer` chain, their run / ancestor backings) unioned
        // with `pin` (which pins every dep source). Held across the whole open, so re-anchoring the
        // zipped carriers — including the dep slice — to `'b` cannot dangle.
        let combined: FrameSet = FrameSet::union(
            &FrameSet::singleton(continuation_witness.storage_rc()),
            &pin,
        );
        // Open the step's externally-witnessed carriers — the continuation, the (frame-gated) return
        // contract, the active scope, and the dep slice — together at a single rank-2 `for<'b>` brand
        // standing in for the step lifetime `'s`, witnessed by `combined`. The brand is generative:
        // nothing outside is at `'b`, so every value the tail consumes is opened here — including the
        // scope, from which the consumer `dest` region is derived. The closure's result cannot name
        // `'b`, so the `Outcome<'b>` the continuation returns and the finalized `Carried<'b>` are
        // consumed in place — erased into the slot store before return — and nothing branded crosses
        // the step boundary. This is what lets the tail carry **no** loose witness-borrow reattach:
        // continuation, scope, dep slice, and contract all route this one open.
        let continuation = SealedExtern::seal(erased_continuation);
        let contract = seal_option(prev_contract);
        // The active scope as an externally-witnessed carrier, for both node-scope shapes: a `Yoked`
        // slot takes the start cart's own child-scope carrier; a `YokedChild` already holds its own
        // `SealedExtern<ScopeRefFamily>` carrier and reuses it directly. `combined` pins both — the
        // cart's region for `Yoked`, an ancestor via the cart's `outer` chain for `YokedChild`.
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
                    // The active `scope` is now live at the brand `'b`, and the consumer `dest` region
                    // is its own region. Deps arrive un-relocated at `'b` from the opened carrier (read
                    // out of their producer slots, which `combined` pins across the open). The consuming
                    // continuation relocates each into `dest` — the `relocate_values` projection (or
                    // `catch`) for a value-copy finish, the construction inversion's `transfer_into`
                    // fold for an aggregate (which relocates once and names every reached region on
                    // the carrier).
                    // The lift itself no longer pre-relocates. A `ForwardReady` relocation below builds
                    // its own witnessed destination carrier from this same scope's brand.
                    let outcome = continuation(
                        &SchedulerView::new(&self.sched, &self.ambient, scope, scope_frame(scope)),
                        deps.results(&dep_sources),
                        idx,
                    );
                    self.sched.reclaim_deps(idx, owned_indices);
                    // `apply_outcome` realizes the outcome into a `NodeStep`; a ready `Outcome::Forward`
                    // becomes a `ForwardReady` relocated below at the brand into this same `dest`. `pin`
                    // (the dep sources) is now purely the step's liveness pin — held across the open — not
                    // a terminal witness: every value terminal rides `DoneWitnessed` with its own carrier
                    // naming exactly the regions it reaches, so no terminal reads `pin`.
                    let step = self.apply_outcome(outcome, idx);
                    let post = self.exit_slot_step(guard);
                    self.ambient.active_in_contract_chain = false;
                    // Drain re-entrant writes against the step scope — the same `scope` opened at the
                    // brand (`combined` pins it through the whole closure; the slot's scope is unchanged
                    // by the step).
                    scope.drain_pending();
                    // The producer's per-call frame, gated to a dying producer: the frame the witnessed
                    // seal folds into a value terminal's witness (a frameless / run-frame producer folds in
                    // nothing) and the destination pin for a `ForwardReady` relocation.
                    let frame = (!post.prev_frame.non_dying()).then_some(&post.prev_frame);
                    match step {
                        NodeStep::DoneWitnessed(carrier) => {
                            // The sole value terminal — object or type, a construction carrier or a
                            // region-pure `resident` seal — arrives already witnessed, naming its foreign
                            // reach. The return contract opened at the brand alongside the continuation, so
                            // it is already a live `ReturnContract<'b>`; the `frame` gate drops it for a
                            // frameless / run-frame producer (no per-call return obligation). The hook folds
                            // the producing frame into the carrier's witness (the scope-reach seal at close)
                            // and — when a declared return coarsens the value (e.g. `List<Number>` through
                            // `:(LIST OF Any)`) — re-stamps it into the contract's home region. No
                            // `dep_reached` / `pin`: the carrier names its own reach.
                            let live_contract = frame.and(live_contract);
                            let result = self.finalize_terminal(carrier, frame, live_contract);
                            if result.is_err() {
                                scope.clear_placeholders_for_producer(id);
                            }
                            self.sched.finalize(idx, result);
                        }
                        NodeStep::Error(error) => {
                            // An error carries no value, so no witness — it finalizes bare. The frame-gated
                            // contract still labels it with the callee's trace frame (a frameless / run
                            // producer carries no contract, so it passes through unlabelled).
                            let live_contract = frame.and(live_contract);
                            let error = finalize_error(error, frame, live_contract);
                            scope.clear_placeholders_for_producer(id);
                            self.sched.finalize(idx, Err(error));
                        }
                        NodeStep::ForwardReady(producer) => {
                            // Relocate `producer`'s terminal into this slot's region via the merge-form
                            // transfer — re-sealed under the producer's own reached sources ∪ this slot's
                            // frame (the `dest` carrier's witness pinning its backing); no contract
                            // re-check (the producer enforced its own). A ready-but-errored producer
                            // relocates to an `Err`, clearing this slot's placeholders as the `Done`
                            // error path does. Framed: the dest brand is `yoke`d into its owning frame,
                            // witnessed by it. Frameless: the dest region is externally pinned for the
                            // step, so a confined empty-set `resident` carries it.
                            let dest = match frame {
                                Some(f) => dest_brand(f.storage_rc()),
                                None => {
                                    Witnessed::<RegionRefFamily, FrameSet>::resident(scope.brand())
                                }
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
                            // Keep the **first** contract of a tail chain: once a contract is set, a nested
                            // tail call does not overwrite it, so the chain checks the original caller's
                            // declared return — not the tail-most callee's. Both contracts are already
                            // erased (the new one by `apply_outcome`), so this is a plain keep-first with no
                            // narrowing here. `prev_contract` is `Copy`, so opening it above into
                            // `live_contract` left the erased original intact for this keep-first.
                            let next_contract: Option<ErasedContract> =
                                prev_contract.or(new_contract);
                            // The frame the body runs in: a freshly installed cart, else the slot's current
                            // one (a `FramePlacement::Inherit` FN-body re-enters the cart a prior `Continue`
                            // already installed — the folded `invoke`). The `ChainOp` reads it (for an
                            // `AssembleBody`) to walk the body scope's lexical chain.
                            let body_frame: &crate::machine::core::CallFrame =
                                new_frame.as_deref().unwrap_or(&prev_frame);
                            let new_chain = chain.apply(prev_chain_carrier, body_frame);
                            match new_frame {
                                Some(f) => {
                                    // A framed tail re-projects `Yoked` from its own cart, so it never
                                    // carries an overlay scope — that is the frameless (`Inherit`) path.
                                    debug_assert!(
                                        overlay_scope.is_none(),
                                        "a framed tail-replace carries no overlay scope"
                                    );
                                    // Rotate the ping-pong reserve: the post-step reserve is superseded by
                                    // today's post-step frame (which we park as the new reserve).
                                    drop(post_step_reserve);
                                    // The non-dying run frame is not a reusable per-call region; parking it
                                    // as the ping-pong reserve would defer (and mis-time) a real frame's
                                    // drop. Treat it as no reserve — the run scope is re-reached through the
                                    // scheduler's `run_frame`, never a reset reserve.
                                    let new_reserve =
                                        (!prev_frame.non_dying()).then_some(prev_frame);
                                    // The tail-replace slot's scope is always this `f` cart's own child, so
                                    // store a payload-less `NodeScope::Yoked` re-projected from the
                                    // co-located cart at the read boundary — no persisted `&'run` to dangle
                                    // across a TCO reset. This Yoked payload is the Koan workload's, built
                                    // here off the scheduler.
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
                                    // A frameless Replace keeps the prior cart — an invoke reuses the
                                    // reserve, never the active cart, so the slot's cart is always present.
                                    // A tail entering an overlay without a fresh frame (USING) installs
                                    // the overlay as the slot's scope — a `YokedChild` opened at read
                                    // against the inherited cart, whose `outer` chain pins the overlay's
                                    // cart-ancestor region — otherwise the slot keeps its existing scope.
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
                            // `producer` and alias it for reads. The slot is not re-queued; `producer`'s
                            // fire wakes the moved consumers, and late parkers resolve the alias when they
                            // wire in. See `scheduler::splice`.
                            self.sched.splice_forward(id, producer);
                        }
                    }
                },
            );
    }
}
