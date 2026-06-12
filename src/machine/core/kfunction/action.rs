//! The scheduler-aware `Action` currency (WIP, gated behind the `action-harness` feature). The peer of
//! [`super::exec::ExecOutcome`]: where `ExecOutcome` is what `run_user_fn` returns (scheduler-
//! *unaware*), `Action` is what a builtin returns and what the harness interprets (scheduler-*aware*).
//! These are the **types only** — they reference core/model types, never `SchedulerHandle`. The
//! interpreter that drives the scheduler from a `Action` lives one layer up in
//! `machine::execute::harness::interpret` (the peer of `dispatch/exec.rs::invoke`).
//!
//! See `scratch/action-spec.md` and `scratch/action-survey{,-r2,-r3}/` for the survey + audit this shape
//! was distilled from.

use std::rc::Rc;

use super::body::{BodyResult, ReturnContract};
use crate::machine::core::{CallArena, LexicalFrame, Scope, ScopeId};
use crate::machine::model::ast::KExpression;
use crate::machine::model::values::Held;
use crate::machine::model::types::KType;
use crate::machine::model::{Carried, KObject};
use crate::machine::{BindingIndex, KError, KErrorKind, NodeId};

/// Unwrap a `Result<T, KError>` inside an `Action`-returning body, early-returning
/// `Action::Done(Err(e))` on the error arm — the `Action`-body analogue of `?`. Collapses the
/// pervasive `match helper(…) { Ok(v) => v, Err(e) => return Action::Done(Err(e)) }` envelope.
/// `#[macro_export]` hoists it to the crate root, so call it as `crate::try_action!(…)` from
/// anywhere with no import.
#[macro_export]
macro_rules! try_action {
    ($expr:expr) => {
        match $expr {
            Ok(value) => value,
            Err(error) => {
                return $crate::machine::core::kfunction::action::Action::Done(Err(error))
            }
        }
    };
}

/// Read a builtin argument's `KObject` from a `BodyCtx::args` `KObject::Record` by name. `None` if
/// the args aren't a record or the named field is a type cell. Two lifetimes: the borrow (`'c`,
/// `BodyCtx`'s) is shorter than the content (`'a`, the run).
pub fn arg_object<'a, 'c>(args: &'c KObject<'a>, name: &str) -> Option<&'c KObject<'a>> {
    match args {
        KObject::Record(fields, _) => fields.get(name).and_then(Held::as_object),
        _ => None,
    }
}

/// Read a builtin argument's `KType` (a type-cell arg) from `BodyCtx::args` by name.
pub fn arg_type<'a, 'c>(args: &'c KObject<'a>, name: &str) -> Option<&'c KType<'a>> {
    match args {
        KObject::Record(fields, _) => fields.get(name).and_then(Held::as_type),
        _ => None,
    }
}

/// Read a builtin argument's raw cell ([`Held::Object`] / [`Held::Type`]) from `BodyCtx::args` by
/// name — for builtins that branch on the value vs type channel (e.g. LET's name/value slots).
pub fn arg_held<'a, 'c>(args: &'c KObject<'a>, name: &str) -> Option<&'c Held<'a>> {
    match args {
        KObject::Record(fields, _) => fields.get(name),
        _ => None,
    }
}

/// Read a builtin argument's `KType` (a type-cell arg), or the canonical `require_ktype`
/// diagnostic — `TypeMismatch{expected: "TypeExprRef"}` for an object cell, `MissingArg` when
/// absent. The `Action`-side twin of [`ArgumentBundle::require_ktype`](super::argument_bundle::ArgumentBundle::require_ktype).
pub fn require_ktype<'a>(args: &KObject<'a>, name: &str) -> Result<KType<'a>, KError> {
    match arg_held(args, name) {
        Some(Held::Type(kt)) => Ok(kt.clone()),
        Some(Held::Object(o)) => Err(KError::new(KErrorKind::TypeMismatch {
            arg: name.to_string(),
            expected: "TypeExprRef".to_string(),
            got: o.ktype().name(),
        })),
        None => Err(KError::new(KErrorKind::MissingArg(name.to_string()))),
    }
}

/// Resolve the bare type-name in the `Type`-arm of arg `slot` — the binder name of a
/// type-defining builtin (UNION / NEWTYPE / MODULE / SIG / RECURSIVE) — or the canonical error:
/// `MissingArg` for an absent slot, `ShapeError` for a structural type. `surface` is the keyword
/// embedded in the diagnostic. The `Action`-side twin of
/// [`extract_bare_type_name`](super::argument_bundle::extract_bare_type_name).
pub fn require_bare_type_name<'a>(
    args: &KObject<'a>,
    slot: &str,
    surface: &str,
) -> Result<String, KError> {
    match arg_type(args, slot) {
        Some(t) => super::argument_bundle::bare_type_name(t, slot, surface),
        None => Err(KError::new(KErrorKind::MissingArg(slot.to_string()))),
    }
}

/// Convert a [`BodyResult`] into an [`Action`] for the deferral helpers that reuse the
/// existing `BodyResult`-returning `finalize_*` functions inside an `Action` finish. Those
/// finalizers only ever produce `Value` / `Err`; a `Tail` / `DeferTo` would be a logic error.
pub(crate) fn body_result_to_action<'a>(result: BodyResult<'a>) -> Action<'a> {
    match result {
        BodyResult::Value(carried) => Action::Done(Ok(carried)),
        BodyResult::Err(error) => Action::Done(Err(error)),
        BodyResult::Tail { .. } | BodyResult::DeferTo(_) => {
            unreachable!("a field-list / fn-def finalize only yields Value or Err")
        }
    }
}

/// Extract a cloned `KExpression` from arg `slot`, or the canonical parenthesized-slot
/// `ShapeError` (`"<builtin> <slot> slot must be a parenthesized expression"`). The `Action`-side
/// twin of [`ArgumentBundle::extract_kexpression_or_shape_error`](super::argument_bundle::ArgumentBundle::extract_kexpression_or_shape_error),
/// owning that error text so every `KExpression`-slot builtin reports it identically.
pub fn require_kexpression<'a>(
    args: &KObject<'a>,
    builtin: &str,
    slot: &str,
) -> Result<KExpression<'a>, KError> {
    match arg_object(args, slot) {
        Some(KObject::KExpression(e)) => Ok(e.clone()),
        _ => Err(KError::new(KErrorKind::ShapeError(format!(
            "{builtin} {slot} slot must be a parenthesized expression"
        )))),
    }
}

/// A builtin body: `fn(&BodyCtx) -> Action`. The builtin mutates `BodyCtx.scope` directly (binding
/// install is a scope write, not an `Action` effect) and returns an `Action` describing the
/// scheduler continuation.
pub type ActionFn = for<'a> fn(&BodyCtx<'a, '_>) -> Action<'a>;

/// Read-only-ish context a builtin body receives. `scope` is **interior-mutable**: the builtin
/// binds / registers / allocs on it directly before returning a `Action`. `frame` is a *reference to
/// the cart `Rc`* (so MODULE can `Rc::clone` it into `KType::Module`), `None` for def-time builtins.
/// `chain` is `None` for a top-level binder (`bind_index` → `BindingIndex::BUILTIN`). `args` is the
/// builtin's arguments as a `KObject::Record`; unevaluated args ride as `KObject::KExpression`
/// cells.
pub struct BodyCtx<'a, 'c> {
    pub scope: &'c Scope<'a>,
    pub frame: Option<&'c Rc<CallArena>>,
    /// The ambient lexical chain (an `Rc`, as `current_lexical_chain` hands it out — binders read
    /// its `index` for `BindingIndex`, MATCH passes it to `resolve_type_expr`). `None` at top level.
    pub chain: Option<Rc<LexicalFrame>>,
    pub args: &'c KObject<'a>,
}

impl<'a, 'c> BodyCtx<'a, 'c> {
    /// The lexical position a binding the builtin installs takes: the ambient chain's index, or
    /// [`BindingIndex::BUILTIN`] when there is no chain (a top-level / direct-body binder, e.g. a
    /// test fixture that bypasses the scheduler).
    pub fn bind_index(&self) -> BindingIndex {
        self.chain
            .as_ref()
            .map(|chain| BindingIndex::value(chain.index))
            .unwrap_or(BindingIndex::BUILTIN)
    }
}

/// Wake-time context a finish receives: the slot's **own** scope (interior-mutable, with `.arena`)
/// re-projected at wake — a deferred binder `register_*`s on it here.
pub struct FinishCtx<'a, 'c> {
    pub scope: &'c Scope<'a>,
}

/// A `Combine` finish: re-entered at wake with the resolved dep values, yielding another `Action` the
/// harness recurses into. No `&mut SchedulerHandle` — exec's continuation pattern.
pub type Cont<'a> = Box<dyn FnOnce(&FinishCtx<'a, '_>, &[Carried<'a>]) -> Action<'a> + 'a>;

/// A `Catch` finish: re-entered with the watched slot's `Result`, yielding a `Action`.
pub type CatchCont<'a> =
    Box<dyn FnOnce(&FinishCtx<'a, '_>, Result<&'a KObject<'a>, KError>) -> Action<'a> + 'a>;

/// What happens next for a slot — the four shapes the builtin survey reduced everything to.
pub enum Action<'a> {
    /// Produce a value / error for this slot (after any direct scope mutation the builtin did).
    Done(Result<Carried<'a>, KError>),
    /// Tail-replace into `tail` (after the `leading` body statements, dispatched as siblings),
    /// carrying `contract`, in a cart per `frame_placement`. `block_entry` is the body/arm scope id
    /// when the tail enters a fresh lexical block (MATCH / TRY arms, FN-body tails) — `None` for a
    /// frameless / current-block continuation (EVAL). The harness derives the body-statement chains
    /// and the tail's `body_index` from `block_entry` + `leading`.
    Tail {
        leading: Vec<KExpression<'a>>,
        tail: KExpression<'a>,
        contract: Option<ReturnContract<'a>>,
        frame_placement: FramePlacement<'a>,
        block_entry: Option<ScopeId>,
    },
    /// Dispatch `deps`, then `finish` over their resolved values yields the next `Action`.
    Combine { deps: Vec<Dep<'a>>, finish: Cont<'a> },
    /// Watch `watched`, recover via `finish`.
    Catch { watched: Dep<'a>, finish: CatchCont<'a> },
}

/// A Combine/Tail dependency. `Dispatch` → an owned sub-slot the harness dispatches; `Existing` → a
/// producer NodeId the builtin already found in scope (a forward-ref / pending type) kept alive as
/// a park-producer.
pub enum Dep<'a> {
    Dispatch { expr: KExpression<'a>, placement: DepPlacement<'a> },
    Existing(NodeId),
}

/// Where a `Dep::Dispatch` attaches — collapses the `_here` / `_in_frame` / `_with_chain` zoo.
pub enum DepPlacement<'a> {
    /// The slot's own `NodeScope` (`add_dispatch_here`) — binders' type sub-dispatches.
    OwnScope,
    /// The active frame's child (`add_dispatch_in_frame`) — FN-body leading statements.
    ActiveFrame,
    /// A builtin-minted child scope (module/sig/recursive/using body), carried by reference. In a
    /// `Combine` a multi-statement body fans out one sub-dispatch per top-level statement
    /// (`enter_body_block`); in a `Catch` a single watched expr enters a fresh lexical block
    /// (`enter_block`).
    InScope(&'a Scope<'a>),
}

/// The cart a `Tail` runs in.
pub enum FramePlacement<'a> {
    /// Reuse the slot's ping-pong reserve cart (`acquire_tail_frame(outer)`). The TCO tail-call
    /// frame — FN-body invoke, deferred `PerCall` tails. The only harness-constructed cart.
    ReuseReserve { outer: &'a Scope<'a> },
    /// A **pre-built** fresh cart the builtin minted (`CallArena::new`, never the reserve), handed
    /// to the harness to install. The builtin owns construction because it may seed the cart before
    /// the tail dispatches — MATCH/TRY bind `it` into it via `with_anchored_child`; EVAL builds it
    /// for the UAF guard.
    FreshChild { frame: Rc<CallArena> },
    /// No new frame; continue in the slot's current cart. Frameless tails / `Done`.
    Inherit,
}
