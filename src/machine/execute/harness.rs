//! The apply harness — koan's half of the drain protocol.
//!
//! [`KoanRuntime`] owns the [`Scheduler`] and the koan-side [`Host`] as two sibling fields, so the
//! drain can borrow the scheduler mutably while the host stays reachable: the whole run is
//! `sched.drain(|sched, step| host.step(sched, step))` ([`KoanRuntime::execute`]). [`Host::step`]
//! is the drain callback — it opens the sealed continuation at the step brand, re-brands the
//! pre-read dep terminals against the step's coverage, brackets the ambient frame, runs the decide
//! to an [`Outcome`], drains the step's `WriteOp` effects sink, and hands the outcome to
//! [`Host::apply`] — the **only** `&mut Scheduler` code in koan — which maps it onto the
//! [`StepVerdict`] the drain applies. Dep wiring has one door, [`Host::wire_deps`]; slot
//! retirement is the [`Workload::retiring`] hook.
//!
//! The dispatch-submission wrappers ([`Host::enter_block`], [`Host::dispatch_in_own_scope`],
//! [`Host::dispatch_body`], the literal scheduling in [`super::decide`]) also live on the host —
//! they are the apply side's other writers, all reached from within a step or from
//! [`KoanRuntime::run_program`].
//!
//! See design/execution/README.md and design/memory-model.md.

use std::cell::RefCell;
use std::rc::Rc;

use crate::machine::ProducerId;
use crate::machine::core::KoanStorageProfile;
use crate::machine::core::bindings::{WriteGate, WriteOp};
use crate::machine::core::scope_frame;
use crate::machine::core::{BlockEntry, DepPlacement, FramePlacement, ScopeId};
use crate::machine::core::{ProgramBrand, RegionBrand, ScopeRefFamily};
use crate::machine::model::CarriedFamily;
use crate::machine::model::{
    ExpressionPart, KExpression, Part, PartClass, WorkingExpression, WorkingPart,
};
use crate::machine::{
    CallFrame, DeliveredCarried, FrameCoverage, Installer, KError, KErrorKind, LexicalFrame,
    NodeId, Scope,
};
use crate::scheduler::{
    Anchor, Dep, Deps, DrainDeadlock, EdgeId, InstalledEdge, Scheduler, Step, StepVerdict, Workload,
};
use crate::scheduler::{DeliveryDestination, SealedTerminal};
use crate::witnessed::{SealedExtern, Within, erase_to_static};

use super::ambient::AmbientContext;
use super::decide::{BodyPlacement, DecideCtx, DepRequest, SubmitContext, with_node_scope};
use super::finalize::{NodeFinalize, finalize_error};
use super::lift::relocate_seam;
use super::nodes::{ChainOp, NodePayload, NodeScope, NodeWork, SlotFrame};
use super::obligation::with_obligation;
use super::outcome::{
    Await, Continuation, DepTerminal, Outcome, TerminalDepFinish, dep_error_frame,
};
use super::{
    ContinuationFamily, catch_continuation, ignore_results, seal_witnessed, short_circuit,
};

/// The Koan instantiation of the scheduler's [`Workload`] interface — the marker that binds the
/// opaque scheduler types to their concrete Koan forms. The scheduler is generic over `W: Workload`
/// and names none of these directly; the workload side (this module, `decide/**`) supplies them.
pub(in crate::machine::execute) struct KoanWorkload;

impl Workload for KoanWorkload {
    type Value = CarriedFamily;
    type Error = KError;
    type Profile = KoanStorageProfile;
    type Frame = SlotFrame;
    type Continuation = ContinuationFamily;

    /// Koan's half of delivery: the value-level escape seam ([`relocate_seam`]) — the cost-driven
    /// copy-or-pin verdict with the retention claim derived from the rebuilt product. The walk
    /// decides which destinations a terminal lands in; this decides what the crossing costs.
    fn deliver(
        terminal: &DeliveredCarried,
        dest: DeliveryDestination<KoanWorkload>,
    ) -> DeliveredCarried {
        relocate_seam(terminal, dest)
    }

    /// The slot-retirement hook: the edges this slot still owns — the binder claims its submission
    /// stamped onto its scope, and a bare-name forward's classification edge — released by the
    /// drain exactly once, at the one point the slot stops being able to release them. The clear
    /// first drops any pending binding arm still naming one, which is what keeps a table from ever
    /// holding a [`ProducerId`] whose edge is gone; a successful binder's write path finalizes its
    /// own claim in place, so the clear usually finds nothing. `take` empties the anchor, so the
    /// release is exactly-once by construction.
    fn retiring(anchor: &SlotFrame) -> Vec<EdgeId> {
        let edges = anchor.take_owned_edges();
        if edges.is_empty() {
            return edges;
        }
        let producers: Vec<ProducerId> = edges
            .iter()
            .copied()
            .map(ProducerId::from_scheduler_edge)
            .collect();
        with_node_scope(&anchor.payload.scope, Some(&anchor.cart), |scope| {
            scope.clear_placeholders_for_producers(&producers, &mut WriteGate::for_run_loop());
        });
        edges
    }
}

/// The koan-side state the drain callback runs against: the ambient per-step context, the run's
/// program storage capability, and the run's output sink (held only until the run frame is minted,
/// which is the writer's real home). Split from the scheduler so `drain` can hold `&mut Scheduler`
/// while the step body holds `&mut Host`.
pub(in crate::machine::execute) struct Host<'run> {
    /// The ambient per-step context — the active per-call frame, run frame, the executing slot's
    /// payload, and the declared-return obligation. See [`super::ambient`].
    pub(in crate::machine::execute) ambient: AmbientContext,
    /// This run's program storage capability, handed to every step through its [`DecideCtx`]. It
    /// also carries `'run`: the scheduler is value-erased (`Scheduler<KoanWorkload>`), so without
    /// this the run lifetime would live only in the harness's own method signatures.
    pub(in crate::machine::execute) program: ProgramBrand<'run>,
    /// The run's output sink, held only until [`ensure_run_frame`](Self::ensure_run_frame) mints
    /// the run frame and moves it onto that frame — the writer's real home, beside the run's
    /// [`TypeRegistry`](crate::machine::model::TypeRegistry). It waits here rather than being
    /// passed at the mint because the mint is lazy: a dispatch into the run scope establishes the
    /// frame if nothing has yet, and that call site has no writer to supply.
    out: Option<Box<dyn std::io::Write>>,
}

/// The embedder handle: the scheduler and the host, owned side by side. The embedder API is the
/// [`interpret`](super::interpret) ladder; everything on this type beyond [`Self::new`] and
/// [`Self::run_program`] is the crate-internal drive-and-read surface the test harness uses.
pub struct KoanRuntime<'run> {
    pub(in crate::machine::execute) sched: Scheduler<KoanWorkload>,
    pub(in crate::machine::execute) host: Host<'run>,
}

impl<'run> KoanRuntime<'run> {
    /// `out` is where this run's `PRINT` output goes; it lands on the run frame the first
    /// submission establishes.
    pub fn new(program: ProgramBrand<'run>, out: Box<dyn std::io::Write>) -> Self {
        Self {
            sched: Scheduler::new(),
            host: Host {
                ambient: AmbientContext::default(),
                program,
                out: Some(out),
            },
        }
    }

    /// **The run loop**: drain the scheduler to quiescence, stepping every ready slot through
    /// [`Host::step`]. Slots still parked after the drain are on a dependency that can never fire —
    /// surfaced as [`KErrorKind::SchedulerDeadlock`] rather than a panic on the top-level result
    /// read. Public so a test harness that submits its own work can drive it; production routes
    /// through [`Self::run_program`].
    pub fn execute(&mut self) -> Result<(), KError> {
        let KoanRuntime { sched, host } = self;
        sched.drain(|sched, step| host.step(sched, step)).map_err(
            |DrainDeadlock { pending, sample }| {
                KError::new(KErrorKind::SchedulerDeadlock { pending, sample })
            },
        )
    }

    /// Drive a parsed program to completion: enter each top-level statement as a root via
    /// [`enter_block`](Self::enter_block), wire one root edge apiece, run the scheduler to
    /// quiescence, then release those edges — on the error path too, so no name outlives the run
    /// frame it was destined at.
    pub(in crate::machine::execute) fn run_program(
        &mut self,
        root: &'run Scope<'run>,
        exprs: Vec<KExpression<'run>>,
    ) -> Result<(), KError> {
        // Each top-level statement crosses into the scheduler here — one slice copy of its parts run
        // into the run region, the door every AST node enters dispatch through.
        let statements: Vec<WorkingExpression<'run>> = exprs
            .into_iter()
            .map(|expr| WorkingExpression::from_ast(root.brand(), expr))
            .collect();
        // The run's roots leave submission as edges. Each names the run frame's region as its
        // destination — where a root's producer delivers at finalize — and holding that owner across
        // the install is the wiring-time proof the region is pinned. The submit-time `NodeId`s are
        // transient currency and go out of scope right here.
        let run_owner = root
            .region_owner()
            .upgrade()
            .expect("the run root's region owner is held for the whole run");
        let roots: Vec<EdgeId> = self
            .enter_block(root.id, statements, root)
            .into_iter()
            .map(|id| self.sched.install_edge(id, &run_owner))
            .collect();
        let outcome = self.drive_roots(root, &roots);
        // Koan is the roots' owner, so koan releases them — before the harness (and with it the run
        // frame these edges name) tears down.
        for &edge in &roots {
            self.sched.release_edge(edge);
        }
        outcome
    }

    /// Run to quiescence and rule on the roots' resolution — the fallible middle of
    /// [`run_program`](Self::run_program), split out so every exit from it passes through the
    /// root-edge release.
    fn drive_roots(&mut self, root: &'run Scope<'run>, roots: &[EdgeId]) -> Result<(), KError> {
        self.execute()?;
        // Each root edge was destined at the run frame's region, so its producer delivered there at
        // finalize — under the seam's own copy-or-pin verdict — and the terminal has been an
        // ordinary resident of the run region ever since. A boundary read is a resident read.
        // Seal the run root's reach-set; it is run-global and never reopens.
        root.close();
        // A bare top-level expression is an untyped resolution boundary: an unstamped
        // empty `[]` / `{}` reaching it has no element type to infer, so reject rather
        // than silently resolve to `List<Any>` / `Dict<Any, Any>`.
        for &edge in roots {
            // Copy out the empty-container verdict from inside the open — the carrier never escapes.
            let is_unannotated_empty = match self.sched.read_edge_result_with(edge, |value| {
                value
                    .as_object()
                    .is_some_and(|o| o.is_unstamped_empty_container())
            }) {
                Err(e) => return Err(e.clone()),
                Ok(flag) => flag,
            };
            if is_unannotated_empty {
                return Err(KError::new(KErrorKind::ShapeError(
                    "bare empty container has no element type to infer; annotate its \
                     type (e.g. via a typed FN return) or use a non-empty literal"
                        .to_string(),
                )));
            }
        }
        Ok(())
    }

    /// Establish the run frame on the first run-lifetime submission. See
    /// [`Host::ensure_run_frame`].
    pub(crate) fn ensure_run_frame<'a>(&mut self, scope: &'a Scope<'a>) {
        self.host.ensure_run_frame(scope);
    }

    /// The run frame's subtype-verdict registry, cloned out of the ambient context. `None` until
    /// `ensure_run_frame` mints the run frame — i.e. before the first submission.
    pub(crate) fn type_registry(&self) -> Option<Rc<crate::machine::model::types::TypeRegistry>> {
        self.host.ambient.type_registry_cloned()
    }

    /// Submit each `statement` as a fresh lexical block over `scope`. The program / test-harness
    /// entry point for top-level statements; see [`Host::enter_block`].
    pub(crate) fn enter_block<'a>(
        &mut self,
        scope_id: ScopeId,
        statements: Vec<WorkingExpression<'a>>,
        scope: &'a Scope<'a>,
    ) -> Vec<NodeId> {
        self.host
            .enter_block(&mut self.sched, scope_id, statements, scope)
    }

    /// Submit an unresolved expression for the scheduler to dispatch + execute against `scope`,
    /// inheriting the ambient lexical chain — or, with no step installed, placed at `index`: the
    /// caller-declared statement position, exactly as if the expression were the `index`-th line
    /// of a file. Only the submission's driver (a REPL session, the test harness's cursor) knows
    /// the position, so it is a parameter. The statement-at-a-time submission door; production
    /// submission is [`Self::run_program`]'s `enter_block`.
    pub(crate) fn dispatch_in_scope<'a>(
        &mut self,
        expr: WorkingExpression<'a>,
        scope: &'a Scope<'a>,
        index: usize,
    ) -> NodeId {
        let chain = self.host.statement_chain(scope, index);
        self.host
            .dispatch_in_scope_with_chain(&mut self.sched, expr, scope, chain)
    }

    /// An edge's delivered terminal's error, or `Ok(())` on success — the value-free probe. Public
    /// for the integration suite, which reads its watched statements' dispositions through it.
    pub fn edge_result_error(&self, edge: EdgeId) -> Result<(), &KError> {
        self.sched.edge_result_error(edge)
    }

    /// Open an edge's delivered terminal at a rank-2 brand and hand the value to `f`, returning its
    /// result or the terminal's error — the destination-verb read. See
    /// [`Scheduler::read_edge_result_with`]. In-crate tests read values through this; production
    /// reads go through the scheduler directly.
    #[cfg(test)]
    pub(crate) fn read_edge_result_with<R>(
        &self,
        edge: EdgeId,
        f: impl for<'b> FnOnce(crate::machine::model::Carried<'b>) -> R,
    ) -> Result<R, &KError> {
        self.sched.read_edge_result_with(edge, f)
    }

    /// Wire an edge onto `producer`, destined at `scope`'s own region — the test harness's
    /// stand-in for the root edge `run_program` installs, and for the placeholder edge a real
    /// binder plan claims. Slots reclaim at finalize, so a test that reads a result holds an edge
    /// exactly as production does; wiring it here, right after the dispatch that allocated the
    /// slot, is the same pre-terminal wiring both production callers do. Crate-internal: the
    /// unconditional test-support harness (compiled for the integration suite) reaches it, so it
    /// cannot ride `#[cfg(test)]`.
    pub(crate) fn install_edge_for_test(
        &mut self,
        producer: NodeId,
        scope: &crate::machine::Scope<'_>,
    ) -> EdgeId {
        let destination = scope
            .region_owner()
            .upgrade()
            .expect("a live scope reference implies a live region owner");
        self.sched.install_edge(producer, &destination)
    }

    /// Drop the scheduler's slot store and start a fresh one, keeping the ambient run frame — and
    /// with it the run's [`TypeRegistry`](crate::machine::model::TypeRegistry) and every binding
    /// already installed on the run root. Call at quiescence.
    ///
    /// This is the teardown a test needs between phases when it measures something the drained
    /// slots hold onto: the scheduler's slot store is a free-list whose length is a high-water
    /// mark, and a finished slot's terminal retains its producer frame. Both are program-lifetime
    /// facts about the scheduler, not the run, so a test measuring one program's slot footprint or
    /// frame retention releases the prior phase's slots first.
    #[cfg(test)]
    pub(crate) fn reset_slots(&mut self) {
        self.sched = Scheduler::new();
    }

    /// The scheduler's slot count (a free-list high-water mark) — slot-footprint asserts.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.sched.len()
    }
}

/// Test-only forwarders: an immutable `&Scheduler` view (`resolve_name` fixtures) plus a slot's
/// stored chain. No `&mut Scheduler` escapes — the accessor hands out `&Scheduler`, keeping the
/// harness the sole writer.
#[cfg(test)]
impl<'run> KoanRuntime<'run> {
    pub(in crate::machine::execute) fn scheduler(&self) -> &Scheduler<KoanWorkload> {
        &self.sched
    }

    pub fn chain_of(&self, id: NodeId) -> Option<Rc<LexicalFrame>> {
        self.sched.anchor_of(id).map(|a| a.payload.chain.clone())
    }
}

// ---------- The drain callback ----------

impl<'run> Host<'run> {
    /// The drain callback: one slot's step, start to finish. Opens the sealed continuation beside
    /// the active-scope carrier at one rank-2 `for<'b>` step brand — the seal carries its own
    /// anchor pin, and the step's coverage witnesses the operand — re-brands the pre-read dep
    /// terminals once against that coverage, brackets the ambient frame
    /// ([`Self::with_slot_step`]), runs the continuation to an [`Outcome`], applies the step's
    /// binding writes, and maps the outcome onto the returned [`StepVerdict`] through
    /// [`Self::apply`]. The closure's result cannot name `'b`, so a `Replace` verdict's
    /// continuation exits through [`erase_to_static`] — stored, never used, until the drain seals
    /// it against the slot's effective anchor and the next step re-anchors it at a fresh brand.
    fn step(
        &mut self,
        sched: &mut Scheduler<KoanWorkload>,
        step: Step<KoanWorkload>,
    ) -> StepVerdict<'static, KoanWorkload> {
        let Step {
            id,
            anchor,
            continuation: sealed_continuation,
            dep_results,
        } = step;
        // Source the step's context off the scheduler-held anchor: the cart, the slot's scope
        // handle, and its lexical chain. Read as values up front so nothing holds a scope borrow
        // across the step's work or a tail hop's frame swap.
        let cart = Rc::clone(&anchor.cart);
        let node_scope = anchor.payload.scope;
        let chain = anchor.payload.chain.clone();
        // The step's open witness — the **step's coverage**: the anchor's projected region owner,
        // which pins the continuation and the dest region plus their ancestor backings via the
        // storage `outer` chain. That is the whole of it, and it already covers every dep: an owned
        // dep's edge is destined at this slot's own anchor region, and a park's inherits the region
        // its source named — a scope region this anchor's `outer` chain pins. Assembled before the
        // open so it outlives `'b`, and held across it, so re-anchoring the carriers to `'b`
        // cannot dangle.
        let combined: FrameCoverage = FrameCoverage::of(Rc::clone(anchor.owner()));
        // Re-brand each delivered resident **once**, here, against the step's coverage: a retained
        // cell proves no liveness of its own, and `combined` is exactly the pin covering every
        // region a dep landed in. From this point the step's readers open pin-free — a dep value
        // rides no shared step brand and needs no envelope of its own.
        let dep_sources: Vec<Result<DepTerminal<'_>, KError>> = dep_results
            .iter()
            .map(
                |resident: &Result<SealedTerminal<KoanWorkload>, KError>| match resident {
                    Ok(cell) => Ok(DepTerminal {
                        cell: cell.brand_with(&combined),
                    }),
                    Err(error) => Err(error.clone()),
                },
            )
            .collect();
        // The active scope as a carrier, per node-scope shape: `Yoked` takes the cart's own
        // child-scope carrier; `YokedChild` reuses the carrier it already holds. `combined` pins
        // both.
        let scope_carrier = match node_scope {
            NodeScope::Yoked => cart.scope_sealed(),
            NodeScope::YokedChild(carrier) => carrier,
        };
        // Open the owned-tier continuation beside the active-scope operand at one rank-2 `for<'b>`
        // brand. The `Within` token's declared `'run: 'b` is what lets `self.program` — a live
        // borrow-checked `ProgramBrand<'run>`, not a sealed carrier — be stored in the view at its
        // own `'program = 'run`, discharging the `DecideCtx`'s `'program: 'step` bound without
        // shortening the brand.
        sealed_continuation.open(
            scope_carrier,
            &combined,
            |_within: Within<'_, 'run>,
             continuation: super::outcome::NodeContinuation<'_>,
             scope| {
                // The step's binding-write sink: every `Action` the step interprets deposits its
                // `WriteOp`s here through `run_action`, and the drain below applies them against
                // the step scope. Declared inside the step brand because a `WriteOp` carries seals
                // branded to `scope`'s region — so "nothing crosses steps" is the borrow checker's
                // rule here, not a convention.
                let step_effects: RefCell<Vec<WriteOp<'_>>> = RefCell::new(Vec::new());
                // Bracket the step's ambient frame/payload — restored on every exit path,
                // including unwinds, by `with_slot_step` itself. The whole tail — decide, effects,
                // apply — runs inside the bracket, so the apply reads the step-end frame and the
                // deposited obligation off the ambient context directly.
                self.with_slot_step(
                    Rc::clone(&cart),
                    NodePayload {
                        scope: node_scope,
                        chain: chain.clone(),
                    },
                    |host| {
                        let outcome = continuation(
                            &DecideCtx::new(
                                &host.ambient,
                                scope,
                                scope_frame(scope),
                                Installer::Statement(anchor.statement()),
                                &step_effects,
                                host.program,
                            ),
                            &dep_sources,
                            id,
                        );
                        // Apply the step's binding writes against the step scope, in the order the
                        // bodies decided them. This is the **only** path that mutates a published
                        // binding table: it runs after the continuation returned — so no koan frame
                        // holds a competing borrow — and before the outcome is realized, so the
                        // writes land while the scope is still open and before any graph edge an
                        // errored step would strand is installed. On the first failure the
                        // remaining ops are dropped and the step becomes the node's error terminal,
                        // so the drain's finalize drops the producer's pending arms and attributes
                        // the error exactly as for an in-step error. A body that errors before
                        // deciding its write installs nothing at all: the writes are outcome data,
                        // and an error terminal carries none.
                        let mut gate = WriteGate::for_run_loop();
                        let outcome = match step_effects
                            .borrow_mut()
                            .drain(..)
                            .try_for_each(|op| op.apply(scope, &mut gate))
                        {
                            Ok(()) => outcome,
                            Err(error) => Outcome::Done(Err(error)),
                        };
                        host.apply(sched, outcome, scope.brand(), id, &anchor)
                    },
                )
            },
        )
    }

    /// **The apply**: turn a decided [`Outcome`] into the scheduler writes it implies and the
    /// [`StepVerdict`] the drain applies — the sole graph-writing tail a step reaches. Runs inside
    /// the step's ambient bracket, so the step-end frame and the deposited obligation are ambient
    /// reads.
    fn apply<'step>(
        &mut self,
        sched: &mut Scheduler<KoanWorkload>,
        outcome: Outcome<'step>,
        brand: RegionBrand<'step>,
        id: NodeId,
        anchor: &Rc<SlotFrame>,
    ) -> StepVerdict<'static, KoanWorkload> {
        match outcome {
            Outcome::Done(result) => {
                anchor.close_opened_scope();
                // The producer's per-call frame gates the return obligation (the contract label and
                // the finalize fold): a frameless / run-frame producer folds in nothing. Retention
                // seeds independently — the scheduler reads the slot's own anchor owner at
                // finalize — so the gate makes no memory decision.
                let step_frame = self
                    .ambient
                    .active_frame_ref()
                    .cloned()
                    .expect("a step always runs against a cart");
                let obligation = (!step_frame.non_dying())
                    .then(|| self.ambient.current_obligation_duplicate())
                    .flatten();
                match result {
                    Ok(carrier) => {
                        // Seal the value terminal into a delivery envelope pinned by the anchor's
                        // own region owner — the same owner the scheduler seeds as the slot's
                        // retention host. [`StepCarried::seal_at_step`] is the sole exit from the
                        // step brand: it discharges `'step` into the lifetime-free envelope.
                        // `finalize_terminal` hands the envelope on whole; the delivery walk reads
                        // its coverage to adopt into each destination, so no consumer re-derives
                        // the reach and no call site here names one.
                        let envelope = carrier.seal_at_step(Rc::clone(anchor.owner()));
                        StepVerdict::Done(self.finalize_terminal(
                            envelope,
                            anchor.owner(),
                            obligation.as_ref(),
                        ))
                    }
                    // An error finalizes bare (no value, no witness); the frame-gated obligation
                    // still labels it with the callee's trace frame.
                    Err(error) => {
                        StepVerdict::Done(Err(finalize_error(error, obligation.as_ref())))
                    }
                }
            }
            Outcome::Continue {
                work,
                frame,
                chain,
                block_entry,
            } => {
                // A tail iteration (`FreshTail`) retires this scope before the fresh cart is
                // installed for the next; other placements keep the current scope live.
                if matches!(frame, FramePlacement::FreshTail { .. }) {
                    anchor.close_opened_scope();
                }
                let new_frame = frame.fresh_frame();
                // An `Overlay` block entry rides the tail slot's scope: erased to a cart-witnessed
                // carrier here (where the overlay is still live) so the frameless replace installs
                // it as the slot's `YokedChild` — the frameless analogue of the `Yoked` a framed
                // tail re-projects from its own cart.
                let overlay_scope = match block_entry {
                    BlockEntry::Overlay(overlay) => {
                        Some(SealedExtern::<ScopeRefFamily>::erase(overlay))
                    }
                    BlockEntry::None | BlockEntry::FrameScope(_) => None,
                };
                // The frame the body runs in: the freshly minted cart, else the slot's current one
                // (an `Inherit` FN-body re-enters the cart a prior `Continue` installed). The
                // `ChainOp` reads it to assemble the body's lexical chain. Read off the ambient —
                // the slot's cart at step end.
                let step_frame = self
                    .ambient
                    .active_frame_ref()
                    .cloned()
                    .expect("a step always runs against a cart");
                let body_frame: &CallFrame = new_frame.as_deref().unwrap_or(&step_frame);
                // Read the chain-reshape variant before `apply` consumes it: a frameless re-entry
                // mints a fresh anchor iff the chain (or the overlay scope) changed — an `Inherit`
                // FN-body re-entry is frameless yet reshapes the chain (`AssembleBody`), so the
                // gate keys on the variant, not on `new_frame.is_some()`.
                let chain_changed = !matches!(chain, ChainOp::Unchanged);
                let new_chain = chain.apply(anchor.payload.chain.clone(), body_frame);
                let next_anchor = match new_frame {
                    Some(f) => {
                        // A framed tail re-projects `Yoked` from its own cart; the overlay scope is
                        // the frameless (`Inherit`) path.
                        debug_assert!(
                            overlay_scope.is_none(),
                            "a framed tail-replace carries no overlay scope"
                        );
                        // The claims and the statement identity belong to the slot, not to the
                        // anchor it happens to be wearing, so the incoming anchor takes them over
                        // from the retiring one. `opening` rather than `replacing` because `f` is a
                        // cart minted for this slot: the slot opens its scope here and closes it at
                        // its own finish. The retiring cart's region has nothing left the next
                        // incarnation reads — the decide that emitted this replace relocated the
                        // callee's arguments into `f`'s region — so the drain drops the displaced
                        // anchor at the reinstall.
                        Some(SlotFrame::opening(f, NodeScope::Yoked, new_chain, anchor))
                    }
                    None => {
                        // A frameless replace keeps the prior cart. A tail entering an overlay
                        // without a fresh frame (USING) installs the overlay as the slot's scope —
                        // a `YokedChild` whose `outer` chain pins the overlay's cart-ancestor
                        // region — otherwise the slot keeps its scope. Mint a fresh anchor only
                        // when the overlay scope or the chain changed; a pure park-free re-decide
                        // (same cart, scope, and chain) keeps the anchor.
                        let has_overlay = overlay_scope.is_some();
                        let scope =
                            overlay_scope.map_or(anchor.payload.scope, NodeScope::YokedChild);
                        (has_overlay || chain_changed).then(|| {
                            SlotFrame::replacing(Rc::clone(&step_frame), scope, new_chain, anchor)
                        })
                    }
                };
                replace_verdict(work, next_anchor)
            }
            Outcome::Park {
                deps,
                continuation,
                dep_error_frame: park_error_frame,
            } => {
                // Wire the whole dep list through the one door; each dep's filled-or-parked verdict
                // comes back index-aligned.
                let installed = self.wire_deps(sched, anchor, id, deps, |host, sched, request| {
                    host.realize_park_request(sched, brand, request)
                });
                // **Install-and-inspect**: a decide never probes a producer's standing, so a park
                // whose producer had already finalized is classified here. An errored one is
                // propagated now rather than waited on — a terminal slot never notifies again, so
                // the park would never wake. The first error wins, matching the step-start pull's
                // short-circuit. Rows and holds already installed discharge through this slot's
                // ordinary death path.
                for verdict in &installed {
                    let InstalledEdge::Filled(edge) = verdict else {
                        continue;
                    };
                    if let Err(dep_error) = sched.edge_result_error(*edge) {
                        let error =
                            super::decide::propagate_dep_error(dep_error, park_error_frame.clone());
                        return self.apply(sched, Outcome::Done(Err(error)), brand, id, anchor);
                    }
                }
                // Lower each variant to its outermost live continuation alongside its
                // deadlock-summary carrier.
                let (continuation, carrier) = match continuation {
                    // A dispatch finish carries its own dep-error frame (the consuming call's, or
                    // `None` frameless); an action/literal dep-finish carries the
                    // `dep_error_frame()` label. The short-circuit is baked into the continuation
                    // by `short_circuit` — the one loop the terminal delivery runs through.
                    Continuation::Finish(finish) => (short_circuit(park_error_frame, finish), None),
                    // The action-harness catch carries its single watched dep unrealized (its
                    // placement differs from a dep-finish body's fan-out: an `InScope` watched
                    // enters a fresh single-statement block, never splitting). Realized here and
                    // wired through the same door as every other dep. `catch_continuation` runs the
                    // finish without short-circuiting on a dep error.
                    Continuation::Catch { watched, finish } => {
                        let _watched_verdict = self.wire_deps(
                            sched,
                            anchor,
                            id,
                            Deps::from_requests([watched]),
                            |host, sched, watched| vec![host.realize_catch_watched(sched, watched)],
                        );
                        (catch_continuation(finish), None)
                    }
                    // The resume closure carries the evolving `working_expr` from here on; the
                    // `carrier` it travels with is only the deadlock-summary sample. A decide takes
                    // no dep values, so `ignore_results` drops the results slice.
                    Continuation::Resume { carrier, resume } => (ignore_results(resume), carrier),
                };
                // Carry the ambient obligation across the park: the resumed step re-deposits it so
                // the chain's declared-return check still fires. The wrap sits on the outermost
                // closure, so every variant — including the dep-error short-circuit inside
                // `short_circuit` — runs under it and its error arm still gets the trace label.
                let continuation =
                    with_obligation(self.ambient.current_obligation_duplicate(), continuation);
                // The degenerate replace: same cart, scope, and chain, so no anchor swaps in.
                replace_verdict(NodeWork::new(continuation, carrier), None)
            }
            Outcome::Forward(source) => {
                // The slot's result *is* the result behind `source`. Classification is the
                // install's, not a probe's: wiring a second edge off `source` answers
                // filled-or-parked and leaves a name this slot can read through. Filled: the
                // producer already delivered into the destination `source` names, and the new edge
                // inherits that destination, so the terminal is resident where this slot reads it
                // and nothing relocates. Parked: the probe edge has said all it can, so `Alias`
                // drives the splice — move consumers onto the producer and alias the slot.
                // A forward is the one shape that wants no destination of its own: the slot is
                // standing in for the producer, so landing the terminal where the producer's own
                // consumers already look is the correct answer rather than a limitation — no site
                // here asks for a delivery aimed at a region `source` does not already name.
                // The classification edge joins the slot's owned list: retirement releases it when
                // the slot terminalizes (or splices out), which is what lets a checker micro-step
                // re-emit `Forward` on an edge of its own rather than on a foreign claim its binder
                // may have retired in the meantime.
                let installed = sched.install_edge_from(source);
                anchor.own_edges([installed.edge_id()]);
                let Some(obligation) = self.ambient.current_obligation_duplicate() else {
                    return match installed {
                        InstalledEdge::Filled(edge) => StepVerdict::Forward(edge),
                        InstalledEdge::Parked(_) => StepVerdict::Alias(source),
                    };
                };
                // A residual declared-return obligation on this splice must be discharged before
                // the rehomed terminal reaches any consumer. Take it out of the ambient so neither
                // this step's finalize (the obligation is spent here) nor the not-ready
                // micro-step's continuation re-observes it; `obligation` is captured (never
                // re-deposited), so the check runs obligation-free.
                self.ambient.take_obligation();
                match installed {
                    // The producer resolved: run the declared-return check inline against its
                    // terminal, then behave as the obligation-free ready path. An errored producer
                    // carries no value to check — the forward relocates its error as the
                    // obligation-free path would.
                    InstalledEdge::Filled(edge) => {
                        // The producer's value is already resident in the edge's destination; the
                        // check reads it in place, under the region's own owner.
                        let checked = match sched.read_edge_result_with(edge, |value| {
                            super::finalize::check_spliced_return(
                                &obligation,
                                value,
                                self.ambient.type_registry(),
                            )
                        }) {
                            Ok(checked) => checked,
                            // A ready-but-errored producer carries no value to check.
                            Err(_) => Ok(()),
                        };
                        match checked {
                            Ok(()) => StepVerdict::Forward(edge),
                            Err(error) => {
                                self.apply(sched, Outcome::Done(Err(error)), brand, id, anchor)
                            }
                        }
                    }
                    // The producer is not yet resolved: park a checker micro-step on it (an
                    // already-terminal producer never re-notifies, so a park is sound only here).
                    // Its finish runs the declared-return check un-relocated and re-emits `Forward`
                    // on a pass — which re-enters this arm with no ambient obligation (the
                    // micro-step ran obligation-free) and, the producer now resolved, takes the
                    // plain ready path. No re-check, no loop. Both the park and the re-emission
                    // name `edge`, this slot's own name for the producer, which outlives the wait
                    // whatever the binder that first published it does.
                    InstalledEdge::Parked(edge) => {
                        let finish: TerminalDepFinish<'step> = Box::new(move |view, terminals| {
                            // The single parked dep is the producer behind `edge`, delivered
                            // un-relocated at index 0.
                            let producer_terminal = terminals[0];
                            let checked = producer_terminal.cell.open(|value| {
                                super::finalize::check_spliced_return(
                                    &obligation,
                                    value,
                                    view.types(),
                                )
                            });
                            match checked {
                                Ok(()) => Outcome::Forward(edge),
                                Err(error) => Outcome::Done(Err(error)),
                            }
                        });
                        let park = Await::on(Deps::from_producers([edge]))
                            .error_frame(dep_error_frame())
                            .finish_terminal(finish);
                        self.apply(sched, park, brand, id, anchor)
                    }
                }
            }
        }
    }
}

/// Erase a step-branded replacement's continuation into the `Replace` verdict: the continuation is
/// **stored**, never used, until the drain seals it against the slot's effective anchor
/// ([`SealedPinned::erase`](crate::witnessed::SealedPinned)) and the next step re-anchors it at a
/// fresh brand — the same storage discipline every dormant continuation rides, with the erase
/// running at the verdict boundary instead of inside a scheduler door.
fn replace_verdict(
    work: NodeWork<'_, KoanWorkload>,
    anchor: Option<Rc<SlotFrame>>,
) -> StepVerdict<'static, KoanWorkload> {
    StepVerdict::Replace {
        work: NodeWork::new(
            erase_to_static::<ContinuationFamily>(work.continuation),
            work.carrier,
        ),
        anchor,
    }
}

// ---------- Dep wiring: the one door ----------

impl<'run> Host<'run> {
    /// **The dep-wiring door.** Resolve `deps` to one source edge per dep — a dep the caller
    /// already named passes its source through; a request is realized by `realize` into producer
    /// slots, each named by a minted source destined at `anchor`'s region — then mint the
    /// consumer's own edge off each source through [`Scheduler::install_deps`] and release the
    /// minted sources, whose only job was carrying producer and destination into the install.
    /// Returns each dep's filled-or-parked verdict, index-aligned with the realized list.
    fn wire_deps<R>(
        &mut self,
        sched: &mut Scheduler<KoanWorkload>,
        anchor: &SlotFrame,
        consumer: NodeId,
        deps: Deps<R>,
        realize: impl FnMut(&mut Self, &mut Scheduler<KoanWorkload>, R) -> Vec<NodeId>,
    ) -> Vec<InstalledEdge> {
        let (sources, minted) = self.named_sources(sched, anchor, deps, realize);
        let installed = sched.install_deps(consumer, &sources);
        for source in minted {
            sched.release_edge(source);
        }
        installed
    }

    /// [`Self::wire_deps`]'s naming half, shared with the submission path (which hands its sources
    /// to [`Scheduler::alloc_node`] instead of `install_deps`, the slot not existing yet): resolve
    /// a dep list to one **source edge** per dep, in dep order, plus the sources this call minted —
    /// which the caller releases once the door it feeds has minted the consumer's own edges.
    fn named_sources<R>(
        &mut self,
        sched: &mut Scheduler<KoanWorkload>,
        anchor: &SlotFrame,
        deps: Deps<R>,
        mut realize: impl FnMut(&mut Self, &mut Scheduler<KoanWorkload>, R) -> Vec<NodeId>,
    ) -> (Vec<EdgeId>, Vec<EdgeId>) {
        let entries = deps.into_entries();
        let entry_count = entries.len();
        let mut sources: Vec<EdgeId> = Vec::with_capacity(entry_count);
        let mut minted: Vec<EdgeId> = Vec::new();
        for entry in entries {
            match entry {
                Dep::Producer(source) => sources.push(source),
                Dep::Request(request) => {
                    let producers = realize(self, sched, request);
                    debug_assert!(
                        producers.len() == 1 || entry_count == 1,
                        "a request fanning out to several producers shifts every later dep's \
                         position, so it may only be the sole entry in its list",
                    );
                    // Name each spawned producer by a source edge destined at this slot's own
                    // anchor region — where a sub-result belongs. The install door mints the slot's
                    // real dep edge off it and inherits that destination, so sub-work needs no
                    // second wiring rule.
                    for producer in producers {
                        let source = sched.install_edge(producer, anchor.owner());
                        sources.push(source);
                        minted.push(source);
                    }
                }
            }
        }
        (sources, minted)
    }

    /// Realize one park-declared dep request into its producer slots. An `InScope` dispatch and a
    /// `BodyBlock` fan out one producer per statement; everything else is a single producer.
    fn realize_park_request<'a>(
        &mut self,
        sched: &mut Scheduler<KoanWorkload>,
        brand: RegionBrand<'a>,
        request: DepRequest<'a>,
    ) -> Vec<NodeId> {
        match request {
            // An `InScope` body fans out one producer per statement (multi-statement split);
            // `OwnScope` realizes as a single producer via the shared [`Self::realize_dispatch`].
            DepRequest::Dispatch {
                expr,
                placement: DepPlacement::InScope(scope),
            } => {
                let statements = split_working_body(scope.brand(), expr);
                self.enter_block(sched, scope.id, statements, scope)
            }
            request @ (DepRequest::Dispatch { .. }
            | DepRequest::ListLit(_)
            | DepRequest::DictLit(_)
            | DepRequest::RecordLit(_)) => {
                vec![self.realize_eager_dep(sched, brand, request)]
            }
            // A body block fans out one producer per statement: into a fresh per-call frame's own
            // scope (`dispatch_body`), or — under `Inherit` — into a caller-allocated overlay via
            // the same `enter_block` fan-out the leading statements of an `InScope` body use
            // (USING).
            DepRequest::BodyBlock {
                statements,
                placement: BodyPlacement::Frame(frame),
            } => self.dispatch_body(sched, &frame, statements),
            DepRequest::BodyBlock {
                statements,
                placement: BodyPlacement::Overlay(overlay),
            } => self.enter_block(sched, overlay.id, statements, overlay),
        }
    }

    /// Realize one staged eager dep as its producer node — the four shapes
    /// [`stage_eager_part`](super::decide::stage_eager_part) emits. `brand` is the realizing
    /// step's, where an aggregate literal's per-element dispatch node is bumped. `BodyBlock` never
    /// reaches here (the stager doesn't produce it).
    pub(in crate::machine::execute) fn realize_eager_dep<'a>(
        &mut self,
        sched: &mut Scheduler<KoanWorkload>,
        brand: RegionBrand<'a>,
        dep: DepRequest<'a>,
    ) -> NodeId {
        match dep {
            DepRequest::Dispatch { expr, placement } => {
                self.realize_dispatch(sched, expr, placement)
            }
            DepRequest::ListLit(items) => self.schedule_list_literal(sched, brand, items),
            DepRequest::DictLit(pairs) => self.schedule_dict_literal(sched, brand, pairs),
            DepRequest::RecordLit(fields) => self.schedule_record_literal(sched, brand, fields),
            DepRequest::BodyBlock { .. } => {
                unreachable!("eager staging emits only Dispatch / literal deps")
            }
        }
    }

    /// Realize a single-statement dispatch dep at `placement` to its producer slot. `OwnScope`
    /// re-dispatches against the executing slot's own scope; `InScope` enters a fresh
    /// **single-statement** block (so an inner `LET` stays local). A multi-statement body splits
    /// separately — see [`Self::realize_park_request`].
    fn realize_dispatch<'a>(
        &mut self,
        sched: &mut Scheduler<KoanWorkload>,
        expr: WorkingExpression<'a>,
        placement: DepPlacement<'a>,
    ) -> NodeId {
        match placement {
            DepPlacement::OwnScope => {
                self.dispatch_in_own_scope(sched, expr, SubmitContext::SubDispatch)
            }
            DepPlacement::InScope(scope) => self
                .enter_block(sched, scope.id, vec![expr], scope)
                .into_iter()
                .next()
                .expect("enter_block of one statement yields one node"),
        }
    }

    /// Realize a [`Continuation::Catch`]'s single watched [`DepRequest`] to a producer `NodeId`: a
    /// `Dispatch` realizes as a single statement (an `InScope` watched expr enters a fresh
    /// single-statement block — see [`Self::realize_dispatch`]). A `Catch` never watches a
    /// dispatcher-only lowering.
    fn realize_catch_watched<'a>(
        &mut self,
        sched: &mut Scheduler<KoanWorkload>,
        dep: DepRequest<'a>,
    ) -> NodeId {
        match dep {
            DepRequest::Dispatch { expr, placement } => {
                self.realize_dispatch(sched, expr, placement)
            }
            DepRequest::ListLit(_)
            | DepRequest::DictLit(_)
            | DepRequest::RecordLit(_)
            | DepRequest::BodyBlock { .. } => {
                unreachable!("a Catch watches only a simple Dispatch dep")
            }
        }
    }
}

/// Split a body block into the statements it fans out to, one working node apiece. The
/// scheduler-side peer of [`split_body_statements`](crate::machine::split_body_statements): a block
/// (two or more parts, every one an expression) yields its children, and any other body is the one
/// statement it already is. A parsed child crosses into the scheduler here, at `brand`; a child the
/// scheduler synthesized is already working form.
fn split_working_body<'a>(
    brand: RegionBrand<'a>,
    body: WorkingExpression<'a>,
) -> Vec<WorkingExpression<'a>> {
    let is_block = body.parts.len() >= 2
        && body
            .parts
            .iter()
            .all(|part| matches!(part.value.class(), PartClass::Expression));
    if !is_block {
        return vec![body];
    }
    body.parts
        .iter()
        .filter_map(|part| match part.value {
            WorkingPart::Ast(ExpressionPart::Expression(child)) => {
                Some(WorkingExpression::from_ast(brand, *child))
            }
            WorkingPart::Expression(child) => Some(*child),
            _ => None,
        })
        .collect()
}

// ---------- Submission ----------

/// Pointer equality of two scopes (identity, not structural).
fn scopes_eq(a: &Scope<'_>, b: &Scope<'_>) -> bool {
    std::ptr::eq(
        a as *const Scope<'_> as *const (),
        b as *const Scope<'_> as *const (),
    )
}

impl<'run> Host<'run> {
    /// The explicit chain when no slot step is installed: a fresh single-frame chain placing the
    /// submission at `index` — the caller-declared statement position, numbered exactly as if the
    /// statement were the `index`-th line of a file, so the visibility cutoff hides its own claims
    /// and everything later while showing every earlier binding. Only the submission's driver (a
    /// REPL session, the test harness's cursor) knows the position, so it is a parameter, never
    /// stored or derived. With a slot step installed this is `None`, inheriting the ambient
    /// payload's chain.
    pub(in crate::machine::execute) fn statement_chain(
        &self,
        scope: &Scope<'_>,
        index: usize,
    ) -> Option<Rc<LexicalFrame>> {
        self.ambient
            .active_payload()
            .is_none()
            .then(|| LexicalFrame::root(scope.id, index))
    }

    /// Establish the run frame on the first run-lifetime submission, so every top-level slot carries
    /// a frame cart and the active frame is never `None` during a top-level step. Idempotent; the
    /// scheduler owns the minted frame's lifecycle.
    pub(in crate::machine::execute) fn ensure_run_frame<'a>(&mut self, scope: &'a Scope<'a>) {
        if !self.ambient.has_run_frame() {
            // The writer moves onto the frame it belongs to. A lazily-established run frame — a
            // dispatch that reached here before any entry point mint — gets the sink, which is
            // what a run with no caller-supplied writer already meant.
            let out = self.out.take().unwrap_or_else(|| Box::new(std::io::sink()));
            self.ambient.set_run_frame(CallFrame::adopting(scope, out));
        }
    }

    /// Decide a run-scope submission's [`NodeScope`] handle — always cart-witnessed, never anchored
    /// at a free `'run`. Cases, in order:
    ///
    /// - The active cart's *own* scope is `scope` → [`NodeScope::Yoked`] (re-projected from the cart).
    /// - The active cart pins `scope`'s region ([`CallFrame::pins_scope_region`]) →
    ///   [`NodeScope::YokedChild`]: `scope` is a block scope a builtin allocated in a cart
    ///   *ancestor* region, held by the cart's `FrameStorage.outer` chain. Stored erased,
    ///   reattached frame-bounded.
    /// - No active frame and the `run_frame` (which adopts the run root) *is* `scope` → `Yoked`.
    /// - No active frame and the run frame pins `scope`'s region → `YokedChild`, the frameless peer
    ///   of the second case: `scope` is a child allocated in the run region, so the run frame pins
    ///   it just as a cart pins an ancestor-region block scope.
    pub(in crate::machine::execute) fn resolve_node_scope<'a>(
        &self,
        scope: &'a Scope<'a>,
    ) -> NodeScope {
        if let Some(f) = self.ambient.active_frame_ref() {
            if f.with_scope(|fs| scopes_eq(fs, scope)) {
                return NodeScope::Yoked;
            }
            if f.pins_scope_region(scope) {
                return NodeScope::YokedChild(SealedExtern::<ScopeRefFamily>::erase(scope));
            }
            unreachable!("a framed submission's scope is the cart's own or a cart-ancestor child");
        }
        if let Some(rf) = self.ambient.run_frame_ref() {
            if rf.with_scope(|rs| scopes_eq(rs, scope)) {
                return NodeScope::Yoked;
            }
            if rf.pins_scope_region(scope) {
                return NodeScope::YokedChild(SealedExtern::<ScopeRefFamily>::erase(scope));
            }
        }
        unreachable!("a frameless submission targets the run root or a child in its region");
    }

    /// Submit each `statement` as a fresh lexical block over `scope`, minting a frame `(scope_id,
    /// i+1)` per statement — the block fan-out behind top-level programs, `InScope` bodies, and
    /// overlay blocks.
    pub(in crate::machine::execute) fn enter_block<'a>(
        &mut self,
        sched: &mut Scheduler<KoanWorkload>,
        scope_id: ScopeId,
        statements: Vec<WorkingExpression<'a>>,
        scope: &'a Scope<'a>,
    ) -> Vec<NodeId> {
        let parent = self.ambient.active_payload().map(|p| p.chain.clone());
        // Indices start at 1: visibility is strict less-than and builtins sit at idx 0,
        // so a top-level statement at index 1 sees them via `0 < 1`.
        statements
            .into_iter()
            .enumerate()
            .map(|(i, expr)| {
                let chain = LexicalFrame::push(parent.clone(), scope_id, i + 1);
                self.dispatch_in_scope_with_chain(sched, expr, scope, Some(chain))
            })
            .collect()
    }

    /// Submit `expr` against a run-lived `scope`: establish the run frame, decide the slot's
    /// [`NodeScope`] handle, then submit with the caller's resolved lexical `chain`.
    pub(in crate::machine::execute) fn dispatch_in_scope_with_chain<'a>(
        &mut self,
        sched: &mut Scheduler<KoanWorkload>,
        expr: WorkingExpression<'a>,
        scope: &'a Scope<'a>,
        chain: Option<Rc<LexicalFrame>>,
    ) -> NodeId {
        self.ensure_run_frame(scope);
        let node_scope = self.resolve_node_scope(scope);
        // Every caller (top-level `enter_block`, the test harness's `dispatch_in_scope`, an
        // `InScope` dep's fresh block) is a statement position, so a binder installs its plan here.
        self.submit_expression(
            sched,
            expr,
            scope,
            node_scope,
            chain,
            SubmitContext::Statement,
        )
    }

    /// Dispatch `expr` as a `Yoked` sub-slot of the currently-active per-call frame. The caller must
    /// have installed the per-call frame as the ambient active frame (the step bracket does this per
    /// step; [`Self::dispatch_body`] does it transiently).
    fn dispatch_in_active_frame<'a>(
        &mut self,
        sched: &mut Scheduler<KoanWorkload>,
        expr: WorkingExpression<'a>,
        chain: Option<Rc<LexicalFrame>>,
        ctx: SubmitContext,
    ) -> NodeId {
        let frame = self
            .ambient
            .active_frame_ref()
            .cloned()
            .expect("in-frame dispatch requires an active frame");
        // Re-project the scope from the frame cart at a `for<'b>` brand confined to the
        // `submit_expression` call, so no borrow rides up the `&mut self` path.
        frame.with_scope(|scope| {
            self.submit_expression(sched, expr, scope, NodeScope::Yoked, chain, ctx)
        })
    }

    /// Dispatch `expr` against the executing slot's own scope handle (the `OwnScope` dep placement).
    /// A `YokedChild` slot reuses its erased cart-ancestor pointer; a `Yoked` slot re-projects via
    /// [`Self::dispatch_in_active_frame`]. Both route through [`Self::submit_expression`] as a
    /// [`SubmitContext::SubDispatch`], so a binder staged into an eager slot is rejected.
    pub(in crate::machine::execute) fn dispatch_in_own_scope<'a>(
        &mut self,
        sched: &mut Scheduler<KoanWorkload>,
        expr: WorkingExpression<'a>,
        ctx: SubmitContext,
    ) -> NodeId {
        let node_scope = self
            .ambient
            .active_payload()
            .expect("a slot step installs the ambient payload before the body submits")
            .scope;
        // The expect above proves a step is active, so the chain is always the ambient one.
        let chain = None;
        match node_scope {
            NodeScope::YokedChild(_) => {
                // Hold the cart `Rc` in a local so the reattach is witnessed by an owned handle: it
                // keeps the cart's `FrameStorage.outer` chain alive while `with_node_scope` opens the
                // `YokedChild` pointer at a `for<'b>` brand, so no borrow escapes the call.
                let cart = self.ambient.active_frame_ref().cloned();
                with_node_scope(&node_scope, cart.as_ref(), |scope| {
                    self.submit_expression(sched, expr, scope, node_scope, chain, ctx)
                })
            }
            NodeScope::Yoked => self.dispatch_in_active_frame(sched, expr, chain, ctx),
        }
    }

    /// Dispatch a body's non-tail `statements` as sibling sub-slots in `frame`, each at body-chain
    /// index `i + 1` (params / `it` sit at idx 0) over the frame's body scope, with the parent chain
    /// reconstructed from the call site via
    /// [`assemble_body_chain`](crate::machine::core::assemble_body_chain). The shared "execute a
    /// block of expressions" primitive (FN body, deferred return-type dep, MATCH/TRY arm body); the
    /// caller tail-replaces into the last statement separately.
    fn dispatch_body<'a>(
        &mut self,
        sched: &mut Scheduler<KoanWorkload>,
        frame: &Rc<CallFrame>,
        statements: Vec<WorkingExpression<'a>>,
    ) -> Vec<NodeId> {
        let call_site_chain = self
            .ambient
            .active_payload()
            .map(|p| p.chain.clone())
            .expect("a body block runs inside an active lexical chain");
        // Open the body scope at a `for<'b>` brand: the id copies out and the chain returns as an
        // unbranded `Rc`, so nothing branded escapes the read.
        let (body_scope_id, parent) = frame.with_scope(|body_scope| {
            (
                body_scope.id,
                crate::machine::core::assemble_body_chain(body_scope, call_site_chain, 0)
                    .parent
                    .clone(),
            )
        });
        let mut ids = Vec::with_capacity(statements.len());
        for (i, statement) in statements.into_iter().enumerate() {
            let statement_chain = LexicalFrame::push(parent.clone(), body_scope_id, i + 1);
            // Bracket `frame` as the ambient cart so the sub-slot inherits it (not the caller's),
            // restoring the previous on every exit path.
            let bid = self.with_active_frame(Rc::clone(frame), |host| {
                // A body's non-tail statements are statement positions in the body scope.
                host.dispatch_in_active_frame(
                    sched,
                    statement,
                    Some(statement_chain),
                    SubmitContext::Statement,
                )
            });
            ids.push(bid);
        }
        ids
    }

    /// Schedule a witnessed dep-finish against the slot's own scope: the finish folds the resolved
    /// deps into a witnessed aggregate carrier, naming every region the result reaches. `deps`
    /// mixes binder edges the cell classifier resolved with sub-dispatches this slot spawned (whose
    /// slots reclaim at their own finalize); the finish reads their results in dep order. The one
    /// submission door behind the aggregate literals.
    ///
    /// No apply-time inspect here: an already-errored dep surfaces at the slot's first poll,
    /// through the step-start pull and the short-circuit the continuation bakes in.
    pub(in crate::machine::execute) fn submit_dep_finish_witnessed_in_own_scope<'a>(
        &mut self,
        sched: &mut Scheduler<KoanWorkload>,
        deps: Deps<NodeId>,
        finish: super::WitnessedDepFinish<'a>,
    ) -> NodeId {
        // Clone the payload off the ambient before taking the scheduler for the submit.
        let payload = self
            .ambient
            .active_payload()
            .expect("a slot step installs the ambient payload before the body submits")
            .clone();
        let (cart, framed) = self.ambient.submission_cart();
        let anchor = SlotFrame::new(cart, payload.scope, payload.chain);
        // The witnessed finish rides the same delivery every dep-finish does: the short-circuit
        // gate over the `seal_witnessed` projection, run under the frameless dep-error label.
        let work = NodeWork::new(
            short_circuit(Some(dep_error_frame()), seal_witnessed(finish)),
            None,
        );
        let (sources, minted) =
            self.named_sources(sched, &anchor, deps, |_host, _sched, producer| {
                vec![producer]
            });
        let id = sched.alloc_node(work, &sources, anchor, framed);
        for source in minted {
            sched.release_edge(source);
        }
        id
    }
}

// ---------- Test-fixture submission prims ----------

/// Test-fixture submission prims that mint a run-lifetime [`SlotFrame`] anchor from a raw `scope`, so
/// scheduler tests stand up raw `NodeWork` slots through the harness. The run path routes a
/// `Dispatch` through [`Host::submit_expression`] instead.
#[cfg(test)]
impl<'run> KoanRuntime<'run> {
    /// A bare dep-finish work item that waits on its wired deps, short-circuits on the first
    /// errored dep under the [`dep_error_frame`] label, else hands the resolved values to a
    /// value-only `finish`.
    fn awaiting(finish: TerminalDepFinish<'run>) -> NodeWork<'run, KoanWorkload> {
        NodeWork::new(short_circuit(Some(dep_error_frame()), finish), None)
    }

    /// Ambient-chain submission for any `NodeWork`; with no slot step installed the node is placed
    /// at the caller-declared statement position `index`, exactly as [`Self::dispatch_in_scope`]
    /// places an expression.
    pub(in crate::machine::execute) fn add(
        &mut self,
        work: NodeWork<KoanWorkload>,
        sub_work: &[NodeId],
        scope: &'run Scope<'run>,
        index: usize,
    ) -> NodeId {
        let explicit_chain = self.host.statement_chain(scope, index);
        self.add_with_chain(work, sub_work, scope, explicit_chain)
    }

    /// Run-lifetime submission funnel: establish the run frame, decide the slot's [`NodeScope`]
    /// handle, default the chain to the ambient one, and submit the assembled [`SlotFrame`] anchor.
    pub(in crate::machine::execute) fn add_with_chain(
        &mut self,
        work: NodeWork<KoanWorkload>,
        sub_work: &[NodeId],
        scope: &'run Scope<'run>,
        explicit_chain: Option<Rc<LexicalFrame>>,
    ) -> NodeId {
        let KoanRuntime { sched, host } = self;
        host.ensure_run_frame(scope);
        let scope_handle = host.resolve_node_scope(scope);
        let chain = explicit_chain
            .or_else(|| host.ambient.active_payload().map(|p| p.chain.clone()))
            .expect("every dispatched node has a chain — submission outside enter_block / ambient payload is a bug");
        let (cart, framed) = host.ambient.submission_cart();
        let anchor = SlotFrame::new(cart, scope_handle, chain);
        let (sources, minted) = host.named_sources(
            sched,
            &anchor,
            Deps::from_requests(sub_work.iter().copied()),
            |_host, _sched, producer| vec![producer],
        );
        let id = sched.alloc_node(work, &sources, anchor, framed);
        for source in minted {
            sched.release_edge(source);
        }
        id
    }

    /// Schedule a dep-finish slot against an explicit `scope`. `sub_work` are the sub-Dispatches
    /// this slot spawned; each is named by a source destined at the slot's own region, which the
    /// install door inherits.
    pub(in crate::machine::execute) fn add_dep_finish(
        &mut self,
        sub_work: &[NodeId],
        scope: &'run Scope<'run>,
        finish: TerminalDepFinish<'run>,
        index: usize,
    ) -> NodeId {
        self.add(Self::awaiting(finish), sub_work, scope, index)
    }
}

#[cfg(test)]
mod run_tests;
#[cfg(test)]
mod tests;
