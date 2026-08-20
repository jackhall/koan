//! The apply harness — koan's half of the drain protocol.
//!
//! [`KoanRuntime`] owns the [`Scheduler`] and the koan-side [`Host`] as two sibling fields, so the
//! drain can borrow the scheduler mutably while the host stays reachable: the whole run is
//! `sched.drain(|sched, step| host.step(sched, step))` ([`KoanRuntime::execute`]). [`Host::step`]
//! is the drain callback — it opens the sealed continuation at the step brand, re-brands the
//! pre-read dep terminals against the step's coverage, brackets the ambient frame, runs the decide
//! to an [`Outcome`], drains the step's `WriteOp` effects sink, and hands the outcome to
//! [`Host::apply`], which maps it onto the [`StepVerdict`] the drain applies. Dep wiring has one
//! door, [`Host::wire_deps`]; slot retirement is the [`Workload::retiring`] hook. The
//! dispatch-submission wrappers live on the host too — the apply side's other writers.
//!
//! See [execution](../../../design/execution/README.md) and
//! [memory-model](../../../design/memory-model.md).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::machine::core::KoanStorageProfile;
use crate::machine::core::bindings::{WriteGate, WriteOp};
use crate::machine::core::scope_frame;
use crate::machine::core::{BlockEntry, BlockRequest, DepPlacement, FramePlacement, ScopeId};
use crate::machine::core::{ProgramBrand, RegionBrand, ScopeRefFamily};
use crate::machine::model::CarriedFamily;
use crate::machine::model::{
    ExpressionPart, KExpression, Part, PartClass, WorkingExpression, WorkingPart,
};
use crate::machine::{
    BindingIndex, CallFrame, DeliveredCarried, FrameCoverage, Installer, KError, KErrorKind,
    LexicalFrame, NodeId, Scope,
};
use crate::scheduler::{
    Anchor, Dep, Deps, DrainDeadlock, EdgeId, InstalledEdge, Scheduler, Step, StepVerdict, Workload,
};
use crate::scheduler::{DeliveryDestination, SealedTerminal};
use crate::witnessed::{BumpVec, SealedExtern, Within, erase_to_static};

use super::ambient::AmbientContext;
use super::decide::{
    BodyPlacement, DecideCtx, DepRequest, SubmitContext, statement_binder_plan, with_node_scope,
};
use super::finalize::{NodeFinalize, finalize_error};
use super::lift::relocate_seam;
use super::nodes::{ChainOp, NodePayload, NodeScope, NodeWork, SlotFrame, WorkLabel};
use super::obligation::with_obligation;
use super::outcome::{
    Await, Continuation, DepTerminal, Outcome, ParkDeps, TerminalDepFinish, dep_error_frame,
};
use super::{
    ContinuationFamily, catch_continuation, ignore_results, seal_witnessed, short_circuit,
};

/// The Koan instantiation of the scheduler's [`Workload`] interface — the marker that binds the
/// opaque scheduler types to their concrete Koan forms.
pub(in crate::machine::execute) struct KoanWorkload;

impl Workload for KoanWorkload {
    type Value = CarriedFamily;
    type Error = KError;
    type Profile = KoanStorageProfile;
    type Frame = SlotFrame;
    type Continuation = ContinuationFamily;

    /// Koan's half of delivery: the walk decides which destinations a terminal lands in;
    /// [`relocate_seam`] decides what the crossing costs.
    fn deliver(
        terminal: &DeliveredCarried,
        dest: DeliveryDestination<KoanWorkload>,
    ) -> DeliveredCarried {
        relocate_seam(terminal, dest)
    }

    /// The slot-retirement hook: the edges this slot still owns, released by the drain at the one
    /// point the slot stops being able to release them. The retire first drops any claim still
    /// naming one, which keeps the store from ever holding a [`ProducerId`] whose edge is gone.
    /// `take` empties the anchor, so the release is exactly-once by construction.
    ///
    /// Keyed on the slot's own [`BindingIndex`] — the one address it knows about itself — so a
    /// commit that already retired its claim costs an array index and a zero test, and no path
    /// scans a binding table. A slot that claimed nothing (a bare-name forward owning only its
    /// classification edge, every non-binder statement) reads an empty record and returns.
    fn retiring(anchor: &SlotFrame) -> Vec<EdgeId> {
        if anchor.installed_claims() {
            let index = BindingIndex::value(anchor.payload.chain.index);
            with_node_scope(&anchor.payload.scope, Some(&anchor.cart), |scope| {
                scope.retire_claims(index, &mut WriteGate::for_run_loop());
            });
        }
        anchor.take_owned_edges()
    }
}

/// The koan-side state the drain callback runs against. Split from the scheduler so `drain` can
/// hold `&mut Scheduler` while the step body holds `&mut Host`.
pub(in crate::machine::execute) struct Host<'run> {
    /// The ambient per-step context. See [`super::ambient`].
    pub(in crate::machine::execute) ambient: AmbientContext,
    /// This run's program storage capability, handed to every step through its [`DecideCtx`]. It
    /// also carries `'run`: the scheduler is value-erased (`Scheduler<KoanWorkload>`), so without
    /// this the run lifetime would live only in the harness's own method signatures.
    pub(in crate::machine::execute) program: ProgramBrand<'run>,
    /// The run's output sink, waiting for [`ensure_run_frame`](Self::ensure_run_frame) to move it
    /// onto the run frame — the writer's real home. It waits here rather than being passed at the
    /// mint because the mint is lazy: a dispatch into the run scope establishes the frame if
    /// nothing has yet, and that call site has no writer to supply.
    out: Option<Box<dyn std::io::Write>>,
}

/// The embedder handle: the scheduler and the host, owned side by side. The embedder API is the
/// [`interpret`](super::interpret) ladder; the rest of this type is the crate-internal
/// drive-and-read surface.
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
    /// read.
    pub fn execute(&mut self) -> Result<(), KError> {
        let KoanRuntime { sched, host } = self;
        sched.drain(|sched, step| host.step(sched, step)).map_err(
            |DrainDeadlock { pending, sample }| {
                KError::new(KErrorKind::SchedulerDeadlock {
                    pending,
                    sample: sample.sample(),
                })
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
        let statements: Vec<WorkingExpression<'run>> = exprs
            .into_iter()
            .map(|expr| WorkingExpression::from_ast(root.brand(), expr))
            .collect();
        // Each root edge names the run frame's region as its destination; holding that owner
        // across the install is the wiring-time proof the region is pinned.
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
        // Koan owns the roots, so koan releases them — before the run frame they name tears down.
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
        // Seal the run root's reach-set; it is run-global and never reopens.
        root.close();
        // A bare top-level expression is an untyped resolution boundary: an unstamped empty `[]` /
        // `{}` reaching it has no element type to infer, so reject rather than silently resolving
        // to `List<Any>` / `Dict<Any, Any>`.
        for &edge in roots {
            // Copy the verdict out from inside the open — the carrier never escapes.
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
    /// caller-declared statement position, numbered exactly as if the expression were the
    /// `index`-th line of a file. Only the submission's driver knows that position, so it is a
    /// parameter. The statement-at-a-time door; whole programs go through [`Self::run_program`].
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

    /// An edge's delivered terminal's error, or `Ok(())` on success — the value-free probe.
    pub fn edge_result_error(&self, edge: EdgeId) -> Result<(), &KError> {
        self.sched.edge_result_error(edge)
    }

    /// Open an edge's delivered terminal at a rank-2 brand and hand the value to `f`, returning its
    /// result or the terminal's error. See [`Scheduler::read_edge_result_with`].
    #[cfg(test)]
    pub(crate) fn read_edge_result_with<R>(
        &self,
        edge: EdgeId,
        f: impl for<'b> FnOnce(crate::machine::model::Carried<'b>) -> R,
    ) -> Result<R, &KError> {
        self.sched.read_edge_result_with(edge, f)
    }

    /// Wire an edge onto `producer`, destined at `scope`'s own region — the test harness's
    /// stand-in for the root edge `run_program` installs. Slots reclaim at finalize, so a test that
    /// reads a result must hold an edge exactly as production does. Not `#[cfg(test)]`: the
    /// unconditional `builtins::test_support` harness reaches it.
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
    /// The slot store is a free-list whose length is a high-water mark, and a finished slot's
    /// terminal retains its producer frame. Both are program-lifetime facts about the scheduler,
    /// not the run, so a test measuring one phase's slot footprint or frame retention releases the
    /// prior phase's slots first.
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

/// Test-only forwarders. No `&mut Scheduler` escapes — the accessor hands out `&Scheduler`.
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
    /// the active-scope carrier at one rank-2 `for<'b>` step brand, re-brands the pre-read dep
    /// terminals once against the step's coverage, brackets the ambient frame
    /// ([`Self::with_slot_step`]), runs the continuation to an [`Outcome`], applies the step's
    /// binding writes, and maps the outcome onto the returned [`StepVerdict`] through
    /// [`Self::apply`]. The closure's result cannot name `'b`, so a `Replace` verdict's
    /// continuation exits through [`erase_to_static`].
    ///
    /// `'scratch` is the drain's per-pop scratch borrow. `'run: 'scratch` holds at the call site —
    /// the borrow lives inside the `drain` call, which the run outlives — and is what lets the open
    /// below take its `Within` token against `'scratch` rather than `'run`.
    fn step<'scratch>(
        &mut self,
        sched: &mut Scheduler<KoanWorkload>,
        step: Step<'scratch, KoanWorkload>,
    ) -> StepVerdict<'static, KoanWorkload>
    where
        'run: 'scratch,
    {
        let Step {
            scratch,
            id,
            anchor,
            continuation: sealed_continuation,
            dep_results,
        } = step;
        // Read the step's context off the anchor as values up front, so nothing holds a scope
        // borrow across the step's work or a tail hop's frame swap.
        let cart = Rc::clone(&anchor.cart);
        let node_scope = anchor.payload.scope;
        let chain = anchor.payload.chain.clone();
        // The **step's coverage**: one bundle covering every re-anchor the open performs — the
        // scope operand and each dep cell — assembled before the open so it outlives `'b`. See
        // [the step's coverage](../../../design/per-node-memory.md#the-steps-coverage).
        let combined: FrameCoverage = FrameCoverage::of(Rc::clone(anchor.owner()));
        // Re-brand each delivered resident **once**, here, so every later read opens pin-free.
        let mut dep_sources: BumpVec<'_, Result<DepTerminal<'_>, KError>> =
            BumpVec::with_capacity_in(dep_results.len(), scratch);
        dep_sources.extend(dep_results.iter().map(
            |resident: &Result<SealedTerminal<KoanWorkload>, KError>| match resident {
                Ok(cell) => Ok(DepTerminal {
                    cell: cell.brand_with(&combined),
                }),
                Err(error) => Err(error.clone()),
            },
        ));
        // The active scope as a carrier; `combined` pins it either way.
        let scope_carrier = match node_scope {
            NodeScope::Yoked => cart.scope_sealed(),
            NodeScope::YokedChild(carrier) => carrier,
        };
        // The `Within` token's declared `'scratch: 'b` is what lets two live borrow-checked
        // capabilities — `self.program` (a `ProgramBrand<'run>`, not a sealed carrier) and the
        // drain's `scratch` handle — be stored in the view without shortening either brand by hand:
        // both are covariant, so each shortens to `'b` by ordinary subtyping once the token supplies
        // the outlives fact the quantifier would otherwise erase. `'run: 'b` still follows, through
        // the method's `'run: 'scratch` bound, so the `DecideCtx`'s `'program: 'step` obligation
        // stays discharged at `'program = 'run`.
        sealed_continuation.open(
            scope_carrier,
            &combined,
            |_within: Within<'_, 'scratch>,
             continuation: super::outcome::NodeContinuation<'_>,
             scope| {
                // The step's binding-write sink. Declared inside the step brand because a
                // `WriteOp` carries seals branded to `scope`'s region — so "nothing crosses steps"
                // is the borrow checker's rule here, not a convention.
                let step_effects: RefCell<Vec<WriteOp<'_>>> = RefCell::new(Vec::new());
                // The whole tail — decide, effects, apply — runs inside the ambient bracket, so
                // the apply reads the step-end frame and the deposited obligation off the ambient
                // context directly.
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
                                scratch,
                            ),
                            &dep_sources,
                            id,
                        );
                        // Drained here so the writes take a firm borrow and land before any edge
                        // an errored step would strand. See [the step's binding
                        // writes](../../../design/execution/classify-and-apply.md#the-steps-binding-writes).
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
    /// [`StepVerdict`] the drain applies. Runs inside the step's ambient bracket, so the step-end
    /// frame and the deposited obligation are ambient reads.
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
                // The producer's per-call frame gates the return obligation: a frameless /
                // run-frame producer folds in nothing. Retention seeds independently — the
                // scheduler reads the slot's own anchor owner at finalize — so the gate makes no
                // memory decision.
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
                        // `seal_at_step` is the sole exit from the step brand: it discharges
                        // `'step` into a lifetime-free envelope pinned by the anchor's own region
                        // owner — the same owner the scheduler seeds as the slot's retention host.
                        // The delivery walk reads the envelope's coverage to adopt into each
                        // destination, so no call site here names a reach.
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
                label,
            } => {
                // A tail iteration (`FreshTail`) retires this scope before the fresh cart is
                // installed for the next; other placements keep the current scope live.
                if matches!(frame, FramePlacement::FreshTail { .. }) {
                    anchor.close_opened_scope();
                }
                let new_frame = frame.fresh_frame();
                // Erased here, where the overlay is still live, so the frameless replace below
                // can install it as the slot's `YokedChild`.
                let overlay_scope = match block_entry {
                    BlockEntry::Overlay(overlay) => {
                        Some(SealedExtern::<ScopeRefFamily>::erase(overlay))
                    }
                    BlockEntry::None | BlockEntry::FrameScope(_) => None,
                };
                // The frame the body runs in: the freshly minted cart, else the slot's current one
                // (an `Inherit` FN-body re-enters the cart a prior `Continue` installed).
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
                        debug_assert!(
                            overlay_scope.is_none(),
                            "a framed tail-replace carries no overlay scope"
                        );
                        // The claims and the statement identity belong to the slot, not to the
                        // anchor it wears, so the incoming anchor takes them over from the retiring
                        // one. `opening` rather than `replacing` because `f` is a cart minted for
                        // this slot: it opens its scope here and closes it at its own finish.
                        Some(SlotFrame::opening(
                            f,
                            NodeScope::Yoked,
                            new_chain,
                            anchor,
                            label,
                        ))
                    }
                    None => {
                        // A frameless replace keeps the prior cart, and a tail entering an overlay
                        // without a fresh frame (USING) takes the overlay as its scope. Mint a
                        // fresh anchor only when the overlay scope or the chain changed; a pure
                        // re-decide (same cart, scope, and chain) keeps the anchor.
                        let has_overlay = overlay_scope.is_some();
                        let scope =
                            overlay_scope.map_or(anchor.payload.scope, NodeScope::YokedChild);
                        (has_overlay || chain_changed).then(|| {
                            SlotFrame::replacing(
                                Rc::clone(&step_frame),
                                scope,
                                new_chain,
                                anchor,
                                label,
                            )
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
                let installed = self.wire_deps(sched, anchor, id, brand, deps);
                // **Install-and-inspect**: a decide never probes a producer's standing, so a park
                // whose producer had already finalized is classified here. An errored one is
                // propagated now rather than waited on — a terminal slot never notifies again, so
                // the park would never wake.
                for verdict in &installed {
                    let InstalledEdge::Filled(edge) = verdict else {
                        continue;
                    };
                    if let Err(dep_error) = sched.edge_result_error(*edge) {
                        let error = super::decide::propagate_dep_error(dep_error, park_error_frame);
                        return self.apply(sched, Outcome::Done(Err(error)), brand, id, anchor);
                    }
                }
                // Lower each variant to its outermost live continuation.
                let continuation = match continuation {
                    // A dispatch finish carries its own dep-error frame (the consuming call's, or
                    // `None` frameless); an action/literal dep-finish carries the
                    // `dep_error_frame()` label.
                    Continuation::Finish(finish) => short_circuit(park_error_frame, finish),
                    // The catch carries its single watched dep unrealized; `catch_continuation`
                    // runs the finish without short-circuiting on a dep error.
                    Continuation::Catch { watched, finish } => {
                        let _watched_verdict = self.wire_deps(
                            sched,
                            anchor,
                            id,
                            brand,
                            ParkDeps::List(Deps::from_requests([watched])),
                        );
                        catch_continuation(finish)
                    }
                    // A decide takes no dep values, so `ignore_results` drops the results slice.
                    Continuation::Resume { resume } => ignore_results(resume),
                };
                // Carry the ambient obligation across the park so the resumed step's
                // declared-return check still fires. The wrap sits on the outermost closure, so
                // every variant — including the dep-error short-circuit — runs under it.
                let continuation =
                    with_obligation(self.ambient.current_obligation_duplicate(), continuation);
                // The degenerate replace: same cart, scope, and chain, so no anchor swaps in —
                // and with it the slot keeps the `WorkLabel` its submission minted.
                replace_verdict(NodeWork::new(continuation), None)
            }
            Outcome::Forward(source) => {
                // The slot's result *is* the result behind `source`. Classification is the
                // install's, not a probe's: wiring a second edge off `source` answers
                // filled-or-parked and leaves a name this slot can read through. A forward wants
                // no destination of its own — the slot stands in for the producer — so the new edge
                // inherits `source`'s destination and nothing relocates; parked, `Alias` drives the
                // splice instead. The classification edge joins the slot's owned list, so a checker
                // micro-step can re-emit `Forward` on an edge of its own rather than on a foreign
                // claim its binder may have retired in the meantime.
                let installed = sched.install_edge_from(source);
                anchor.own_edges([installed.edge_id()]);
                let Some(obligation) = self.ambient.current_obligation_duplicate() else {
                    return match installed {
                        InstalledEdge::Filled(edge) => StepVerdict::Forward(edge),
                        InstalledEdge::Parked(_) => StepVerdict::Alias(source),
                    };
                };
                // A residual declared-return obligation must be discharged before the rehomed
                // terminal reaches any consumer. Take it out of the ambient so neither this step's
                // finalize nor the not-ready micro-step's continuation re-observes it.
                self.ambient.take_obligation();
                match installed {
                    // The producer resolved: run the declared-return check inline against its
                    // terminal, then behave as the obligation-free ready path.
                    InstalledEdge::Filled(edge) => {
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
                    // Not yet resolved: park a checker micro-step on it (an already-terminal
                    // producer never re-notifies, so a park is sound only here). Its finish
                    // re-emits `Forward` on a pass, re-entering this arm obligation-free with the
                    // producer now resolved — no re-check, no loop. Both the park and the
                    // re-emission name `edge`, this slot's own name for the producer.
                    InstalledEdge::Parked(edge) => {
                        let finish: TerminalDepFinish<'step> = Box::new(move |view, terminals| {
                            // The single parked dep is the producer behind `edge`, at index 0.
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
/// fresh brand.
fn replace_verdict(
    work: NodeWork<'_, KoanWorkload>,
    anchor: Option<Rc<SlotFrame>>,
) -> StepVerdict<'static, KoanWorkload> {
    StepVerdict::Replace {
        work: NodeWork::new(erase_to_static::<ContinuationFamily>(work.continuation)),
        anchor,
    }
}

// ---------- Dep wiring ----------

impl<'run> Host<'run> {
    /// **The dep-wiring door.** Resolve `deps` to source edges, mint the consumer's own edge off
    /// each source, then release the minted sources, whose only job was carrying producer and
    /// destination into the install. Returns one filled-or-parked verdict per wired dep.
    fn wire_deps<'a>(
        &mut self,
        sched: &mut Scheduler<KoanWorkload>,
        anchor: &SlotFrame,
        consumer: NodeId,
        brand: RegionBrand<'a>,
        deps: ParkDeps<'a>,
    ) -> Vec<InstalledEdge> {
        let (sources, minted) = match deps {
            ParkDeps::List(list) => {
                self.named_sources(sched, anchor, list, |host, sched, request| {
                    host.realize_dep(sched, brand, request)
                })
            }
            ParkDeps::Block(block) => self.block_sources(sched, anchor, block),
        };
        let installed = sched.install_deps(consumer, &sources);
        for source in minted {
            sched.release_edge(source);
        }
        installed
    }

    /// [`Self::wire_deps`]'s naming half for a **dep list**, shared with the submission path (which
    /// has no slot yet, so it hands its sources to [`Scheduler::alloc_node`]): resolve the list to
    /// one **source edge** per entry, in dep order, plus the sources this call minted — which the
    /// caller releases once the door it feeds has minted the consumer's own edges.
    ///
    /// One entry in, one source out. That is the whole reason the block fan-out lives in
    /// [`Self::block_sources`] rather than here: it is what makes a caller's [`Deps::request`] index
    /// the position its result comes back at.
    fn named_sources<R>(
        &mut self,
        sched: &mut Scheduler<KoanWorkload>,
        anchor: &SlotFrame,
        deps: Deps<R>,
        mut realize: impl FnMut(&mut Self, &mut Scheduler<KoanWorkload>, R) -> NodeId,
    ) -> (Vec<EdgeId>, Vec<EdgeId>) {
        let entries = deps.into_entries();
        let mut sources: Vec<EdgeId> = Vec::with_capacity(entries.len());
        let mut minted: Vec<EdgeId> = Vec::new();
        for entry in entries {
            match entry {
                Dep::Producer(source) => sources.push(source),
                Dep::Request(request) => {
                    let producer = realize(self, sched, request);
                    sources.push(self.mint_source(sched, anchor, producer, &mut minted));
                }
            }
        }
        (sources, minted)
    }

    /// [`Self::wire_deps`]'s naming half for a **statement block**: fan the block out to one
    /// producer per statement and name each by a minted source, in declaration order. Every source
    /// here is minted, so the two vectors agree — the second is returned all the same, so both
    /// naming doors hand their caller the same pair.
    fn block_sources<'a>(
        &mut self,
        sched: &mut Scheduler<KoanWorkload>,
        anchor: &SlotFrame,
        block: BlockRequest<'a>,
    ) -> (Vec<EdgeId>, Vec<EdgeId>) {
        let producers = match block {
            // A body block fans out one producer per statement: into a fresh per-call frame's own
            // scope, or — under `Inherit` — into a caller-allocated overlay (USING).
            BlockRequest::Body {
                statements,
                placement: BodyPlacement::Frame(frame),
            } => self.dispatch_body(sched, &frame, statements),
            BlockRequest::Body {
                statements,
                placement: BodyPlacement::Overlay(overlay),
            } => self.enter_block(sched, overlay.id, statements, overlay),
            // A declaration builtin's body splits into its top-level statements against the child
            // scope it minted (MODULE, SIG).
            BlockRequest::InScope { body, scope } => {
                let statements = split_working_body(scope.brand(), body);
                self.enter_block(sched, scope.id, statements, scope)
            }
        };
        let mut minted: Vec<EdgeId> = Vec::with_capacity(producers.len());
        let sources: Vec<EdgeId> = producers
            .into_iter()
            .map(|producer| self.mint_source(sched, anchor, producer, &mut minted))
            .collect();
        (sources, minted)
    }

    /// Name a spawned `producer` by a source edge destined at this slot's own anchor region — where
    /// a sub-result belongs — recording it in `minted` for the caller to release. The install door
    /// mints the slot's real dep edge off it and inherits that destination, so sub-work needs no
    /// second wiring rule.
    fn mint_source(
        &mut self,
        sched: &mut Scheduler<KoanWorkload>,
        anchor: &SlotFrame,
        producer: NodeId,
        minted: &mut Vec<EdgeId>,
    ) -> EdgeId {
        let source = sched.install_edge(producer, anchor.owner());
        minted.push(source);
        source
    }

    /// Realize one [`DepRequest`] to **its** producer node. `brand` is the realizing step's, where
    /// an aggregate literal's per-element dispatch node is bumped.
    ///
    /// `OwnScope` re-dispatches against the executing slot's own scope; `InScope` enters a fresh
    /// **single-statement** block (so an inner `LET` stays local). A body that splits across
    /// statements is a [`BlockRequest`] and goes through [`Self::block_sources`] instead.
    pub(in crate::machine::execute) fn realize_dep<'a>(
        &mut self,
        sched: &mut Scheduler<KoanWorkload>,
        brand: RegionBrand<'a>,
        dep: DepRequest<'a>,
    ) -> NodeId {
        match dep {
            DepRequest::Dispatch {
                expr,
                placement: DepPlacement::OwnScope,
            } => self.dispatch_in_own_scope(sched, expr, SubmitContext::SubDispatch),
            DepRequest::Dispatch {
                expr,
                placement: DepPlacement::InScope(scope),
            } => self.enter_block_once(sched, scope.id, expr, scope),
            DepRequest::ListLit(items) => self.schedule_list_literal(sched, brand, items),
            DepRequest::DictLit(pairs) => self.schedule_dict_literal(sched, brand, pairs),
            DepRequest::RecordLit(fields) => self.schedule_record_literal(sched, brand, fields),
        }
    }
}

/// Split a body block into the statements it fans out to, one working node apiece. The
/// scheduler-side peer of [`split_body_statements`](crate::machine::split_body_statements): a block
/// (two or more parts, every one an expression) yields its children, and any other body is the one
/// statement it already is.
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

/// The lexical chain of the statement at zero-based `index` in a block over `scope_id`, under the
/// enclosing `parent` chain — the index rule, written once for both block doors. The pushed frame
/// index is `index + 1`: visibility is strict less-than and builtins sit at idx 0, so a statement at
/// index 1 sees them via `0 < 1`.
fn block_statement_chain(
    parent: Option<Rc<LexicalFrame>>,
    scope_id: ScopeId,
    index: usize,
) -> Rc<LexicalFrame> {
    LexicalFrame::push(parent, scope_id, index + 1)
}

/// The name-channel collisions among a block's statements, as `statement position → error`.
///
/// Ruled on here, at the fan-out, because here both declaring statements are in hand and neither
/// has run: the diagnostic names both positions, and which one is rejected does not depend on which
/// body finishes first. A statement's plan is its own spine, so a block's whole namespace is legible
/// from the statement keys alone and this costs one pass over borrowed keys.
///
/// The **bucket** channel is deliberately absent: sibling overloads under one head keyword are the
/// point of that channel, so a shared bucket key is a co-declaration rather than a collision, and
/// the per-signature `DuplicateOverload` check rules on it at seal time where the signatures exist.
fn duplicate_declarations(statements: &[WorkingExpression<'_>]) -> HashMap<usize, KError> {
    let mut declared: HashMap<&str, usize> = HashMap::new();
    let mut rejected: HashMap<usize, KError> = HashMap::new();
    for (position, statement) in statements.iter().enumerate() {
        let Some((name, _kind)) = statement_binder_plan(statement).and_then(|plan| plan.name)
        else {
            continue;
        };
        match declared.get(name) {
            Some(&first) => {
                rejected.insert(
                    position,
                    KError::new(KErrorKind::DuplicateDeclaration {
                        name: name.to_string(),
                        first: first + 1,
                        second: position + 1,
                    }),
                );
            }
            None => {
                declared.insert(name, position);
            }
        }
    }
    rejected
}

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
    /// and everything later while showing every earlier binding. With a slot step installed this is
    /// `None`, inheriting the ambient payload's chain.
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
            // A lazily-established run frame — one reached before any entry-point mint — gets the
            // sink, which is what a run with no caller-supplied writer already meant.
            let out = self.out.take().unwrap_or_else(|| Box::new(std::io::sink()));
            self.ambient.set_run_frame(CallFrame::adopting(scope, out));
        }
    }

    /// Decide a run-scope submission's [`NodeScope`] handle — always cart-witnessed, never anchored
    /// at a free `'run`. The witnessing frame is the active cart, else the run frame: its *own*
    /// scope yields [`NodeScope::Yoked`], and a scope whose region it merely pins
    /// ([`CallFrame::pins_scope_region`]) yields [`NodeScope::YokedChild`] — a block scope
    /// allocated in an ancestor region, held by the frame's `FrameStorage.outer` chain, stored
    /// erased and reattached frame-bounded.
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
        let mut duplicates = duplicate_declarations(&statements);
        // The one act that builds a claim store: the block's fan-out sizes the scope's statement
        // run, so every claim a statement below stamps lands at an index the run already reaches.
        scope.begin_block(statements.len(), &mut WriteGate::for_run_loop());
        statements
            .into_iter()
            .enumerate()
            .map(|(i, expr)| {
                let chain = block_statement_chain(parent.clone(), scope_id, i);
                let rejected = duplicates.remove(&i);
                self.block_statement(sched, chain, expr, scope, rejected)
            })
            .collect()
    }

    /// Submit `statement` as a **fresh single-statement** lexical block over `scope` —
    /// [`Self::enter_block`]'s one-statement case, which an `InScope` dep takes directly rather than
    /// through a one-element vector.
    pub(in crate::machine::execute) fn enter_block_once<'a>(
        &mut self,
        sched: &mut Scheduler<KoanWorkload>,
        scope_id: ScopeId,
        statement: WorkingExpression<'a>,
        scope: &'a Scope<'a>,
    ) -> NodeId {
        let parent = self.ambient.active_payload().map(|p| p.chain.clone());
        scope.begin_block(1, &mut WriteGate::for_run_loop());
        // One statement collides with nothing, so the fan-out's duplicate rule has nothing to say.
        let chain = block_statement_chain(parent, scope_id, 0);
        self.block_statement(sched, chain, statement, scope, None)
    }

    /// Submit one block statement on its resolved `chain` — the per-statement half both block
    /// doors run through.
    ///
    /// `rejected` is the fan-out's verdict on this statement: a statement the block ruled out
    /// before it ran ([`duplicate_declarations`]) submits pre-errored, so it claims nothing and its
    /// slot is terminal from birth.
    fn block_statement<'a>(
        &mut self,
        sched: &mut Scheduler<KoanWorkload>,
        chain: Rc<LexicalFrame>,
        statement: WorkingExpression<'a>,
        scope: &'a Scope<'a>,
        rejected: Option<KError>,
    ) -> NodeId {
        let Some(error) = rejected else {
            return self.dispatch_in_scope_with_chain(sched, statement, scope, Some(chain));
        };
        self.ensure_run_frame(scope);
        let node_scope = self.resolve_node_scope(scope);
        self.submit_pre_errored(sched, &statement, node_scope, chain, error)
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
        // Every caller of this door is a statement position, so a binder installs its plan here.
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
    /// [`Self::dispatch_in_active_frame`].
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
    /// [`assemble_body_chain`](crate::machine::core::assemble_body_chain). The caller tail-replaces
    /// into the last statement separately.
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
            // Bracket `frame` as the ambient cart so the sub-slot inherits it, not the caller's.
            let bid = self.with_active_frame(Rc::clone(frame), |host| {
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
    /// deps into a witnessed aggregate carrier, naming every region the result reaches, and reads
    /// their results in dep order. The one submission door behind the aggregate literals.
    ///
    /// No apply-time inspect here: an already-errored dep surfaces at the slot's first poll,
    /// through the step-start pull and the short-circuit the continuation bakes in.
    pub(in crate::machine::execute) fn submit_dep_finish_witnessed_in_own_scope<'a>(
        &mut self,
        sched: &mut Scheduler<KoanWorkload>,
        deps: Deps<NodeId>,
        finish: super::WitnessedDepFinish<'a>,
    ) -> NodeId {
        let payload = self
            .ambient
            .active_payload()
            .expect("a slot step installs the ambient payload before the body submits")
            .clone();
        let (cart, framed) = self.ambient.submission_cart();
        let anchor = SlotFrame::new(cart, payload.scope, payload.chain, WorkLabel::None);
        let work = NodeWork::new(short_circuit(
            Some(dep_error_frame()),
            seal_witnessed(finish),
        ));
        let (sources, minted) =
            self.named_sources(sched, &anchor, deps, |_host, _sched, producer| producer);
        let id = sched.alloc_node(work, &sources, anchor, framed);
        for source in minted {
            sched.release_edge(source);
        }
        id
    }
}

// ---------- Test-fixture submission prims ----------

/// Test-fixture submission prims that mint a run-lifetime [`SlotFrame`] anchor from a raw `scope`,
/// so scheduler tests stand up raw `NodeWork` slots through the harness.
#[cfg(test)]
impl<'run> KoanRuntime<'run> {
    /// A bare dep-finish work item that waits on its wired deps, short-circuits on the first
    /// errored dep under the [`dep_error_frame`] label, else hands the resolved values to a
    /// value-only `finish`.
    fn awaiting(finish: TerminalDepFinish<'run>) -> NodeWork<'run, KoanWorkload> {
        NodeWork::new(short_circuit(Some(dep_error_frame()), finish))
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
        let anchor = SlotFrame::new(cart, scope_handle, chain, WorkLabel::None);
        let (sources, minted) = host.named_sources(
            sched,
            &anchor,
            Deps::from_requests(sub_work.iter().copied()),
            |_host, _sched, producer| producer,
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
