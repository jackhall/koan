//! The scheduler-aware `Action` currency. The peer of
//! [`super::exec::ExecOutcome`]: where `ExecOutcome` is what `run_user_fn` returns (scheduler-
//! *unaware*), `Action` is what a builtin returns and what the harness interprets (scheduler-*aware*).
//! These are the **types only** — they reference core/model types, never the scheduler. The
//! interpreter that drives the scheduler from an `Action` lives one layer up in
//! `machine::execute::runtime::run_action` (the peer of `dispatch/exec.rs::invoke`).

use std::rc::Rc;

use super::body::ReturnContract;
use super::KFunction;
use crate::machine::core::{CallFrame, FrameStorage, LexicalFrame, Scope, StepAllocator};
use crate::machine::execute::StepCarried;
use crate::machine::model::Held;
use crate::machine::model::TypeRegistry;
use crate::machine::model::{Carried, KObject};
use crate::machine::model::{ExpressionPart, KExpression, TypeIdentifier};
use crate::machine::model::{KType, Record};
use crate::machine::{BindingIndex, DeliveredCarried, KError, KErrorKind, NodeId};
use crate::scheduler::DepResults;
#[cfg(test)]
use crate::witnessed::Witnessed;

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
/// owning `Rc` back to its frame — and upgrades for as long as the scope can run: a **producing**
/// scope during its own step (the producing node holds the frame); a **consumer/current** scope
/// during a step (the slot's cart — or a cart ancestor via the `FrameStorage.outer` chain, for a
/// `YokedChild` overlay scope — is held by the step machinery for the whole step); or the **run
/// root** (the run storage is held by the interpreter for the whole run). The single owner of this
/// invariant's assertion; step-scoped callers should route through `SchedulerView::dest_frame` or a
/// finish's `ctx.frame()` instead of upgrading directly.
pub fn scope_frame(scope: &Scope<'_>) -> Rc<FrameStorage> {
    scope.region_owner().upgrade().expect(
        "a scope's region owner is held while the scope can run: its cart (or a cart ancestor) for the step, the run storage for the run root",
    )
}

/// Read a builtin argument's `KObject` from a `BodyCtx::args` `KObject::Record` by name. `None` if
/// the args aren't a record or the named field is a type cell.
pub fn arg_object<'a, 'c>(args: &'c KObject<'a>, name: &str) -> Option<&'c KObject<'a>> {
    match args {
        KObject::Record(fields, _) => fields.get(name).and_then(Held::as_object),
        _ => None,
    }
}

/// Read a builtin argument's `KType` (a type-cell arg) from `BodyCtx::args` by name.
pub fn arg_type<'a, 'c>(args: &'c KObject<'a>, name: &str) -> Option<&'c KType> {
    match args {
        KObject::Record(fields, _) => fields.get(name).and_then(Held::as_type),
        _ => None,
    }
}

/// Read a builtin argument's unlowered type name (a [`Held::UnresolvedType`] cell) from
/// `BodyCtx::args` by name. The bind seam parks a bare user type name here rather than lowering
/// it to a type handle, so a type-slot consumer probes this before [`arg_type`] and resolves the
/// name against its own scope chain.
pub fn arg_unresolved_type<'a, 'c>(
    args: &'c KObject<'a>,
    name: &str,
) -> Option<&'c TypeIdentifier> {
    match args {
        KObject::Record(fields, _) => match fields.get(name) {
            Some(Held::UnresolvedType(ti)) => Some(ti),
            _ => None,
        },
        _ => None,
    }
}

/// Read a builtin argument's raw cell ([`Held::Object`] / [`Held::Type`] /
/// [`Held::UnresolvedType`]) from `BodyCtx::args` by
/// name — for builtins that branch on the value vs type channel (e.g. LET's name/value slots).
pub fn arg_held<'a, 'c>(args: &'c KObject<'a>, name: &str) -> Option<&'c Held<'a>> {
    match args {
        KObject::Record(fields, _) => fields.get(name),
        _ => None,
    }
}

/// Read a builtin argument's `KType` (a type-cell arg), or the canonical diagnostic —
/// `TypeMismatch{expected: "ProperType"}` for an object cell, `MissingArg` when absent.
pub fn require_ktype<'a>(args: &KObject<'a>, name: &str) -> Result<KType, KError> {
    match arg_held(args, name) {
        Some(Held::Type(kt)) => Ok(kt.clone()),
        Some(Held::Object(o)) => Err(KError::new(KErrorKind::TypeMismatch {
            arg: name.to_string(),
            expected: "ProperType".to_string(),
            got: o.ktype().name(),
        })),
        // Every slot reaching here is `OfKind(AnyType)`, which dispatch auto-wraps into a
        // resolved type carrier, so an unlowered name is not a shape this door serves.
        Some(Held::UnresolvedType(ti)) => Err(KError::new(KErrorKind::TypeMismatch {
            arg: name.to_string(),
            expected: "ProperType".to_string(),
            got: ti.render(),
        })),
        None => Err(KError::new(KErrorKind::MissingArg(name.to_string()))),
    }
}

/// Resolve the identifier-name in the `Identifier`-arm of arg `slot` — the binder name of a
/// value-defining builtin (MODULE) — or the canonical error: `MissingArg` for an absent slot,
/// `ShapeError` for any other value shape. `surface` is the keyword embedded in the diagnostic.
/// The value-channel twin of [`require_bare_type_name`]; an `Identifier` name part resolves to a
/// `KObject::KString` cell.
pub fn require_identifier_name<'a>(
    args: &KObject<'a>,
    slot: &str,
    surface: &str,
) -> Result<String, KError> {
    match arg_object(args, slot) {
        Some(KObject::KString(s)) => Ok(s.clone()),
        Some(other) => Err(KError::new(KErrorKind::ShapeError(format!(
            "{surface} {slot} must be a bare identifier, got `{}`",
            other.ktype().name(),
        )))),
        None => Err(KError::new(KErrorKind::MissingArg(slot.to_string()))),
    }
}

/// Resolve the bare type-name in the `Type`-arm of arg `slot` — the binder name of a
/// type-defining builtin (UNION / NEWTYPE / SIG / RECURSIVE) — or the canonical error:
/// `MissingArg` for an absent slot, `ShapeError` for a structural type. `surface` is the keyword
/// embedded in the diagnostic. The `Action`-side twin of
/// [`extract_bare_type_name`](super::argument_bundle::extract_bare_type_name).
pub fn require_bare_type_name<'a>(
    args: &KObject<'a>,
    slot: &str,
    surface: &str,
) -> Result<String, KError> {
    match arg_held(args, slot) {
        // A binder name is exactly the shape the bind seam leaves unlowered: a bare user type
        // name with nothing bound to it yet.
        Some(Held::UnresolvedType(ti)) => Ok(ti.render()),
        Some(Held::Type(t)) => bare_type_name(t, slot, surface),
        Some(Held::Object(_)) | None => Err(KError::new(KErrorKind::MissingArg(slot.to_string()))),
    }
}

/// Resolve a resolved `KType` to its bare type name, for the binders that read their name from a
/// `KObject::Record` type cell. A simple / nominal leaf yields its `name()`; a structural type
/// (List, Record, FN, …) is a `ShapeError`. `surface` is the keyword (`"NEWTYPE"`, `"UNION"`, …)
/// embedded in the message.
fn bare_type_name(t: &KType, name: &str, surface: &str) -> Result<String, KError> {
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
        | KType::Any
        | KType::SetRef { .. }
        | KType::Signature { .. }
        | KType::AbstractType { .. } => Ok(t.name()),
        KType::List { .. }
        | KType::Dict { .. }
        | KType::Record { .. }
        | KType::KFunction { .. }
        | KType::DeferredReturn(_)
        | KType::SetLocal(_)
        | KType::Union { .. }
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
/// the cart `Rc`* (so a body that seals a type operand can `Rc::clone` it), `None` for def-time
/// builtins. `chain` is `None` for a top-level binder (`bind_index` → `BindingIndex::BUILTIN`). `args` is the
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
    /// no entry — [`arg_carrier`](Self::arg_carrier) reads `None`, i.e. "no foreign reach". Each carrier
    /// is borrowed off the working expression's own splice cells (which outlive the call), never copied.
    pub arg_carriers: &'c Record<&'c DeliveredCarried>,
    /// The step construction allocator for this slot's own scope, branded at the step lifetime
    /// `'a`: its doors return a [`StepCarried`] that cannot outlive the step. The same allocator a
    /// wake-time [`FinishCtx`] carries.
    pub ctx: StepAllocator<'a>,
    /// The run's subtype-verdict registry, reached from the scheduler view at the call. A builtin
    /// body that runs a type predicate (ascription, MATCH arm selection, `==`) passes it down.
    pub types: &'c TypeRegistry,
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
    pub fn arg_carrier(&self, name: &str) -> Option<&'c DeliveredCarried> {
        self.arg_carriers.get(name).copied()
    }

    /// A [`FinishCtx`] over this body's own scope and context — for a synchronous body that hands its
    /// resolve/dispatch continuation the same shape a wake-time finish receives (e.g.
    /// `resolve_or_await`'s synchronous arm).
    pub fn finish_ctx(&self) -> FinishCtx<'a> {
        FinishCtx {
            scope: self.scope,
            ctx: self.ctx.clone(),
        }
    }
}

/// Wake-time context a finish receives: the slot's **own** scope (interior-mutable, with `.region`)
/// re-projected at wake — a deferred binder `register_*`s on it here — plus the step construction
/// context wrapping the frame storage owning that scope's region, resolved by the step machinery so
/// a finish allocates with no failure path (`ctx.region()` / `ctx.alloc()` / `ctx.alloc_with()`;
/// `design/scheduler-library.md` guarantees 3 and 5).
pub struct FinishCtx<'a> {
    pub scope: &'a Scope<'a>,
    pub ctx: StepAllocator<'a>,
}

impl<'a> FinishCtx<'a> {
    /// Build a `FinishCtx` from a scope alone, reconstructing the step context over the scope's own
    /// frame — for a synchronous site that holds a scope but no live step context (a resolve
    /// combinator's `Done` arm, a unit test). `scope_frame(scope)` names the same dest frame the
    /// harness step context wraps at wake, so both allocate in the same region. A site that already
    /// holds the live step context (a builtin body) uses [`BodyCtx::finish_ctx`] instead.
    pub fn for_scope(scope: &'a Scope<'a>) -> Self {
        FinishCtx {
            scope,
            ctx: StepAllocator::for_scope(scope),
        }
    }
}

/// A resolved dep terminal as a continuation receives it. `value` is the terminal re-anchored
/// **live at the step brand** (pinned by the step open) for a value-reading finish that reads it
/// directly (`resolve_or_await`, `fn_def`/`return_type`, dispatch constructors / literal);
/// `delivered` is the producer's own carrier bundled with its retained producer-frame owner as one
/// [`DeliveredCarried`] envelope — a [`duplicate`](crate::witnessed::Delivered::duplicate), so the
/// producer keeps its terminal for other consumers. A **construction finish** folds the dep
/// *witnessed* via the envelope's cell
/// ([`Delivered::transfer_into`](crate::witnessed::Delivered::transfer_into)), its reach named on the
/// result by construction; a finish that parks the carrier on the working expression across steps
/// (the working-copy splice) duplicates the whole envelope into the
/// [`Spliced`](crate::machine::model::ExpressionPart::Spliced) cell, keeping the value's backing
/// retained (its host = the scheduler's retention hold, `None` for a frameless / run-region producer
/// whose backing already outlives the terminal) through the `Replace` to the step that adopts it.
/// Defined here in core (not the execute layer that resolves it) so the builtin-`Action` currency —
/// [`AwaitContinue`] — can name it.
pub struct DepTerminal<'a> {
    pub value: Carried<'a>,
    pub delivered: DeliveredCarried,
}

/// A `AwaitDeps` finish: re-entered at wake with the resolved dep terminals as a [`DepResults`] view
/// (addressed by `park` / `owned` position) of un-relocated [`DepTerminal`]s — each carrying its
/// step-brand `value` and its own reach `carrier` — yielding another `Action` the harness recurses
/// into. Reads only a `FinishCtx`, never the scheduler — exec's continuation pattern.
pub type AwaitContinue<'a> =
    Box<dyn FnOnce(&FinishCtx<'a>, DepResults<'_, &DepTerminal<'a>>) -> Action<'a> + 'a>;

/// A `Catch` finish: re-entered with the watched slot's delivery envelope (value, reach, and
/// retained producer pin as one unit, adopted or opened at the finish's own step brand) or the
/// watched `KError`.
pub type CatchContinue<'a> =
    Box<dyn FnOnce(&FinishCtx<'a>, Result<DeliveredCarried, KError>) -> Action<'a> + 'a>;

/// The return contract a [`Action::Tail`] carries — eager, or resolved from the last leading
/// statement's result at finish time (a deferred-`Expression` FN return: the return-type
/// expression rides as the last leading statement, and the lowering's finish reads the resolved
/// type and homes it as a `PerCall` contract for `func`).
pub enum TailContract<'a> {
    Eager(Option<ReturnContract<'a>>),
    FromLastResult { func: &'a KFunction<'a> },
}

/// What happens next for a slot — the four shapes the builtin survey reduced everything to.
pub enum Action<'a> {
    /// Produce this slot's terminal (after any direct scope mutation the builtin did): a witnessed
    /// value or an error. The `Ok` carrier is built **inside the witness closure** — already bundled
    /// with the set of regions it reaches ([`yoke`](crate::witnessed::Witnessed::yoke) / `merge` at
    /// the alloc site, or a step-context `alloc_carried`/`alloc_carried_with` (and their typed
    /// wrappers) / `resident_type_carrier` sealing a constructed or read value) — so it is co-located
    /// by construction rather than paired with an asserted witness at finalize. The construction
    /// terminal for **both** channels: a builtin that allocates a `KObject` or a `KType` seals it here.
    /// The carrier rides the step brand `'a` from the door that built it (a [`StepCarried`]), so it
    /// cannot be stashed past the step; the sole exit to node storage is finalize's seal.
    Done(Result<StepCarried<'a>, KError>),
    /// Tail-replace into `tail`, carrying `contract` (see [`TailContract`]), in a cart per
    /// `frame_placement`. When `leading` (the body's non-tail statements) is non-empty the slot
    /// first parks on them as owned deps and tail-replaces only once they resolve — so they run,
    /// and cascade-free, before the tail continues. `block_entry` names the lexical block the tail
    /// enters (see [`BlockEntry`]); the harness derives the body-statement chains and the tail's
    /// `body_index` from it + `leading`.
    Tail {
        leading: Vec<KExpression<'a>>,
        tail: KExpression<'a>,
        contract: TailContract<'a>,
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
    /// alloc site (`alloc_carried`/`alloc_carried_with` / `yoke` / `merge` / `resident_*_carrier`), so
    /// this stays behind `cfg(test)`.
    pub(crate) fn done_resident(value: Carried<'a>) -> Self {
        Action::Done(Ok(StepCarried::born(Witnessed::resident(value))))
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

/// Where a [`DepRequest::Dispatch`] attaches.
pub enum DepPlacement<'a> {
    /// The slot's own `NodeScope` (`dispatch_in_own_scope`) — binders' type sub-dispatches.
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
    /// The installed frame's own scope is the block; the frame carries its own scope id
    /// (`frame.scope_id()`) for the chain push / FN-body assembly, and the lowering fans any
    /// leading statements into the frame itself (`BodyPlacement::Frame`) — MATCH / TRY arms,
    /// FN-body tails.
    FrameScope(Rc<CallFrame>),
    /// A caller-allocated overlay scope in a cart-ancestor region, entered without a fresh frame —
    /// the tail runs in it under the inherited call-site cart (USING). Carries the overlay so the
    /// harness fans the leading statements into it and installs it as the tail slot's scope.
    Overlay(&'a Scope<'a>),
}

/// The cart a `Tail` runs in.
pub enum FramePlacement<'a> {
    /// Mint a fresh cart at apply through the reserved `CallFrame::new_tail(outer)`. The TCO
    /// tail-call frame — FN-body invoke, deferred `PerCall` tails. The only harness-constructed cart;
    /// the retiring cart drops at the reinstall, and the library retires its region once the sealed
    /// argument carriers that pin it release their hold. `new_tail` is the one door that chains no
    /// ancestor pin, so the minted frame strong-owns no ancestor and carries no back-edge.
    FreshTail { outer: &'a Scope<'a> },
    /// A **pre-built** fresh cart the builtin minted (`CallFrame::new`), handed
    /// to the harness to install. The builtin owns construction because it may seed the cart before
    /// the tail dispatches — MATCH/TRY bind `it` into it via `CallFrame::with_scope`; EVAL builds it
    /// for the UAF guard.
    FreshChild { frame: Rc<CallFrame> },
    /// No new frame; continue in the slot's current cart. Frameless tails / `Done`.
    Inherit,
}
