//! The scheduler-aware `Action` currency. The peer of
//! [`super::exec::ExecOutcome`]: where `ExecOutcome` is what `run_user_fn` returns (scheduler-
//! *unaware*), `Action` is what a builtin returns and what the harness interprets (scheduler-*aware*).
//! These are the **types only** — they reference core/model types, never the scheduler. The
//! interpreter that drives the scheduler from an `Action` lives one layer up in
//! `machine::execute`'s `run_action` (the peer of `decide/exec.rs::invoke`).

use std::rc::Rc;

use super::body::ReturnContract;
use crate::machine::core::bindings::WriteOp;
use crate::machine::core::carrier_witness::{SealedFunction, SplicedCell};
use crate::machine::core::{
    CallFrame, FrameStorage, LexicalFrame, ProgramBrand, RegionBrand, RunWriter, Scope,
    StepAllocator,
};
use crate::machine::execute::StepCarried;
use crate::machine::model::BinderSymbol;
#[cfg(test)]
use crate::machine::model::Carried;
use crate::machine::model::Held;
use crate::machine::model::KObject;
use crate::machine::model::RunRegistries;
use crate::machine::model::Symbol;
use crate::machine::model::TypeRegistry;
use crate::machine::model::WorkingExpression;
use crate::machine::model::labels::TypeSymbol;
use crate::machine::model::{ExpressionPart, KExpression};
use crate::machine::model::{KType, TypeNode};
use crate::machine::model::{StaticName, ValueSymbol};
use crate::machine::{
    BindingIndex, DeclarationSite, DeliveredCarried, Installer, KError, KErrorKind,
};
use crate::scheduler::Deps;
use crate::source::SourceRef;

/// Unwrap a `Result<T, KError>` inside an `Action`-returning body, early-returning
/// `Action::done(Err(e))` on the error arm — the `Action`-body analogue of `?`. Collapses the
/// pervasive `match helper(…) { Ok(v) => v, Err(e) => return Action::done(Err(e)) }` envelope.
/// `#[macro_export]` hoists it to the crate root, so call it as `crate::try_action!(…)` from
/// anywhere with no import.
#[macro_export]
macro_rules! try_action {
    ($expr:expr) => {
        match $expr {
            Ok(value) => value,
            Err(error) => return $crate::machine::core::kfunction::action::Action::done(Err(error)),
        }
    };
}

/// The `Rc<FrameStorage>` that owns `scope`'s region — the witness a value built into that region is
/// `yoke`d under (the object-family construction inversion: a region-resident object is born bundled
/// with its frame as its reach). The link a scope derives its owner through is `Weak` — an in-region
/// value holds no owning `Rc` back to its frame — and upgrades for as long as the scope can run: a **producing**
/// scope during its own step (the producing node holds the frame); a **consumer/current** scope
/// during a step (the slot's cart — or a cart ancestor via the `FrameStorage.outer` chain, for a
/// `YokedChild` overlay scope — is held by the step machinery for the whole step); or the **run
/// root** (the run storage is held by the interpreter for the whole run). The single owner of this
/// invariant's assertion; step-scoped callers should route through `DecideCtx::dest_frame` or a
/// finish's `ctx.frame()` instead of upgrading directly.
pub fn scope_frame(scope: &Scope<'_>) -> Rc<FrameStorage> {
    scope.region_owner().upgrade().expect(
        "a scope's region owner is held while the scope can run: its cart (or a cart ancestor) for the step, the run storage for the run root",
    )
}

/// One bound argument slot: the resolved value and, when the argument arrived as a delivered
/// sub-result, its reach carrier. Positional — its meaning comes from the schema slot it sits at.
#[derive(Clone, Copy)]
pub struct BoundArg<'a, 'c> {
    pub value: Held<'a>,
    pub carrier: Option<&'c DeliveredCarried>,
}

/// A call's arguments as a **schema-keyed view**: the signature's own parameter schema paired with
/// a values-only slice on the step scratch, aligned slot-for-slot.
///
/// Nothing is re-keyed per call. The schema is built once at
/// [`ExpressionSignature::mint`](crate::machine::model::ExpressionSignature::mint) and shared by
/// every call to that signature; the slice holds only the bound values and their delivery
/// envelopes. A named read reads the symbol off the slot's declared name and scans the schema —
/// linear over call arity, no hash, no map, no allocation.
///
/// See [design/label-interning.md](../../../../design/label-interning.md).
#[derive(Clone, Copy)]
pub struct BoundArgs<'a, 'c> {
    schema: &'c [(BinderSymbol, KType)],
    slots: &'c [BoundArg<'a, 'c>],
}

impl<'a, 'c> BoundArgs<'a, 'c> {
    /// Pair a signature's schema with the call's filled slots. The two must be the same length —
    /// the binder fills one slot per parameter, in `part_slots` order.
    pub fn new(schema: &'c [(BinderSymbol, KType)], slots: &'c [BoundArg<'a, 'c>]) -> Self {
        debug_assert_eq!(
            schema.len(),
            slots.len(),
            "an argument view pairs one slot per declared parameter",
        );
        BoundArgs { schema, slots }
    }

    /// The empty view — a nullary call, and the shape a test fixture with no arguments takes.
    pub fn empty() -> Self {
        BoundArgs {
            schema: &[],
            slots: &[],
        }
    }

    fn slot(&self, name: &StaticName<ValueSymbol>) -> Option<&'c BoundArg<'a, 'c>> {
        let symbol = name.symbol().symbol();
        self.schema
            .iter()
            .position(|(candidate, _)| candidate.symbol() == symbol)
            .and_then(|at| self.slots.get(at))
    }

    /// The argument's raw cell ([`Held::Object`] / [`Held::Type`] / [`Held::UnresolvedType`]) — for
    /// a builtin that branches on the value vs type channel (e.g. LET's name/value slots).
    pub fn held(&self, name: &StaticName<ValueSymbol>) -> Option<&'c Held<'a>> {
        self.slot(name).map(|slot| &slot.value)
    }

    /// The argument's `KObject`. `None` if the named slot is a type cell.
    pub fn object(&self, name: &StaticName<ValueSymbol>) -> Option<&'c KObject<'a>> {
        self.held(name).and_then(Held::as_object)
    }

    /// The argument's `KType` — a type-cell argument.
    pub fn ktype(&self, name: &StaticName<ValueSymbol>) -> Option<KType> {
        self.held(name).and_then(Held::as_type)
    }

    /// The argument's captured name — the value-channel read of an `:Identifier` slot. The bind
    /// seam parks the token's symbol here rather than rendering it to a string, so a name-taking
    /// builtin reads the symbol the parse minted.
    pub fn identifier(&self, name: &StaticName<ValueSymbol>) -> Option<ValueSymbol> {
        match self.held(name) {
            Some(Held::Identifier(v)) => Some(*v),
            _ => None,
        }
    }

    /// The argument's unlowered type name. The bind seam parks a bare user type name here rather
    /// than lowering it to a handle, so a type-slot consumer probes this before [`Self::ktype`] and
    /// resolves the name against its own scope chain.
    pub fn unresolved_type(&self, name: &StaticName<ValueSymbol>) -> Option<TypeSymbol> {
        match self.held(name) {
            Some(Held::UnresolvedType(ti)) => Some(*ti),
            _ => None,
        }
    }

    /// The argument's reach carrier — `Some` when it arrived as a resolved value (so a
    /// value-embedding body can fold / merge it), `None` for a region-pure scalar literal.
    pub fn carrier(&self, name: &StaticName<ValueSymbol>) -> Option<&'c DeliveredCarried> {
        self.slot(name).and_then(|slot| slot.carrier)
    }

    /// Every argument in declaration order, as `(symbol, cell)`.
    pub fn iter(&self) -> impl Iterator<Item = (Symbol, &'c Held<'a>)> {
        self.schema
            .iter()
            .zip(self.slots)
            .map(|((name, _), slot)| (name.symbol(), &slot.value))
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }
}

/// Read a builtin argument's `KType` (a type-cell arg), or the canonical diagnostic —
/// `TypeMismatch{expected: "ProperType"}` for an object cell, `MissingArg` when absent.
pub fn require_ktype<'a>(
    args: BoundArgs<'a, '_>,
    name: &StaticName<ValueSymbol>,
    registries: &RunRegistries,
) -> Result<KType, KError> {
    match args.held(name) {
        Some(Held::Type(kt)) => Ok(*kt),
        Some(Held::Object(o)) => Err(KError::new(KErrorKind::TypeMismatch {
            arg: name.text().to_string(),
            expected: "ProperType".to_string(),
            got: o.ktype().name(registries),
        })),
        // Every slot reaching here is `OfKind(AnyType)`, which dispatch auto-wraps into a
        // resolved type carrier, so an unlowered name is not a shape this door serves.
        Some(Held::UnresolvedType(ti)) => Err(KError::new(KErrorKind::TypeMismatch {
            arg: name.text().to_string(),
            expected: "ProperType".to_string(),
            got: crate::machine::model::types::render_label(ti.symbol(), registries),
        })),
        // Every slot reaching here is a type slot, which admits no raw name part.
        Some(Held::Identifier(_)) => {
            unreachable!("a type slot never captures an identifier")
        }
        None => Err(KError::new(KErrorKind::MissingArg(name.text().to_string()))),
    }
}

/// Resolve the identifier-name in the `Identifier`-arm of arg `slot` — the binder name of a
/// value-defining builtin (MODULE) — or the canonical error: `MissingArg` for an absent slot,
/// `ShapeError` for any other value shape. `surface` is the keyword embedded in the diagnostic.
/// The value-channel twin of [`require_bare_type_name`]; both hand back the symbol their slot
/// captured, which the parse minted when it classified the token.
pub fn require_identifier_name<'a>(
    args: BoundArgs<'a, '_>,
    slot: &StaticName<ValueSymbol>,
    surface: &str,
    registries: &RunRegistries,
) -> Result<ValueSymbol, KError> {
    let spelling = slot.text();
    match args.held(slot) {
        Some(Held::Identifier(v)) => Ok(*v),
        Some(other) => Err(KError::new(KErrorKind::ShapeError(format!(
            "{surface} {spelling} must be a bare identifier, got `{}`",
            other.ktype(&registries.types).name(registries),
        )))),
        None => Err(KError::new(KErrorKind::MissingArg(spelling.to_string()))),
    }
}

/// Resolve the bare type-name in the `Type`-arm of arg `slot` — the binder name of a
/// type-defining builtin (UNION / NEWTYPE / SIG / RECURSIVE) — or the canonical error:
/// `MissingArg` for an absent slot, `ShapeError` for a structural type. `surface` is the keyword
/// embedded in the diagnostic. The `Action`-side twin of
/// [`extract_bare_type_name`](super::argument_bundle::extract_bare_type_name).
pub fn require_bare_type_name<'a>(
    args: BoundArgs<'a, '_>,
    slot: &StaticName<ValueSymbol>,
    surface: &str,
    registries: &RunRegistries,
) -> Result<TypeSymbol, KError> {
    match args.held(slot) {
        // A binder name is exactly the shape the bind seam leaves unlowered: a bare user type
        // name with nothing bound to it yet, already classified and interned by the parser.
        Some(Held::UnresolvedType(ti)) => Ok(*ti),
        // A resolved leaf handle reaches its name only as rendered text, so this arm re-declares
        // it — the one seam where a builtin's name is minted from a string rather than a token.
        Some(Held::Type(t)) => {
            let name = bare_type_name(*t, slot.text(), surface, registries)?;
            TypeSymbol::declared(&name, &registries.labels).ok_or_else(|| {
                KError::new(KErrorKind::ShapeError(format!(
                    "{surface} name must be a bare type name, got `{name}`",
                )))
            })
        }
        // A type-name slot is `PROPER_TYPE` / `ANY_TYPE`, which admits no raw value-name part.
        Some(Held::Identifier(_)) => unreachable!("a type-name slot never captures an identifier"),
        Some(Held::Object(_)) | None => {
            Err(KError::new(KErrorKind::MissingArg(slot.text().to_string())))
        }
    }
}

/// Resolve a resolved `KType` to its bare type name, for the binders that read their name from a
/// `KObject::Record` type cell. A simple / nominal leaf yields its `name()`; a structural type
/// (List, Record, FN, …) is a `ShapeError`. `surface` is the keyword (`"NEWTYPE"`, `"UNION"`, …)
/// embedded in the message.
fn bare_type_name(
    t: KType,
    name: &str,
    surface: &str,
    registries: &RunRegistries,
) -> Result<String, KError> {
    let types = &registries.types;
    match types.node(t) {
        TypeNode::Number
        | TypeNode::Str
        | TypeNode::Bool
        | TypeNode::Null
        | TypeNode::Identifier
        | TypeNode::KExpression
        | TypeNode::SigiledTypeExpr
        | TypeNode::RecordType
        | TypeNode::OfKind(_)
        | TypeNode::Any
        | TypeNode::SetMember { .. }
        | TypeNode::Signature { .. }
        | TypeNode::AbstractType { .. } => Ok(t.name(registries)),
        TypeNode::List { .. }
        | TypeNode::Dict { .. }
        | TypeNode::Record { .. }
        | TypeNode::KFunction { .. }
        | TypeNode::DeferredReturn(_)
        | TypeNode::Sibling(_)
        | TypeNode::Union { .. }
        | TypeNode::ConstructorApply { .. } => Err(KError::new(KErrorKind::ShapeError(format!(
            "{surface} {name} must be a bare type name, got `{}`",
            t.render(registries),
        )))),
    }
}

/// Read the `KExpression` in arg `slot`, or the canonical parenthesized-slot
/// `ShapeError` (`"<builtin> <slot> slot must be a parenthesized expression"`), owning that error
/// text so every `KExpression`-slot builtin reports it identically.
pub fn require_kexpression<'a>(
    args: BoundArgs<'a, '_>,
    builtin: &str,
    slot: &StaticName<ValueSymbol>,
) -> Result<KExpression<'a>, KError> {
    let spelling = slot.text();
    match args.object(slot) {
        Some(KObject::KExpression(e)) => Ok(e.node()),
        _ => Err(KError::new(KErrorKind::ShapeError(format!(
            "{builtin} {spelling} slot must be a parenthesized expression"
        )))),
    }
}

/// A builtin body: `fn(&BodyCtx) -> Action`. The builtin mutates `BodyCtx.scope` directly (binding
/// install is a scope write, not an `Action` effect) and returns an `Action` describing the
/// scheduler continuation.
pub type ActionFn = for<'a> fn(&BodyCtx<'_, 'a, '_>) -> Action<'a>;

/// Read-only-ish context a builtin body receives. `scope` is **interior-mutable**: the builtin
/// binds / registers / allocs on it directly before returning a `Action`. `frame` is a *reference to
/// the cart `Rc`* (so a body that seals a type operand can `Rc::clone` it), `None` for def-time
/// builtins. `chain` is `None` for a top-level binder (`bind_index` → `BindingIndex::BUILTIN`). `args`
/// is the builtin's bound arguments as a transient owned record, borrowed for the call — never a
/// is the schema-keyed argument view — never a `KObject`, never region-allocated; unevaluated args
/// ride as `KObject::KExpression` cells.
pub struct BodyCtx<'program: 'a, 'a, 'c> {
    pub scope: &'a Scope<'a>,
    pub frame: Option<&'c Rc<CallFrame>>,
    /// The ambient lexical chain (an `Rc`, as `active_chain` hands it out — binders read
    /// its `index` for `BindingIndex`, MATCH passes it to `resolve_type_identifier`). `None` at top level.
    pub chain: Option<Rc<LexicalFrame>>,
    /// The call's arguments as a schema-keyed view: the signature's own parameter schema paired
    /// with a values-only slice on the step scratch. Each slot carries the bound cell and, when the
    /// argument arrived as a resolved value (a spliced sub-result or a bound-name read), its
    /// [`Sealed`] reach carrier naming every region that value reaches. A value-embedding body
    /// folds the carrier of the value it deposits (a bind into the scope reach-set) or `merge`s the
    /// one it embeds (a `Wrapped` / re-tagged `Record`), so the result names that reach by
    /// construction; a region-pure scalar literal has no carrier, which reads as "no foreign
    /// reach". Every carrier is borrowed off the working expression's own splice cells (which
    /// outlive the call), never copied.
    pub args: BoundArgs<'a, 'c>,
    /// The statement running this body, as its installing declaration's identity. A type binder
    /// threads it into the `types` entry through [`Self::declaration_site`]; value-side binders
    /// (LET etc.) read only [`Self::bind_index`].
    pub installer: Installer,
    /// The step construction allocator for this slot's own scope, branded at the step lifetime
    /// `'a`: its doors return a [`StepCarried`] that cannot outlive the step. The same allocator a
    /// wake-time [`FinishCtx`] carries.
    pub ctx: StepAllocator<'a>,
    /// The run's lookup state — the type registry and the label interner — borrowed from the
    /// scheduler view at the call. A body that runs a type predicate (ascription, MATCH arm
    /// selection, `==`) reaches the registry through [`Self::types`]; a body that builds a record
    /// or renders a label interns and resolves through `registries.labels`. The registries are
    /// owned by the run frame and outlive the call, so the body forwards the borrow rather than
    /// sharing ownership.
    pub registries: &'c RunRegistries,
    /// The run's output sink, borrowed from the scheduler view at the call — the same channel and
    /// the same run-frame owner as [`Self::registries`]. A body that emits program output writes
    /// here.
    pub out: &'c RunWriter,
    /// The run's program storage allocation capability, threaded down from the scheduler view. A
    /// body that has to synthesize a node reaching the **value channel** builds it through this,
    /// since the marker those arms carry is mintable only here. Everything a body builds merely to
    /// dispatch takes [`Self::brand`] instead.
    ///
    /// Minted once per run and carried at its own `'program`, related to the step lifetime only by
    /// the struct's `'program: 'a` bound: a door reached through this brand pins its parts at
    /// program storage, so a step-allocated part cannot reach one.
    pub program: ProgramBrand<'program>,
}

impl<'program: 'a, 'a, 'c> BodyCtx<'program, 'a, 'c> {
    /// The run's type registry — the currency for pure type-structure questions.
    pub fn types(&self) -> &'c TypeRegistry {
        &self.registries.types
    }

    /// The lexical position a binding the builtin installs takes: the ambient chain head's index —
    /// this step's own statement position in its block — or [`BindingIndex::BUILTIN`] when there is
    /// no chain (a top-level / direct-body binder, e.g. a test fixture that bypasses the
    /// scheduler). The dispatch-time placeholder stamp reads the same head, so a claim and the
    /// write that finalizes it agree.
    pub fn bind_index(&self) -> BindingIndex {
        self.chain
            .as_deref()
            .map_or(BindingIndex::BUILTIN, |chain| {
                BindingIndex::value(chain.index)
            })
    }

    /// The installing declaration's identity: this body's statement ([`Self::installer`]) paired
    /// with its lexical position ([`Self::bind_index`]). A type binder threads this into its
    /// `types` entry so a same-declaration check compares the installing statement, not a lexical
    /// position that a driver can hand to two distinct statements — e.g. a persistent scope's
    /// separate runs, each numbering from 1 again.
    pub fn declaration_site(&self) -> DeclarationSite {
        DeclarationSite {
            installer: self.installer,
            index: self.bind_index(),
        }
    }

    /// The allocation capability for this body's own scope region, branded at the step lifetime
    /// `'a` — what a body bumping text or freezing a parts run allocates through.
    pub fn brand(&self) -> RegionBrand<'a> {
        self.scope.brand()
    }

    /// The scheduler-side working copy of a body this builtin holds as raw AST: the AST → scheduler
    /// crossing, made at the moment the body declares the dep that runs the node. One slice copy of
    /// the parts run into this body's own region, never a rebuild.
    pub fn working(&self, ast: KExpression<'a>) -> WorkingExpression<'a> {
        WorkingExpression::from_ast(self.brand(), ast)
    }

    /// A [`FinishCtx`] over this body's own scope and context — for a synchronous body that hands its
    /// resolve/dispatch continuation the same shape a wake-time finish receives (e.g.
    /// `resolve_or_await`'s synchronous arm).
    pub fn finish_ctx(&self) -> FinishCtx<'a, 'c> {
        FinishCtx {
            scope: self.scope,
            ctx: self.ctx.clone(),
            registries: self.registries,
        }
    }
}

/// Wake-time context a finish receives: the slot's **own** scope (interior-mutable, with `.region`)
/// re-projected at wake — a deferred binder `register_*`s on it here — plus the step construction
/// context wrapping the frame storage owning that scope's region, resolved by the step machinery so
/// a finish allocates with no failure path (`ctx.region()` / `ctx.alloc()` / `ctx.alloc_with()`;
/// `design/scheduler-library.md` guarantees 3 and 5).
pub struct FinishCtx<'a, 'r> {
    pub scope: &'a Scope<'a>,
    pub ctx: StepAllocator<'a>,
    /// The run's lookup state, mirroring [`BodyCtx::registries`] so a wake-time finish runs the
    /// same type predicates and resolves the same labels a synchronous body does. Borrowed for the
    /// duration of the finish call: the site building this context holds the registries and
    /// consumes the context as a short `&FinishCtx`, so `'r` is independent of the step brand `'a`.
    pub registries: &'r RunRegistries,
}

impl<'a, 'r> FinishCtx<'a, 'r> {
    /// Build a `FinishCtx` from a scope alone, reconstructing the step context over the scope's own
    /// frame — for a synchronous site that holds a scope but no live step context (a resolve
    /// combinator's `Done` arm, a unit test). `scope_frame(scope)` names the same dest frame the
    /// harness step context wraps at wake, so both allocate in the same region. A site that already
    /// holds the live step context (a builtin body) uses [`BodyCtx::finish_ctx`] instead.
    pub fn for_scope(scope: &'a Scope<'a>, registries: &'r RunRegistries) -> Self {
        FinishCtx {
            scope,
            ctx: StepAllocator::for_scope(scope),
            registries,
        }
    }

    /// The run's type registry — the currency for pure type-structure questions.
    pub fn types(&self) -> &'r TypeRegistry {
        &self.registries.types
    }

    /// The allocation capability for this finish's own scope region, branded at the step lifetime
    /// `'a`. The wake-time peer of [`BodyCtx::brand`].
    pub fn brand(&self) -> RegionBrand<'a> {
        self.scope.brand()
    }

    /// The scheduler-side working copy of raw AST this finish holds — the wake-time peer of
    /// [`BodyCtx::working`].
    pub fn working(&self, ast: KExpression<'a>) -> WorkingExpression<'a> {
        WorkingExpression::from_ast(self.brand(), ast)
    }
}

/// A resolved dep terminal as a continuation receives it: the delivered value **already resident in
/// the region this step reads it from**, as a [`SplicedCell`] re-branded once at step start under
/// the step's own coverage. There is no envelope and no per-dep pin: the producer's finalize walk
/// adopted the value into this edge's destination — the region the dep's source named, which for
/// spawned sub-work is the consumer's own — and it is covered for the step's whole life by the
/// slot anchor's owner chain.
///
/// So a value-reading finish (`resolve_or_await`, `fn_def`/`return_type`, dispatch constructors /
/// literal) opens the cell at its own borrow ([`Sealed::open_at`](crate::witnessed::Sealed::open_at))
/// with no pin to thread; a **construction finish** that folds the dep into a longer-lived result
/// lifts it back to an envelope first ([`Scope::lift_spliced`](crate::machine::core::Scope::lift_spliced)),
/// which owns the reach the fold composes; and a finish that parks the carrier on the working
/// expression across steps ([`Spliced`](WorkingPart::Spliced)) rests it into the finishing step's
/// own region.
///
/// `'b` is the step's read borrow, not the value's home: the cell is `Copy` data whose pointee lives
/// one level down, in the destination region. Defined here in core (not the execute layer that
/// resolves it) so the builtin-`Action` currency — [`AwaitContinue`] — can name it.
#[derive(Clone, Copy)]
pub struct DepTerminal<'b> {
    pub cell: SplicedCell<'b>,
}

/// A `AwaitDeps` finish: re-entered at wake with the resolved [`DepTerminal`]s — each a resident cell
/// of a region this step already covers — as one slice **in dep order**, yielding another `Action`
/// the harness recurses into. Reads only a `FinishCtx`, never the scheduler — exec's continuation
/// pattern.
///
/// Higher-ranked in the dep brand `'d` as well as the ctx borrow: the residents are branded against
/// the step's coverage at step start, and a stored finish must accept whatever borrow that is.
pub type AwaitContinue<'a> =
    Box<dyn for<'r, 'd> FnOnce(&FinishCtx<'a, 'r>, &[DepTerminal<'d>]) -> Action<'a> + 'a>;

/// A `Catch` finish: re-entered with the watched slot's delivery envelope (value, reach, and
/// retained producer pin as one unit, adopted or opened at the finish's own step brand) or the
/// watched `KError`.
pub type CatchContinue<'a> = Box<
    dyn for<'r> FnOnce(&FinishCtx<'a, 'r>, Result<DeliveredCarried, KError>) -> Action<'a> + 'a,
>;

/// The return contract a [`Action::Tail`] carries — eager, or resolved from the last leading
/// statement's result at finish time (a deferred-`Expression` FN return: the return-type
/// expression rides as the last leading statement, and the lowering's finish reads the resolved
/// type and homes it as a `PerCall` contract for `func`).
pub enum TailContract<'a> {
    Eager(Option<ReturnContract<'a>>),
    FromLastResult {
        func: SealedFunction<'a>,
        site: Option<SourceRef>,
    },
}

/// What a builtin body (or a wake-time finish) returns: the binding-table writes it decided on,
/// plus what happens next for the slot.
///
/// `effects` is the outcome-ops channel — a body never mutates a published scope's binding tables
/// itself. It seals its value through a `seal_*` construction door and describes the write as a
/// [`WriteOp`]; the run loop drains the step's effects after the continuation returns and applies
/// them in program order, before finalize. An apply error becomes the node's error terminal, so
/// per-step writes are all-or-nothing.
pub struct Action<'a> {
    pub(crate) effects: Vec<WriteOp<'a>>,
    pub next: ActionKind<'a>,
}

impl<'a> Action<'a> {
    /// An `Action` writing nothing.
    pub fn from_kind(next: ActionKind<'a>) -> Self {
        Action {
            effects: Vec::new(),
            next,
        }
    }

    /// Produce this slot's terminal. See [`ActionKind::Done`].
    pub fn done(result: Result<StepCarried<'a>, KError>) -> Self {
        Action::from_kind(ActionKind::Done(result))
    }

    /// Tail-replace into `tail`. See [`ActionKind::Tail`].
    pub fn tail(
        leading: Vec<WorkingExpression<'a>>,
        tail: WorkingExpression<'a>,
        contract: TailContract<'a>,
        frame_placement: FramePlacement,
        block_entry: BlockEntry<'a>,
    ) -> Self {
        Action::from_kind(ActionKind::Tail {
            leading,
            tail,
            contract,
            frame_placement,
            block_entry,
        })
    }

    /// Dispatch `deps`, then continue through `finish`. See [`ActionKind::AwaitDeps`].
    pub fn await_deps(deps: Deps<SubDispatch<'a>>, finish: AwaitContinue<'a>) -> Self {
        Action::from_kind(ActionKind::AwaitDeps { deps, finish })
    }

    /// Fan `block` out to one dep per statement, then continue through `finish`. See
    /// [`ActionKind::AwaitBlock`].
    pub fn await_block(block: BlockRequest<'a>, finish: AwaitContinue<'a>) -> Self {
        Action::from_kind(ActionKind::AwaitBlock { block, finish })
    }

    /// Watch `watched`, recover via `finish`. See [`ActionKind::Catch`].
    pub fn catch(watched: DepRequest<'a>, finish: CatchContinue<'a>) -> Self {
        Action::from_kind(ActionKind::Catch { watched, finish })
    }

    /// A `Done` terminal paired with the binding-table writes the body decided, or the error
    /// terminal — the shape every binder's finalize helper returns.
    pub(crate) fn done_writing(
        result: Result<(StepCarried<'a>, Vec<WriteOp<'a>>), KError>,
    ) -> Self {
        match result {
            Ok((carrier, effects)) => Action::done(Ok(carrier)).with_effects(effects),
            Err(error) => Action::done(Err(error)),
        }
    }

    /// Attach the binding-table writes this step decided on, in program order.
    pub(crate) fn with_effects(mut self, effects: Vec<WriteOp<'a>>) -> Self {
        self.effects.extend(effects);
        self
    }

    /// Attach one binding-table write.
    pub(crate) fn with_effect(self, effect: WriteOp<'a>) -> Self {
        self.with_effects(vec![effect])
    }
}

/// What happens next for a slot — the four shapes the builtin survey reduced everything to.
pub enum ActionKind<'a> {
    /// Produce this slot's terminal (after any direct scope mutation the builtin did): a witnessed
    /// value or an error. The `Ok` carrier is built **inside the witness closure** — already bundled
    /// with the set of regions it reaches ([`yoke`](crate::witnessed::Witnessed::yoke) / `merge` at
    /// the alloc site, or a step-context `alloc_carried`/`alloc_carried_with` (and their typed
    /// wrappers) / `Scope::resident` sealing a constructed or read value) — so it is co-located
    /// by construction rather than paired with an asserted witness at finalize. The construction
    /// terminal for **both** channels: a builtin that allocates a `KObject` or a `KType` seals it here.
    /// The carrier rides the step brand `'a` from the door that built it (a [`StepCarried`]), so it
    /// cannot be stashed past the step; reaching node storage means going through finalize's seal.
    Done(Result<StepCarried<'a>, KError>),
    /// Tail-replace into `tail`, carrying `contract` (see [`TailContract`]), in a cart per
    /// `frame_placement`. When `leading` (the body's non-tail statements) is non-empty the slot
    /// first waits on them as deps and tail-replaces only once they resolve — so they run,
    /// and reclaim, before the tail continues. `block_entry` names the lexical block the tail
    /// enters (see [`BlockEntry`]); the harness derives the body-statement chains and the tail's
    /// `body_index` from it + `leading`.
    Tail {
        leading: Vec<WorkingExpression<'a>>,
        tail: WorkingExpression<'a>,
        contract: TailContract<'a>,
        frame_placement: FramePlacement,
        block_entry: BlockEntry<'a>,
    },
    /// Dispatch `deps`, then `finish` over their resolved values yields the next `Action`. Every
    /// entry contributes exactly one dep, so a builtin that recorded a [`Deps::request`] index reads
    /// its result back at that position.
    AwaitDeps {
        deps: Deps<SubDispatch<'a>>,
        finish: AwaitContinue<'a>,
    },
    /// Fan a statement block out to one dep per statement, then `finish` over their resolved values
    /// yields the next `Action`. The block currency is separate from `AwaitDeps` precisely because
    /// its dep count is not known at declaration time — see [`BlockRequest`].
    AwaitBlock {
        block: BlockRequest<'a>,
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
    /// marker object that references no foreign region ([`Scope::resident`] mints the description
    /// hosting it in `scope`'s own region with no members). Production never mints a bare terminal:
    /// a real value is always built witnessed at its alloc site (`alloc_carried`/`alloc_carried_with`
    /// / `yoke` / `merge` / `resident_*_carrier`), so this stays behind `cfg(test)`.
    pub(crate) fn done_resident(scope: &Scope<'a>, value: Carried<'a>) -> Self {
        Action::done(Ok(StepCarried::born(scope.resident(value))))
    }
}

/// The sub-work shape a builtin declares in an [`Action::AwaitDeps`]: a sub-expression the harness
/// dispatches on the consumer's behalf, whose slot reclaims at its own finalize. It is the request
/// arm of the [`Deps`](crate::scheduler::Deps) builder — a producer the builtin merely reads in goes
/// in through the other arm, by the edge naming it, so the two are told apart by construction rather
/// than by a role field.
pub struct SubDispatch<'a> {
    pub expr: WorkingExpression<'a>,
    pub placement: DepPlacement<'a>,
}

impl<'a> SubDispatch<'a> {
    /// Lower into the library dep currency — the crossing the harness (and
    /// the field-list bundle's Outcome finish) makes right before `Await::on`.
    pub fn into_request(self) -> DepRequest<'a> {
        // A builtin-declared sub-dispatch (module/sig/recursive/using bodies) enters a fresh
        // block via `InScope`, which is a statement position.
        DepRequest::Dispatch {
            expr: self.expr,
            placement: self.placement,
        }
    }
}

/// The dependency currency a dispatch [`Outcome::Park`](crate::machine::execute) declares
/// and a [`Action::Catch`] carries for its single watched dep — defined here in core so `Action` can
/// carry it without core depending on the execute layer.
///
/// **Every `DepRequest` realizes to exactly one producer**, hence one dep. That is what lets a
/// caller bank a [`Deps::request`] index and read its result back at that position; work whose dep
/// count is only known once the statements are split goes through [`BlockRequest`] instead, which is
/// not a dep-list entry at all.
///
/// The builtin `AwaitDeps` currency does not flow through `DepRequest`: its request entries are
/// [`SubDispatch`]es, which lower to one of these. `DepRequest`'s roles are `Catch`'s single
/// `watched` dep (a `Dispatch` of the watched sub-expression) and the dispatcher-side `Outcome`
/// currency: `Dispatch` staged subs, and the `ListLit` / `DictLit` / `RecordLit` literal lowerings
/// that schedule an aggregate literal as one producer.
pub enum DepRequest<'a> {
    /// One sub-expression, one producer. `OwnScope` re-dispatches against the slot's own scope;
    /// `InScope` enters a fresh **single-statement** block (so an inner `LET` stays local).
    Dispatch {
        expr: WorkingExpression<'a>,
        placement: DepPlacement<'a>,
    },
    ListLit(&'a [ExpressionPart<'a>]),
    DictLit(&'a [(ExpressionPart<'a>, ExpressionPart<'a>)]),
    RecordLit(&'a [(BinderSymbol, ExpressionPart<'a>)]),
}

/// A statement block to fan out — **one producer, and so one dep, per statement**, in declaration
/// order. The count is a property of the split, not of the request, so this is deliberately *not* a
/// [`DepRequest`]: it never joins a dep list, and the harness realizes it through its own door
/// ([`Host::block_sources`](crate::machine::execute)) rather than the per-entry one. A finish behind
/// a block reads its results in order and never by a banked index.
pub enum BlockRequest<'a> {
    /// A body's non-tail statements, already split. `placement` picks where they bind (see
    /// [`BodyPlacement`]): a deferred-return FN's first-call body and a leading-carrying arm bind
    /// into a fresh per-call frame's own scope; a leading-carrying USING binds into an
    /// inherited-cart overlay.
    Body {
        statements: Vec<WorkingExpression<'a>>,
        placement: BodyPlacement<'a>,
    },
    /// A body expression dispatched against `scope`, split into its top-level statements there — a
    /// declaration builtin's child-scope body (MODULE, SIG).
    InScope {
        body: WorkingExpression<'a>,
        scope: &'a Scope<'a>,
    },
}

/// Where a [`BlockRequest::Body`]'s statements bind — the two block fan-outs a leading-carrying
/// tail chooses between.
pub enum BodyPlacement<'a> {
    /// Dispatch as body-chain siblings in `frame`'s own scope (`KoanRuntime::dispatch_body`) — a
    /// deferred-return FN's first-call body (its non-tail body + the return-type expression) and
    /// MATCH / TRY arm leading statements.
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
    /// A builtin-minted child scope, carried by reference: the expression enters a fresh
    /// **single-statement** lexical block there (`enter_block`), so an inner `LET` stays local. A
    /// body that must split across statements is a [`BlockRequest::InScope`], not this.
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
pub enum FramePlacement {
    /// The TCO tail-call cart — FN-body invoke, deferred `PerCall` tails — minted by the decide
    /// through `CallFrame::new(outer)`, where `outer` is the callee closure's captured (definition)
    /// scope, so the fresh cart chains that scope's region owner and a closure's captured per-call
    /// frame survives the hop. (A top-level-defined recursive fn captures the run-root scope and so
    /// chains nothing; see [`CallFrame::new`].)
    ///
    /// Carrying the built cart rather than the scope to build it from is what puts the callee's
    /// argument bind in the step that *emits* the replace: the arguments are relocated into this
    /// region while the retiring one is still the deciding step's own, so the retiring cart drops at
    /// the reinstall with nothing left reading it. Distinct from [`FreshChild`](Self::FreshChild),
    /// which installs a cart the same way but does not retire the slot's current scope.
    FreshTail { frame: Rc<CallFrame> },
    /// A **pre-built** fresh cart the builtin minted (`CallFrame::new`), handed
    /// to the harness to install. The builtin owns construction because it may seed the cart before
    /// the tail dispatches — MATCH/TRY bind `it` into it via `CallFrame::with_scope`; EVAL builds it
    /// for the UAF guard.
    FreshChild { frame: Rc<CallFrame> },
    /// No new frame; continue in the slot's current cart. Frameless tails / `Done`.
    Inherit,
}

impl FramePlacement {
    /// The cart a tail-replace installs: the decide-minted cart (a TCO tail-call cart or a
    /// builtin's fresh child), or `None` to keep the current one. Both fresh placements arrive
    /// already built — the deciding step is where a callee's arguments are relocated into the new
    /// cart, so it holds the cart itself.
    pub fn fresh_frame(self) -> Option<Rc<CallFrame>> {
        match self {
            FramePlacement::FreshTail { frame } | FramePlacement::FreshChild { frame } => {
                Some(frame)
            }
            FramePlacement::Inherit => None,
        }
    }
}
