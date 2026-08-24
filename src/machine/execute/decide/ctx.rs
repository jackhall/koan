//! The decide-phase context.
//!
//! [`DecideCtx`] is the surface every dispatch *decide* runs against: the ambient step context —
//! scope, destination frame, installer identity, effects sink, obligations, types, writer, the
//! program brand, and the step's scratch arena — and nothing else. A decide holds no scheduler borrow at all: every graph
//! question is either the install's answer (the park verdicts the harness reads off
//! [`InstalledEdge`](crate::scheduler::InstalledEdge)) or foreclosed by the language's lexical
//! well-foundedness rule (a park can never wait forward, so no decide probes for cycles). A shape
//! handler decides against this and returns an [`Outcome`](super::Outcome) that the harness
//! ([`super::super::harness`]) applies, so no shape module mutates the scheduler.

use std::cell::RefCell;
use std::rc::Rc;

use crate::machine::core::bindings::WriteOp;
use crate::machine::core::scope_frame;
use crate::machine::core::{FrameStorage, ProgramBrand, RunWriter, StepAllocator};
use crate::machine::model::types::TypeRegistry;
use crate::machine::model::{ExpressionPart, RunRegistries, WorkingPart};
use crate::machine::{CallFrame, Installer, LexicalFrame, Scope};
use crate::source::Spanned;
use crate::witnessed::{BumpAllocator, BumpVec};

use super::super::ambient::AmbientContext;
use super::super::nodes::NodeScope;
use super::super::obligation::ReturnObligation;
use super::resolve::{Resolution, resolve_name};

/// Run `f` with a [`NodeScope`] handle's scope opened at a `for<'b>` brand. A `Yoked` slot
/// re-projects from the active cart through [`CallFrame::with_scope`]; a `YokedChild` slot opens its
/// erased cart-ancestor [`SealedExtern<ScopeRefFamily>`](crate::witnessed::SealedExtern) carrier at
/// the same brand, pinned by `frame`. Either way the `&Scope<'b>` is confined to `f`, so no borrow
/// rides up a `&mut` path.
pub(in crate::machine::execute) fn with_node_scope<R>(
    node_scope: &NodeScope,
    frame: Option<&Rc<CallFrame>>,
    f: impl for<'b> FnOnce(&'b Scope<'b>) -> R,
) -> R {
    let frame = frame.expect("a slot keeps its active cart");
    match node_scope {
        NodeScope::YokedChild(carrier) => carrier.open(frame, f),
        NodeScope::Yoked => frame.with_scope(f),
    }
}

/// Run `f` with the active slot's scope recovered from the ambient payload, for a path that holds
/// the ambient context rather than the step's branded scope. Panics outside a slot step; within a
/// step the scope is always present.
pub(in crate::machine::execute) fn with_current_node_scope<R>(
    ambient: &AmbientContext,
    f: impl for<'b> FnOnce(&'b Scope<'b>) -> R,
) -> R {
    let payload = ambient
        .active_payload()
        .expect("a slot step installs the ambient payload (and a Yoked slot keeps its frame)");
    with_node_scope(&payload.scope, ambient.active_frame_ref(), f)
}

/// The frame storage owning the active slot's scope region, read through the ambient payload — the
/// ambient-context analogue of [`DecideCtx::dest_frame`]. Routes through `scope_frame`, the
/// liveness invariant's single owner.
pub(in crate::machine::execute) fn current_dest_frame(
    ambient: &AmbientContext,
) -> Rc<FrameStorage> {
    with_current_node_scope(ambient, scope_frame)
}

/// The decide-phase context: the ambient step values a shape handler reads while deciding. A
/// `DecideCtx` lives only for the decide call, and its borrows end before the harness applies the
/// outcome, so decide and apply never overlap.
pub(in crate::machine::execute) struct DecideCtx<'program: 'step, 'step, 'view> {
    /// Per-step context for the scope/chain reads (`chain_deref`, `active_chain`, `current_frame`)
    /// and the obligation slot.
    ambient: &'view AmbientContext,
    /// The active slot's scope, opened at the step brand and handed in by the harness's step
    /// `open`, so [`Self::current_scope`] returns it directly. It carries the cart content lifetime
    /// `'step` every decide runs at; a longer-lived program-storage `KExpression` reaches that
    /// lifetime by ordinary subtyping, the node being covariant.
    scope: &'step Scope<'step>,
    /// The `Rc<FrameStorage>` owning the active scope's region — resolved once per step by the
    /// harness while the step machinery holds it, so step code reads a live frame with no failure
    /// path.
    dest_frame: Rc<FrameStorage>,
    /// The statement this view's slot is running, as an [`Installer`]. A binder body reads it
    /// through [`Self::installer`] to stamp the installing declaration's identity onto its `types`
    /// entry.
    installer: Installer,
    /// The step's binding-write sink, owned and drained by the harness's step — see [the step's
    /// binding writes](../../../../design/execution/classify-and-apply.md#the-steps-binding-writes).
    /// **Private**, with one `pub(in crate::machine::execute)` deposit method: a builtin receives
    /// a [`BodyCtx`](crate::machine::BodyCtx), which does not carry it, and nothing outside the
    /// execute layer can deposit.
    effects: &'view RefCell<Vec<WriteOp<'step>>>,
    /// The run's program storage capability, minted once per run and carried unchanged across every
    /// step. A builtin body reaches it through [`BodyCtx::program`](crate::machine::BodyCtx), and
    /// synthesizing a **value-channel** node (`OP`'s bridge body) builds against it. It is carried
    /// **unshortened**, at its own `'program`, related to the step lifetime only by the struct's
    /// `'program: 'step` bound — so a mint door reached through it
    /// pins its parts at program storage, not at the step.
    program: ProgramBrand<'program>,
    /// The step's **scratch arena**: the drain's per-pop bump, handed down at `'step`. A staging
    /// buffer that is built, read and dropped inside the decide belongs here rather than on the
    /// global heap — the drain resets the bump at the next pop, so the bytes cost nothing to
    /// reclaim.
    ///
    /// Carried at `'step` and not one lifetime longer, which is the whole confinement: a
    /// [`BumpVec`](crate::witnessed::BumpVec) built through this handle names `'step` in its type.
    /// An `Outcome` is `'step` too, so a park's dep list ([`StepDeps`](super::StepDeps)) rides the
    /// arena by design; what the type rules out is a buffer reaching past the pop — the `'static`
    /// continuation the harness seals, or the `StepVerdict` it returns — as a borrow-check error
    /// rather than a convention someone has to keep.
    scratch: BumpAllocator<'step>,
}

impl<'program: 'step, 'step, 'view> DecideCtx<'program, 'step, 'view> {
    pub(in crate::machine::execute) fn new(
        ambient: &'view AmbientContext,
        scope: &'step Scope<'step>,
        dest_frame: Rc<FrameStorage>,
        installer: Installer,
        effects: &'view RefCell<Vec<WriteOp<'step>>>,
        program: ProgramBrand<'program>,
        scratch: BumpAllocator<'step>,
    ) -> Self {
        Self {
            ambient,
            scope,
            dest_frame,
            installer,
            effects,
            program,
            scratch,
        }
    }

    /// The step's scratch allocator, `Copy` like the handle it wraps. See the field docs for why
    /// the `'step` it comes back at is the confinement.
    pub(in crate::machine::execute) fn scratch(&self) -> BumpAllocator<'step> {
        self.scratch
    }

    pub(in crate::machine::execute) fn program(&self) -> ProgramBrand<'program> {
        self.program
    }

    /// Append this step's next batch of binding writes to the harness-owned sink, preserving the
    /// order the bodies decided them in. The only way into `effects`.
    pub(in crate::machine::execute) fn deposit_effects(&self, ops: Vec<WriteOp<'step>>) {
        self.effects.borrow_mut().extend(ops);
    }

    pub(in crate::machine::execute) fn installer(&self) -> Installer {
        self.installer
    }

    pub(in crate::machine::execute) fn current_scope(&self) -> &'step Scope<'step> {
        self.scope
    }

    /// The run's lookup state, read through the ambient context's run frame — the currency for
    /// anything that renders a label or constructs a record.
    pub(in crate::machine::execute) fn registries(&self) -> &RunRegistries {
        self.ambient.registries()
    }

    /// The run's subtype-verdict store, read through the ambient context's run frame. Memoized
    /// predicates take it as their final parameter.
    pub(in crate::machine::execute) fn types(&self) -> &TypeRegistry {
        &self.registries().types
    }

    /// The run's output sink, read through the ambient context's run frame — the same channel and
    /// the same owner as [`Self::types`].
    pub(in crate::machine::execute) fn out(&self) -> &RunWriter {
        self.ambient.writer()
    }

    pub(super) fn chain_deref(&self) -> Option<&LexicalFrame> {
        self.ambient.active_payload().map(|p| &*p.chain)
    }

    /// Cloned `Rc` to the active chain — for the type-leaf and field-list reads that take it by
    /// value, and the `record_type` elaborator deferral.
    pub(super) fn active_chain(&self) -> Option<Rc<LexicalFrame>> {
        self.ambient.active_payload().map(|p| p.chain.clone())
    }

    /// Cloned `Rc` to the active per-call frame. `None` only outside any frame (top-level builtins).
    pub(in crate::machine::execute) fn current_frame(&self) -> Option<Rc<CallFrame>> {
        self.ambient.active_frame_ref().cloned()
    }

    /// The frame storage owning the active scope's region — infallible: resolved at step entry from
    /// what the step machinery already holds. The destination frame for in-step allocation
    /// (`alloc_witnessed` / `yoke_branded`) and relocation.
    pub(in crate::machine::execute) fn dest_frame(&self) -> Rc<FrameStorage> {
        Rc::clone(&self.dest_frame)
    }

    /// The step construction allocator wrapping [`Self::dest_frame`], branded at the step lifetime
    /// `'step` — its doors return a [`StepCarried`](crate::machine::execute::StepCarried) confined to
    /// the step (`design/scheduler-library.md` guarantees 3 and 5), handed to a finish through
    /// [`FinishCtx`](crate::machine::core::FinishCtx).
    pub(in crate::machine::execute) fn step_ctx(&self) -> StepAllocator<'step> {
        StepAllocator::over_frame(self.dest_frame())
    }

    /// Deposit the slot's declared-return obligation into the ambient slot-step state — the reach
    /// the [`with_obligation`](super::super::obligation::with_obligation) wrapper closure runs to
    /// carry the checker down the tail chain.
    pub(in crate::machine::execute) fn deposit_obligation(&self, obligation: ReturnObligation) {
        self.ambient.deposit_obligation(obligation)
    }

    /// Duplicate the chain's established obligation without removing it — keep-first and park
    /// propagation read it to wrap the replacement continuation, and `enter_user_fn` asks
    /// `.is_some()` of it to detect a tail call within an established chain.
    pub(in crate::machine::execute) fn current_obligation_duplicate(
        &self,
    ) -> Option<ReturnObligation> {
        self.ambient.current_obligation_duplicate()
    }

    /// Build the per-part `bare_outcomes` cache: one [`resolve_name`] per bare-name part, `None`
    /// otherwise. The cache carries no error channel — a producer's failure reaches this consumer
    /// through the park the harness installs instead.
    pub(super) fn build_bare_outcomes(
        &self,
        parts: &[Spanned<WorkingPart<'step>>],
    ) -> BumpVec<'step, Option<Resolution>> {
        let active_chain = self.ambient.active_payload().map(|p| &p.chain);
        let mut outcomes = BumpVec::with_capacity_in(parts.len(), self.scratch());
        outcomes.extend(parts.iter().map(|p| match p.value.as_ast() {
            Some(ast @ (ExpressionPart::Identifier(_) | ExpressionPart::Type(_))) => Some(
                resolve_name(self.current_scope(), &ast, active_chain, self.registries()),
            ),
            _ => None,
        }));
        outcomes
    }
}
