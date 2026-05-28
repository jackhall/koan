//! State envelope ridden by every `NodeWork::Dispatch` slot.
//!
//! Step 1 of the stateful-dispatch refactor introduces the carrier shape
//! without lighting up any per-variant behavior. Each per-variant struct
//! embeds `Initialized` by value so the universal `pre_subs` field rides
//! along structurally — dropping it requires an explicit destructure-and-
//! discard rather than a silent oversight. Later steps fill the per-variant
//! payloads with state cached across re-park (bare-name outcomes, candidate
//! lists, classified shape, etc.).
//!
//! The `'a` parameter on each per-variant struct is unused in step 1 — held
//! by a `PhantomData<&'a ()>` marker — but is declared now so later steps
//! can add borrowed fields without re-shaping the `DispatchState` enum
//! carrier (and without churning every pattern site in `execute.rs` /
//! `submit.rs` / `dispatch.rs`).
//!
//! Step 4b lights up the `eager_subs` track on `KeywordedState`: the
//! Resolved-with-eager-subs and `Deferred` paths now park the slot directly
//! on its eager subs (no intervening `NodeWork::Bind` allocation) and the
//! re-entry routes through `stateful_keyworded_resume_eager_subs`, which
//! splices the resolved futures into `working_expr` and either binds the
//! captured picked function (Resolved) or re-resolves dispatch against the
//! spliced expression (Deferred).
//!
//! Step 4c lights up the `bare_name_park` track on `KeywordedState`:
//! the Resolved-with-parked-bare-names path (a wrap or ref-name slot
//! whose bare name resolved to `Parked(producer)` during the fused part
//! walk) now installs the park edges and transitions to
//! `Keyworded(WaitingBareNamePark)` instead of rebuilding the slot as
//! `DispatchState::initialized` via the legacy `install_combined_park`.
//! On track completion `stateful_keyworded_resume_bare_name_park`
//! re-runs `stateful_keyworded_initial` against the carried
//! `working_expr` and the preserved `pre_subs`. Re-classification is
//! redundant for this path (the entry shape doesn't change across the
//! wake), but Step 4 is the *variant-state* lighting step — the per-
//! wake re-classify elimination falls out of Step 5's cutover once
//! every transition stays inside `Keyworded`.
//!
//! Visibility: `nodes.rs` lives at `crate::machine::execute::nodes`,
//! outside the `scheduler/` submodule. To let `NodeWork::Dispatch` name
//! `DispatchState`, every public symbol here uses
//! `pub(in crate::machine::execute)` — wide enough for `nodes.rs`, narrow
//! enough that no caller outside the execute tree sees the carrier.
//!
//! See `roadmap/dispatch_fix/stateful-dispatch-01-scaffolding.md`.

use crate::machine::core::kfunction::KFunction;
use crate::machine::model::ast::KExpression;
use crate::machine::NodeId;

/// Universal birth state of a Dispatch slot — the shape before
/// classification. Embedded by value in every per-variant state struct so
/// `pre_subs` rides along structurally rather than by convention.
pub(in crate::machine::execute) struct Initialized {
    /// Pre-submitted sub-Dispatches keyed by their slot index in
    /// `expr.parts`; populated by submit-time recursion for binder-shaped
    /// expressions (see `roadmap/dispatch_fix/nested-binder-submission.md`),
    /// empty otherwise. Phase 4 of `run_dispatch` reuses these instead of
    /// allocating fresh sub-Dispatches for the named slots.
    pub(in crate::machine::execute) pre_subs: Vec<(usize, NodeId)>,
}

pub(in crate::machine::execute) struct BareIdState<'a> {
    pub(in crate::machine::execute) init: Initialized,
    _ph: std::marker::PhantomData<&'a ()>,
}

pub(in crate::machine::execute) struct BareTypeState<'a> {
    pub(in crate::machine::execute) init: Initialized,
    _ph: std::marker::PhantomData<&'a ()>,
}

pub(in crate::machine::execute) struct CtorState<'a> {
    pub(in crate::machine::execute) init: Initialized,
    _ph: std::marker::PhantomData<&'a ()>,
}

pub(in crate::machine::execute) struct FnValueState<'a> {
    pub(in crate::machine::execute) init: Initialized,
    _ph: std::marker::PhantomData<&'a ()>,
}

pub(in crate::machine::execute) struct SigilState<'a> {
    pub(in crate::machine::execute) init: Initialized,
    _ph: std::marker::PhantomData<&'a ()>,
}

pub(in crate::machine::execute) struct KeywordedState<'a> {
    pub(in crate::machine::execute) init: Initialized,
    /// Eager-subs track installed by the Resolved-with-subs or `Deferred`
    /// arm of `stateful_keyworded_initial`. `None` is the initial-entry
    /// shape (the universal birth state has no tracks installed); the
    /// transition `Initialized → Keyworded` writes `Some` when the slot
    /// stages eager subs and parks waiting on them, and the re-entry
    /// `stateful_keyworded_resume_eager_subs` consumes it.
    pub(in crate::machine::execute) eager_subs: Option<EagerSubsTrack<'a>>,
    /// Bare-name park track installed by the Resolved-with-parked-bare-
    /// names arm of `stateful_keyworded_initial` (the path the legacy
    /// driver served via `install_combined_park`). `None` is the initial-
    /// entry shape; the transition `Initialized → Keyworded` writes
    /// `Some` when ≥1 wrap-slot or ref-name-slot bare name resolved to a
    /// `NameOutcome::Parked(producer)`. Mutually exclusive with
    /// `eager_subs` at install time — the part walk's park-precedence
    /// guard installs the park *before* staging any subs (submitting
    /// would leak nodes on the re-Dispatch wake path). On track
    /// completion `stateful_keyworded_resume_bare_name_park` re-runs
    /// `stateful_keyworded_initial` against the carried `working_expr`
    /// and `pre_subs`; the producers are sibling forward references that
    /// now resolve through `scope.resolve_with_chain`, so the rebuilt
    /// `bare_outcomes` cache picks up their now-bound values and the
    /// wrap-slot splice fires `Future(obj)` for them on the second pass.
    pub(in crate::machine::execute) bare_name_park: Option<BareNameParkTrack<'a>>,
}

/// Track state for the eager-subs sub-Dispatches a `Keyworded` slot is
/// parked on. Each `(part_idx, sub_id)` is the slot index in
/// `working_expr.parts` that the sub's resolved value will be spliced
/// into (as `ExpressionPart::Future(obj)`) at track completion, plus
/// the sub NodeId itself — the Owned dep this slot installed at park-
/// install time.
///
/// The track does NOT carry the picked function from the initial
/// resolve. Both the Resolved-with-eager-subs and `Deferred` arms
/// install the same track shape; on completion the resume handler
/// re-resolves dispatch against the spliced `working_expr` and uses
/// the re-resolve's pick. This matches legacy `run_bind`'s authority
/// surface — element-type mismatches a typed `Future(_)` reveals are
/// surfaced as `DispatchFailed` (non-match) rather than a bind-time
/// `TypeMismatch`, per the "element type is part of what an overload
/// matches, so a non-satisfying container is a non-match rather than
/// a committed-then-failed bind" contract.
///
/// `working_expr` carries every part that was *not* a sub (literals,
/// keywords, already-spliced wrap slots) plus an
/// `Identifier("")` placeholder at every sub index — the same shape
/// today's `NodeWork::Bind.expr` uses, so the post-completion
/// re-resolve sees the same input the legacy `run_bind` would.
pub(in crate::machine::execute) struct EagerSubsTrack<'a> {
    pub(in crate::machine::execute) working_expr: KExpression<'a>,
    pub(in crate::machine::execute) subs: Vec<(usize, NodeId)>,
    /// PhantomData here keeps the `'a` parameter alive without binding
    /// a now-unused field. Later sub-steps (4c bare-name park, 4d
    /// overload park) repurpose the lifetime; placing the marker now
    /// localizes the future churn.
    _ph: std::marker::PhantomData<&'a KFunction<'a>>,
}

impl<'a> EagerSubsTrack<'a> {
    pub(in crate::machine::execute) fn new(
        working_expr: KExpression<'a>,
        subs: Vec<(usize, NodeId)>,
    ) -> Self {
        Self { working_expr, subs, _ph: std::marker::PhantomData }
    }
}

/// Track state for the bare-name forward references a `Keyworded` slot
/// is parked on. Carries the partly-spliced `working_expr` (Resolved
/// wrap slots have already been substituted for `Future(obj)`; Parked
/// wrap and ref-name slots keep their original bare-name token) so the
/// re-entry `stateful_keyworded_resume_bare_name_park` can re-run
/// `stateful_keyworded_initial` against it. Producers are recorded for
/// debug/invariant tracing only — the re-entry rebuilds `bare_outcomes`
/// against the scope (which now sees the producers' bound values), so
/// the resume path doesn't read the list.
///
/// Park edges are installed as `Notify` (via `add_park_edge`) matching
/// the legacy `install_combined_park` shape: the producers are sibling
/// forward references, not children of this slot, so the slot's reclaim
/// walk must not transit into them.
pub(in crate::machine::execute) struct BareNameParkTrack<'a> {
    pub(in crate::machine::execute) working_expr: KExpression<'a>,
    pub(in crate::machine::execute) producers: Vec<NodeId>,
    _ph: std::marker::PhantomData<&'a KFunction<'a>>,
}

impl<'a> BareNameParkTrack<'a> {
    pub(in crate::machine::execute) fn new(
        working_expr: KExpression<'a>,
        producers: Vec<NodeId>,
    ) -> Self {
        Self { working_expr, producers, _ph: std::marker::PhantomData }
    }
}

/// One variant per `DispatchShape`, plus the pre-classification
/// `Initialized` birth state. Every Dispatch slot enters the driver in
/// `Initialized`; the stateful driver classifies and transitions to the
/// matching per-variant state on first entry. Step 1 of the refactor
/// delegates straight back to the legacy `run_dispatch` from every variant
/// so the carrier shape can land without any behavior change; later steps
/// replace each variant's delegation with a real handler.
pub(in crate::machine::execute) enum DispatchState<'a> {
    Initialized(Initialized),
    BareIdentifier(BareIdState<'a>),
    BareTypeLeaf(BareTypeState<'a>),
    ConstructorCall(CtorState<'a>),
    FunctionValueCall(FnValueState<'a>),
    SigiledTypeExpr(SigilState<'a>),
    Keyworded(KeywordedState<'a>),
}

impl<'a> DispatchState<'a> {
    /// Construct the universal birth state. Every submission and re-park
    /// site goes through this constructor so `pre_subs` is the only field
    /// any caller names.
    pub(in crate::machine::execute) fn initialized(pre_subs: Vec<(usize, NodeId)>) -> Self {
        DispatchState::Initialized(Initialized { pre_subs })
    }
}

// The per-variant constructors below exist so the stateful driver in
// `dispatch.rs` can transition `Initialized → <variant>` without touching
// the private `_ph` marker at every call site. The marker is held inside
// the struct so later steps can swap it for real borrowed state without
// changing the public construction surface.
impl<'a> BareIdState<'a> {
    pub(in crate::machine::execute) fn from_init(init: Initialized) -> Self {
        Self { init, _ph: std::marker::PhantomData }
    }
}

impl<'a> BareTypeState<'a> {
    pub(in crate::machine::execute) fn from_init(init: Initialized) -> Self {
        Self { init, _ph: std::marker::PhantomData }
    }
}

impl<'a> CtorState<'a> {
    pub(in crate::machine::execute) fn from_init(init: Initialized) -> Self {
        Self { init, _ph: std::marker::PhantomData }
    }
}

impl<'a> FnValueState<'a> {
    pub(in crate::machine::execute) fn from_init(init: Initialized) -> Self {
        Self { init, _ph: std::marker::PhantomData }
    }
}

impl<'a> SigilState<'a> {
    pub(in crate::machine::execute) fn from_init(init: Initialized) -> Self {
        Self { init, _ph: std::marker::PhantomData }
    }
}

impl<'a> KeywordedState<'a> {
    pub(in crate::machine::execute) fn from_init(init: Initialized) -> Self {
        Self { init, eager_subs: None, bare_name_park: None }
    }

    /// Build the parked-on-eager-subs shape. Used by the
    /// `Initialized → Keyworded(WaitingEagerSubs)` transition in
    /// `stateful_keyworded_initial` — `init` is the just-consumed birth
    /// state (its `pre_subs` was read into the part walk), `track` carries
    /// the staged subs the slot now parks on.
    pub(in crate::machine::execute) fn with_eager_subs(
        init: Initialized,
        track: EagerSubsTrack<'a>,
    ) -> Self {
        Self { init, eager_subs: Some(track), bare_name_park: None }
    }

    /// Build the parked-on-bare-name-producers shape. Used by the
    /// `Initialized → Keyworded(WaitingBareNamePark)` transition in
    /// `stateful_keyworded_initial` when the part walk discovers ≥1
    /// `NameOutcome::Parked(producer)` on a wrap or ref-name slot. `init`
    /// carries the `pre_subs` forward across re-Dispatch (the resume
    /// handler hands them back to `stateful_keyworded_initial` so the
    /// binder recursive-submission optimization survives the wake), and
    /// `track` carries the partly-spliced `working_expr` plus the producer
    /// list for invariant tracing.
    pub(in crate::machine::execute) fn with_bare_name_park(
        init: Initialized,
        track: BareNameParkTrack<'a>,
    ) -> Self {
        Self { init, eager_subs: None, bare_name_park: Some(track) }
    }
}
