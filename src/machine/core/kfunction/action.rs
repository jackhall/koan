//! The scheduler-aware `Action` currency. The peer of
//! [`super::exec::ExecOutcome`]: where `ExecOutcome` is what `run_user_fn` returns (scheduler-
//! *unaware*), `Action` is what a builtin returns and what the harness interprets (scheduler-*aware*).
//! These are the **types only** — they reference core/model types, never the scheduler. The
//! interpreter that drives the scheduler from an `Action` lives one layer up in
//! `machine::execute::runtime::run_action` (the peer of `dispatch/exec.rs::invoke`).

use std::rc::Rc;

use super::body::ReturnContract;
use crate::machine::core::{CallFrame, FrameStorage, LexicalFrame, Scope, ScopeId};
use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::model::types::{KType, Record};
use crate::machine::model::values::{CarriedFamily, Held};
use crate::machine::model::{Carried, KObject};
use crate::machine::{BindingIndex, FrameSet, KError, KErrorKind, NodeId};
use crate::scheduler::DepResults;
use crate::witnessed::{Sealed, Witnessed};

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
            Err(error) => return $crate::machine::core::kfunction::action::Action::Done(Err(error)),
        }
    };
}

/// The `Rc<FrameStorage>` that owns `scope`'s region — the witness a value built into that region is
/// `yoke`d under (the object-family construction inversion: a region-resident object is born bundled
/// with its frame as its reach). The scope's `region_owner` is `Weak` — an in-region value holds no
/// owning `Rc` back to its frame — and upgrades for the whole of the scope's own step, while the
/// producing node holds the frame live.
pub fn scope_frame(scope: &Scope<'_>) -> Rc<FrameStorage> {
    scope
        .region_owner()
        .upgrade()
        .expect("a producing scope's frame is live during its own step")
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

/// Read a builtin argument's `KType` (a type-cell arg), or the canonical diagnostic —
/// `TypeMismatch{expected: "ProperType"}` for an object cell, `MissingArg` when absent.
pub fn require_ktype<'a>(args: &KObject<'a>, name: &str) -> Result<KType<'a>, KError> {
    match arg_held(args, name) {
        Some(Held::Type(kt)) => Ok(kt.clone()),
        Some(Held::Object(o)) => Err(KError::new(KErrorKind::TypeMismatch {
            arg: name.to_string(),
            expected: "ProperType".to_string(),
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
        Some(t) => bare_type_name(t, slot, surface),
        None => Err(KError::new(KErrorKind::MissingArg(slot.to_string()))),
    }
}

/// Resolve a resolved `KType` to its bare type name, for the binders that read their name from a
/// `KObject::Record` type cell. A simple / nominal leaf yields its `name()`; a structural type
/// (List, Record, FN, …) is a `ShapeError`. `surface` is the keyword (`"NEWTYPE"`, `"UNION"`, …)
/// embedded in the message.
fn bare_type_name<'a>(t: &KType<'a>, name: &str, surface: &str) -> Result<String, KError> {
    match t {
        KType::Number
        | KType::Str
        | KType::Bool
        | KType::Null
        | KType::Identifier
        | KType::KExpression
        | KType::SigiledTypeExpr
        | KType::RecordType
        | KType::OfKind(_)
        | KType::Unresolved(_)
        | KType::Any
        | KType::SetRef { .. }
        | KType::Signature { .. }
        | KType::Module { .. }
        | KType::AbstractType { .. } => Ok(t.name()),
        KType::List(_)
        | KType::Dict(_, _)
        | KType::Record(_)
        | KType::KFunction { .. }
        | KType::KFunctor { .. }
        | KType::DeferredReturn(_)
        | KType::SetLocal(_)
        | KType::Variant { .. }
        | KType::RecursiveRef(_)
        | KType::RecursiveGroup(_)
        | KType::ConstructorApply { .. } => Err(KError::new(KErrorKind::ShapeError(format!(
            "{surface} {name} must be a bare type name, got `{}`",
            t.render(),
        )))),
    }
}

/// Extract a cloned `KExpression` from arg `slot`, or the canonical parenthesized-slot
/// `ShapeError` (`"<builtin> <slot> slot must be a parenthesized expression"`), owning that error
/// text so every `KExpression`-slot builtin reports it identically.
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
    pub scope: &'a Scope<'a>,
    pub frame: Option<&'c Rc<CallFrame>>,
    /// The ambient lexical chain (an `Rc`, as `current_lexical_chain` hands it out — binders read
    /// its `index` for `BindingIndex`, MATCH passes it to `resolve_type_identifier`). `None` at top level.
    pub chain: Option<Rc<LexicalFrame>>,
    pub args: &'c KObject<'a>,
    /// Per-parameter reach carriers, keyed by parameter name: the [`Sealed`] carrier of each argument
    /// that arrived as a resolved value (a spliced sub-result or a bound-name read), naming every
    /// region that value reaches. A value-embedding body folds the carrier of the value it deposits (a
    /// bind into the scope reach-set) or `merge`s the one it embeds (a `Wrapped` / re-tagged `Record`),
    /// so the result names that reach by construction. A scalar-literal argument is region-pure and has
    /// no entry — [`arg_carrier`](Self::arg_carrier) reads `None`, i.e. "no foreign reach".
    pub arg_carriers: &'c Record<Sealed<CarriedFamily, FrameSet>>,
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

    /// The reach carrier of argument `name` — `Some` when it arrived as a resolved value (so a
    /// value-embedding body can fold / merge it), `None` for a scalar-literal (region-pure) argument.
    pub fn arg_carrier(&self, name: &str) -> Option<&Sealed<CarriedFamily, FrameSet>> {
        self.arg_carriers.get(name)
    }
}

/// Wake-time context a finish receives: the slot's **own** scope (interior-mutable, with `.region`)
/// re-projected at wake — a deferred binder `register_*`s on it here.
pub struct FinishCtx<'a> {
    pub scope: &'a Scope<'a>,
}

/// A `AwaitDeps` finish: re-entered at wake with the resolved dep values as a [`DepResults`] view
/// (addressed by `park` / `owned` position), yielding another `Action` the harness recurses into.
/// Reads only a `FinishCtx`, never the scheduler — exec's continuation pattern.
pub type AwaitContinue<'a> =
    Box<dyn FnOnce(&FinishCtx<'a>, DepResults<'_, Carried<'a>>) -> Action<'a> + 'a>;

/// The watched value as a `Catch` finish receives it on success: the value **relocated** into the
/// consumer region (for a finish that reads it — TRY-WITH's `it` bind) plus the watched producer's own
/// [`Sealed`] carrier (for a finish that builds a *witnessed* result — CATCH's `Result`, folded via
/// [`transfer_into`](crate::witnessed::Sealed::transfer_into) so it names every region the watched
/// value reaches). On a watched error the finish gets the `KError` instead.
pub struct CatchOk<'a> {
    pub value: Carried<'a>,
    pub carrier: Sealed<CarriedFamily, FrameSet>,
}

/// A `Catch` finish: re-entered with the watched slot's [`CatchOk`] (or error), yielding a `Action`.
pub type CatchContinue<'a> =
    Box<dyn FnOnce(&FinishCtx<'a>, Result<CatchOk<'a>, KError>) -> Action<'a> + 'a>;

/// What happens next for a slot — the four shapes the builtin survey reduced everything to.
pub enum Action<'a> {
    /// Produce this slot's terminal (after any direct scope mutation the builtin did): a witnessed
    /// value or an error. The `Ok` carrier is built **inside the witness closure** — already bundled
    /// with the set of regions it reaches ([`yoke`](crate::witnessed::Witnessed::yoke) / `merge` at
    /// the alloc site, or a `seal_value` / `resident_type_carrier` sealing a constructed or read
    /// value) — so it is co-located by construction rather than paired with an asserted witness at
    /// finalize. The construction terminal for **both** channels: a builtin that allocates a `KObject`
    /// or a `KType` seals it here.
    Done(Result<Witnessed<CarriedFamily, FrameSet>, KError>),
    /// Tail-replace into `tail`, carrying `contract`, in a cart per `frame_placement`. When
    /// `leading` (the body's non-tail statements) is non-empty the slot first parks on them as
    /// owned deps and tail-replaces only once they resolve — so they run, and cascade-free, before
    /// the tail continues. `block_entry` names the lexical block the tail enters (see
    /// [`BlockEntry`]); the harness derives the body-statement chains and the tail's `body_index`
    /// from it + `leading`.
    Tail {
        leading: Vec<KExpression<'a>>,
        tail: KExpression<'a>,
        contract: Option<ReturnContract<'a>>,
        frame_placement: FramePlacement<'a>,
        block_entry: BlockEntry<'a>,
    },
    /// Dispatch `deps`, then `finish` over their resolved values yields the next `Action`.
    AwaitDeps {
        deps: Vec<DepRequest<'a>>,
        finish: AwaitContinue<'a>,
    },
    /// Watch `watched`, recover via `finish`.
    Catch {
        watched: DepRequest<'a>,
        finish: CatchContinue<'a>,
    },
}

#[cfg(test)]
impl<'a> Action<'a> {
    /// Seal a **region-pure** bare value as a `Done` terminal — the test-only constructor for a
    /// marker object that references no foreign region ([`Witnessed::resident`] fixes the empty
    /// witness). Production never mints a bare terminal: a real value is always built witnessed at its
    /// alloc site (`seal_value` / `yoke` / `merge` / `resident_*_carrier`), so this stays behind
    /// `cfg(test)`.
    pub(crate) fn done_resident(value: Carried<'a>) -> Self {
        Action::Done(Ok(Witnessed::resident(value)))
    }
}

/// The dependency currency both an [`Action`] (`AwaitDeps` / `Catch`) and a dispatch
/// [`Outcome::ParkThenContinue`](crate::machine::execute) declare — the one dep type, defined here in
/// core so `Action` can carry it without core depending on the execute layer.
///
/// `Dispatch` → an owned sub-slot the harness dispatches; `Existing` → a producer NodeId already in
/// scope (a forward-ref / pending type) the builder parks on. These two arms are the whole
/// builtin-`Action` currency. The remaining arms are dispatcher-only lowerings a builtin never
/// constructs: `ListLit` / `DictLit` / `RecordLit` schedule an aggregate literal as one owned
/// producer, and `BodyBlock` fans a non-tail statement block out to one owned producer per statement
/// (see [`BodyPlacement`] for where they bind). The harness assembles the realized deps into a
/// [`Deps`](crate::scheduler::Deps) builder — parks first, owned in declaration order — and a finish
/// addresses their results through a [`DepResults`] view (`park` / `owned`), where an
/// `InScope`-placed `Dispatch` and a `BodyBlock` each fan their multi-statement body out to one owned
/// result per statement.
pub enum DepRequest<'a> {
    Dispatch {
        expr: KExpression<'a>,
        placement: DepPlacement<'a>,
    },
    Existing(NodeId),
    ListLit(Vec<ExpressionPart<'a>>),
    DictLit(Vec<(ExpressionPart<'a>, ExpressionPart<'a>)>),
    RecordLit(Vec<(String, ExpressionPart<'a>)>),
    /// A body's non-tail statements dispatched as a block, fanning out to one owned producer per
    /// statement (the harness `extend`s them in declaration order). `placement` picks where they
    /// bind (see [`BodyPlacement`]): a deferred-return FN's first-call body and a leading-carrying
    /// arm bind into a fresh per-call frame's own scope; a leading-carrying USING binds into an
    /// inherited-cart overlay.
    BodyBlock {
        statements: Vec<KExpression<'a>>,
        placement: BodyPlacement<'a>,
    },
}

/// Where a [`DepRequest::BodyBlock`]'s statements bind — the two block fan-outs a leading-carrying
/// tail chooses between.
pub enum BodyPlacement<'a> {
    /// Dispatch as body-chain siblings in `frame`'s own scope (`KoanRuntime::dispatch_body`) — a
    /// deferred-return FN's first-call body (its non-tail body + the return-type expression) and
    /// MATCH / TRY arm leading statements. The only dep that carries its own frame.
    Frame(Rc<CallFrame>),
    /// Enter `overlay` as a fresh lexical block without a per-call frame (`KoanRuntime::enter_block`)
    /// — USING's leading statements, which bind into the transparent overlay inside the inherited
    /// call-site cart.
    Overlay(&'a Scope<'a>),
}

/// Where a [`DepRequest::Dispatch`] attaches — collapses the `_here` / `_in_frame` / `_with_chain` zoo.
pub enum DepPlacement<'a> {
    /// The slot's own `NodeScope` (`add_dispatch_here`) — binders' type sub-dispatches.
    OwnScope,
    /// A builtin-minted child scope (module/sig/recursive/using body), carried by reference. In a
    /// `AwaitDeps` a multi-statement body fans out one sub-dispatch per top-level statement
    /// (`split_body_statements` + `enter_block`); in a `Catch` a single watched expr enters a
    /// fresh lexical block (`enter_block`).
    InScope(&'a Scope<'a>),
}

/// The lexical block a [`Action::Tail`] enters — the block whose scope its `body_index` positions
/// and whose reshape the harness applies. The block scope is named one of two ways: projected from
/// the installed frame (`FrameScope`), or carried directly (`Overlay`) when the tail runs under an
/// inherited cart with no fresh frame to project from.
pub enum BlockEntry<'a> {
    /// No lexical block push; the tail continues in the slot's current block with the chain
    /// unchanged (EVAL, frameless continuations).
    None,
    /// The installed frame's own scope is the block; the run loop projects it from the frame at the
    /// tail-replace. Carries the scope id for the chain push / FN-body assembly (MATCH / TRY arms,
    /// FN-body tails).
    FrameScope(ScopeId),
    /// A caller-allocated overlay scope in a cart-ancestor region, entered without a fresh frame —
    /// the tail runs in it under the inherited call-site cart (USING). Carries the overlay so the
    /// harness fans the leading statements into it and installs it as the tail slot's scope.
    Overlay(&'a Scope<'a>),
}

/// The cart a `Tail` runs in.
pub enum FramePlacement<'a> {
    /// Reuse the slot's ping-pong reserve cart (`acquire_tail_frame(outer)`). The TCO tail-call
    /// frame — FN-body invoke, deferred `PerCall` tails. The only harness-constructed cart; the
    /// minted frame strong-owns no ancestor, so it carries no back-edge.
    ReuseReserve { outer: &'a Scope<'a> },
    /// A **pre-built** fresh cart the builtin minted (`CallFrame::new`, never the reserve), handed
    /// to the harness to install. The builtin owns construction because it may seed the cart before
    /// the tail dispatches — MATCH/TRY bind `it` into it via `CallFrame::with_scope`; EVAL builds it
    /// for the UAF guard.
    FreshChild { frame: Rc<CallFrame> },
    /// No new frame; continue in the slot's current cart. Frameless tails / `Done`.
    Inherit,
}
