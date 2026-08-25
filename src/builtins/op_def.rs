//! `OP #(<sym>) OVER <Operand> = (<body>)` — declare a chainable operator in the enclosing
//! scope. The symbol is **quoted**: `#(+)` is a parse-static
//! [`QuotedExpression`](crate::machine::model::ExpressionPart::QuotedExpression) part, so it
//! rides an ordinary `:KExpression` slot and `OP` keeps a fixed untyped key — the dispatch
//! classifier knows nothing about operator declarations.
//!
//! Each declaration writes two places:
//!
//! - the enclosing scope's **function bucket**, under the key a use site computes — `[Slot,
//!   Keyword(sym), Slot]` for a binary operator, `[Keyword(sym), Slot]` for a unary one (plus a
//!   synthesized binary *bridge*, since a two-operand run `a ~ b` names one keyword and so
//!   dispatches as a plain keyworded call, not an operator chain);
//! - the enclosing scope's **operator registry**, a size-1 group `sym → FoldLeft` (binary) /
//!   `sym → Unary` (unary), so a run of three or more operands reduces. Inside a `GROUP` body the
//!   registry write is skipped: the group is the sole registrar for its members.
//!
//! Registration goes through [`Scope::register_operator_function`], the door without the
//! builtin-shadowing guard. Shadowing is type-gated rather than forbidden: `OP #(+) OVER Number`
//! registers, but dispatch consults the immutable root bucket first, so the builtin `+` still wins
//! for `Number` operands. A module declaring `+` over its own operand type reduces its own runs and
//! leaves arithmetic alone.
//!
//! An operator body captures its declaring scope, so it sees its sibling module bindings exactly as
//! a bare `FN` body does, and an `OP` statement evaluates to the function it declares.
//!
//! Surface design: [design/operators.md](../../design/operators.md).

use crate::machine::WriteGate;
use crate::machine::core::RegionBrand;
use crate::machine::execute::extend_deps_on;
use crate::scheduler::Deps;

use crate::machine::BindingIndex;
use crate::machine::KFunction;
use crate::machine::StepCarried;
use crate::machine::core::ProgramBrand;
use crate::machine::core::bindings::SealedValue;
use crate::machine::core::bindings::{WriteOp, powerset_probes};
use crate::machine::model::CarriedFamily;
use crate::machine::model::KType;
use crate::machine::model::labels::{KeywordSymbol, LabelInterner, TypeSymbol};
use crate::machine::model::{ExpressionPart, KExpression};
use crate::machine::model::{KKind, SignatureDraft, SignatureElement};
use crate::machine::model::{OperatorGroup, ReductionMode, binary_key, unary_key};
use crate::machine::{
    Action, AwaitContinue, BodyCtx, DepPlacement, DepTerminal, FinishCtx, SubDispatch,
    require_kexpression,
};
use crate::machine::{Body, CarrierWitness, KError, KErrorKind, Scope};
use crate::source::Spanned;
use crate::witnessed::Witnessed;

use super::fn_def::return_type::{ReturnTypeState, classify_return_type, extract_type_slot_raw};
use super::resolve_or_await::{expect_type_terminal, resolve_at_wake};
use super::{arg, kw, sig};
use crate::machine::model::RunRegistries;

/// Slot labels for the type-resolution diagnostics.
const OPERAND_SLOT: &str = "OP operand type";
const RESULT_SLOT: &str = "OP result type";

/// Which surface declared the operator — the one axis the shared body branches on.
#[derive(Clone, Copy, PartialEq, Eq)]
enum OpKind {
    /// `OP #(<sym>) OVER Operand [-> Result] = (<body>)`: binds `left` / `right`.
    Binary,
    /// `UNARY OP #(<sym>) OVER Operand -> Result = (<body>)`: binds `operands`.
    Unary,
}

// ---------- symbol extraction ----------
//
// The statement-side symbol reader and the OP bucket extractor live in
// [`crate::machine::model::binder`] (the single source of truth for binder discovery). They are
// re-imported here for the registration sites and re-exported for `GROUP`, which reads its member
// operators the same way.

use crate::machine::BoundArgs;
pub(super) use crate::machine::model::symbol_from_parts;
use crate::machine::model::symbol_from_quote_body;
use crate::machine::model::{StaticName, ValueSymbol};
use crate::machine::{GroupSeal, OverloadSeal};

// This builtin's slot spellings, minted once and read back by symbol. A pairwise group's
// combiner is itself an `OP`, so it binds the same `left` / `right` pair — but positionally, by
// the infix shape the reducer synthesizes, not by name. `operands` is the unary form's single
// parameter: the whole run as one list.
crate::slots! { SLOTS { body, left, name, operand, operands, return_type, right, symbol } }

/// Body-side symbol read: a quoted slot's raw `KObject::KExpression` is the quote body. Shared with
/// `GROUP`, whose pairwise `combiner` slot names an operator the same way (`super::group_def`).
pub(super) fn symbol_from_slot(
    args: BoundArgs<'_, '_>,
    builtin: &str,
    slot: &StaticName<ValueSymbol>,
    labels: &LabelInterner,
) -> Result<KeywordSymbol, KError> {
    let quoted = require_kexpression(args, builtin, slot)?;
    symbol_from_quote_body(&quoted).map_err(|reason| reason.into_error(labels))
}

// ---------- type slots ----------

/// A type slot's state across the (possible) dep-finish boundary: resolved outright, re-resolved
/// against the wake-side scope, or sub-dispatched as a `:(…)` expression whose result comes back at
/// dep index `dep_index`.
enum TypeCapture {
    Done(KType),
    AtWake(TypeSymbol),
    Sub { dep_index: usize },
}

/// Route one classified type slot into a [`TypeCapture`], appending whatever it must wait on to the
/// one dep list — the producers behind a still-finalizing type binder, or a sub-dispatch whose dep
/// index the capture records so the finish can read its result back.
fn capture_type_slot<'a>(
    state: ReturnTypeState<'a>,
    deps: &mut Deps<SubDispatch<'a>>,
    brand: RegionBrand<'a>,
) -> Result<TypeCapture, KError> {
    match state {
        ReturnTypeState::Done(kt) => Ok(TypeCapture::Done(kt)),
        ReturnTypeState::Pending { te, producers } => {
            extend_deps_on(deps, producers);
            Ok(TypeCapture::AtWake(te))
        }
        ReturnTypeState::ExprToSubDispatch(expr) => Ok(TypeCapture::Sub {
            dep_index: deps.request(SubDispatch {
                expr: crate::machine::model::WorkingExpression::from_ast(brand, expr),
                placement: DepPlacement::OwnScope,
            }),
        }),
        // An operator's operands are named by the surface, not declared as parameters, so an `OP`
        // type slot can reference nothing that is unbound in the declaring scope: the per-call
        // deferral `FN` needs for `-> er` never arises here.
        ReturnTypeState::Deferred(_) => Err(KError::new(KErrorKind::ShapeError(
            "OP type slot cannot reference a parameter".to_string(),
        ))),
    }
}

/// An operand and a result each type a value, so each must be a proper type; a bare constructor
/// of kind `* -> *` standing unapplied is a kind error. Guards both readback paths, so the
/// synchronous and dep-finished builds share one verdict.
///
/// The kind diagnostic reads `label` as the subject of "must be a proper type", so the bare slot
/// noun takes its definite article here.
fn checked_value_type(kt: KType, label: &str, registries: &RunRegistries) -> Result<KType, KError> {
    match crate::machine::model::unsaturated_constructor_message(
        kt,
        &format!("the {label}"),
        registries,
    ) {
        Some(message) => Err(KError::new(KErrorKind::ShapeError(message))),
        None => Ok(kt),
    }
}

/// The `Done` arm alone — the synchronous path, taken exactly when no slot parked or
/// sub-dispatched.
fn done_type(
    capture: TypeCapture,
    label: &str,
    registries: &RunRegistries,
) -> Result<KType, KError> {
    match capture {
        TypeCapture::Done(kt) => checked_value_type(kt, label, registries),
        _ => Err(KError::new(KErrorKind::ShapeError(format!(
            "{label} is unresolved with no dependency to wait on"
        )))),
    }
}

/// Read a capture back at dep-finish: a parked name re-resolves against the wake-side scope, a
/// sub-dispatched expression reads its terminal's type. The type is owned data, cloned out of the
/// terminal, so it crosses into the declaring scope by value.
fn resolve_capture<'a>(
    capture: TypeCapture,
    fctx: &FinishCtx<'a, '_>,
    results: &[DepTerminal<'_>],
    label: &str,
) -> Result<KType, KError> {
    let kt = match capture {
        TypeCapture::Done(kt) => kt,
        TypeCapture::AtWake(te) => {
            resolve_at_wake(fctx.scope, label, fctx.registries, |s, registries| {
                s.resolve_type_identifier(te, None, registries)
            })
        }?,
        TypeCapture::Sub { dep_index } => {
            expect_type_terminal(results, dep_index, label, fctx.registries)?
        }
    };
    checked_value_type(kt, label, fctx.registries)
}

// ---------- body ----------

/// The `OP` body: extract and validate the symbol, check the group context, elaborate the operand
/// (and any explicit result) type, then synthesize and register the operator's `KFunction`(s). A
/// type slot naming a still-finalizing type binder — or spelled as a `:(…)` expression that has to
/// sub-dispatch — defers the whole build to a dep-finish.
fn build<'a>(
    ctx: &BodyCtx<'_, 'a, '_>,
    kind: OpKind,
    bound_name: Option<crate::machine::model::ValueSymbol>,
) -> Action<'a> {
    let sym = crate::try_action!(symbol_from_slot(
        ctx.args,
        "OP",
        &SLOTS.symbol,
        &ctx.registries.labels
    ));
    let body_expr = crate::try_action!(require_kexpression(ctx.args, "OP", &SLOTS.body));
    let has_result = ctx.args.held(&SLOTS.return_type).is_some();
    let group = ctx.scope.nearest_group_context();
    crate::try_action!(check_group_context(
        kind,
        has_result,
        group,
        sym,
        &ctx.registries.labels
    ));

    let operand_raw = crate::try_action!(extract_type_slot_raw(
        ctx.args,
        &SLOTS.operand,
        OPERAND_SLOT
    ));
    let operand_state = crate::try_action!(classify_return_type(
        operand_raw,
        &[],
        ctx.scope,
        ctx.chain.clone(),
        OPERAND_SLOT,
        ctx.registries,
    ));
    let result_state = if has_result {
        let raw = crate::try_action!(extract_type_slot_raw(
            ctx.args,
            &SLOTS.return_type,
            RESULT_SLOT
        ));
        Some(crate::try_action!(classify_return_type(
            raw,
            &[],
            ctx.scope,
            ctx.chain.clone(),
            RESULT_SLOT,
            ctx.registries,
        )))
    } else {
        None
    };

    // One dep list, built as the slots are classified: an operand's producers and a result's
    // sub-dispatch interleave freely, since each capture records the dep index its own result
    // arrives at.
    let brand = ctx.scope.brand();
    let mut deps: Deps<SubDispatch<'a>> = Deps::new();
    let operand_capture = crate::try_action!(capture_type_slot(operand_state, &mut deps, brand));
    let result_capture = match result_state {
        Some(state) => Some(crate::try_action!(capture_type_slot(
            state, &mut deps, brand
        ))),
        None => None,
    };

    // The group context is a property of the declaring scope, which a dep-finish re-projects
    // unchanged, so it is decided here — once — for both paths.
    let plan = OpPlan {
        sym,
        kind,
        body_expr,
        in_group: group.is_some(),
        bind_index: ctx.bind_index(),
        program: ctx.program,
        bound_name,
    };
    if deps.is_empty() {
        let operand = crate::try_action!(done_type(operand_capture, OPERAND_SLOT, ctx.registries));
        let result = match result_capture {
            Some(capture) => Some(crate::try_action!(done_type(
                capture,
                RESULT_SLOT,
                ctx.registries
            ))),
            None => None,
        };
        return op_action(plan.finalize(ctx.scope, operand, result, ctx.registries));
    }
    let finish: AwaitContinue<'a> = Box::new(move |fctx, results| {
        let operand = crate::try_action!(resolve_capture(
            operand_capture,
            fctx,
            results,
            OPERAND_SLOT
        ));
        let result = match result_capture {
            Some(capture) => Some(crate::try_action!(resolve_capture(
                capture,
                fctx,
                results,
                RESULT_SLOT
            ))),
            None => None,
        };
        op_action(plan.finalize(fctx.scope, operand, result, fctx.registries))
    });
    Action::await_deps(deps, finish)
}

/// The surface rules an operator declaration's *context* decides (see
/// [`Scope::nearest_group_context`]):
///
/// - an explicit `-> Result` makes a binary operator heterogeneous, which only holds where the pair
///   results are folded through a combiner — i.e. inside a `PAIRWISE` group. A fold member's result
///   is its operand type, since the fold feeds it back in;
/// - a unary operator takes the whole run as one list, so there is nothing for a group to chain it
///   with.
fn check_group_context(
    kind: OpKind,
    has_result: bool,
    group: Option<&OperatorGroup<'_>>,
    symbol: KeywordSymbol,
    labels: &LabelInterner,
) -> Result<(), KError> {
    let sym = labels.display(symbol.symbol());
    if kind == OpKind::Unary && group.is_some() {
        return Err(KError::new(KErrorKind::ShapeError(format!(
            "`UNARY OP #({sym})` cannot be declared inside a GROUP: a unary operator takes the \
             whole run as one list, so it chains with nothing",
        ))));
    }
    if kind == OpKind::Binary && has_result {
        let pairwise = group.is_some_and(|g| matches!(g.mode(), ReductionMode::Pairwise { .. }));
        if !pairwise {
            return Err(KError::new(KErrorKind::ShapeError(format!(
                "`OP #({sym})` declares an explicit `-> Result`, which only a PAIRWISE group's \
                 members may do — a fold member's result is its operand type. Drop the `->`, or \
                 declare the operator inside a `GROUP … PAIRWISE FOLD …` body",
            ))));
        }
    }
    Ok(())
}

/// Everything the finalize needs that does not come out of the dep results, captured whole into the
/// dep-finish closure so the deferred and synchronous paths run the same code.
struct OpPlan<'program: 'a, 'a> {
    sym: KeywordSymbol,
    kind: OpKind,
    body_expr: KExpression<'a>,
    /// Inside a `GROUP` body the group owns the registry entry for every member, so the declaration
    /// writes the function bucket only.
    in_group: bool,
    bind_index: BindingIndex,
    /// The run's program storage capability, carried off the declaring step's [`BodyCtx`] at its
    /// own `'program`: the bridge body is a **value-channel** node, so its marked operand arms are
    /// mintable only here, and only against parts that outlive program storage.
    program: ProgramBrand<'program>,
    /// `Some` for the combined `LET <name> = OP …` statement, which also binds the operator's
    /// primary function under that value name — one declaration reaching both install channels.
    bound_name: Option<crate::machine::model::ValueSymbol>,
}

impl<'program: 'a, 'a> OpPlan<'program, 'a> {
    /// Synthesize the operator's `KFunction`(s) and describe the writes they imply — the function
    /// bucket overloads and, outside a group, the size-1 registry entry that makes a run of three or
    /// more operands reduce. Returns the declared function's value beside those writes, which ride
    /// the step outcome.
    fn finalize(
        self,
        scope: &'a Scope<'a>,
        operand: KType,
        result: Option<KType>,
        registries: &RunRegistries,
    ) -> Result<(Witnessed<CarriedFamily, CarrierWitness>, Vec<WriteOp<'a>>), KError> {
        let types = &registries.types;
        let OpPlan {
            sym,
            kind,
            body_expr,
            in_group,
            bind_index,
            program,
            bound_name,
        } = self;
        let mut writes: Vec<WriteOp<'a>> = Vec::new();
        // The cell of the operator's *primary* function — the binary body for a binary operator,
        // the list body for a unary one. It is the value the declaration evaluates to, and, for the
        // combined form, the value the bound name reads.
        let cell = match kind {
            OpKind::Binary => {
                let elements = vec![
                    arg(registries, &SLOTS.left, operand),
                    SignatureElement::Keyword(sym),
                    arg(registries, &SLOTS.right, operand),
                ];
                let result_type = result.unwrap_or(operand);
                let (cell, overload) = register_body(
                    scope,
                    sym,
                    sig(result_type, elements),
                    Body::UserDefined(body_expr),
                    bind_index,
                    registries,
                )?;
                writes.push(overload);
                if !in_group {
                    let record = scope.birth_operator_group(&[sym], ReductionMode::FoldLeft);
                    writes.push(WriteOp::Group {
                        probes: powerset_probes(&[sym], &registries.labels),
                        seal: GroupSeal::of_delivered(scope, &record),
                        index: bind_index,
                    });
                }
                cell
            }
            OpKind::Unary => {
                let result_type = result.ok_or_else(|| {
                    KError::new(KErrorKind::ShapeError(
                        "UNARY OP requires an explicit `-> Result`".to_string(),
                    ))
                })?;
                let list_signature = sig(
                    result_type,
                    vec![
                        SignatureElement::Keyword(sym),
                        arg(registries, &SLOTS.operands, types.list(operand)),
                    ],
                );
                // The binary bridge: `a ~ b` names one keyword, so it dispatches as a plain
                // keyworded call, not an operator chain — without a two-operand body it would
                // simply miss. Its body is the AST `sym [left right]`, the shape a reduced run
                // takes, so both surfaces land on the one list body the user wrote.
                let bridge_signature = sig(
                    result_type,
                    vec![
                        arg(registries, &SLOTS.left, operand),
                        SignatureElement::Keyword(sym),
                        arg(registries, &SLOTS.right, operand),
                    ],
                );
                // `check_group_context` rejects `UNARY OP` inside a `GROUP` before the plan is
                // built, so `in_group` cannot hold here; the door asserts that rather than take
                // it on trust, since it writes the single-member group unconditionally.
                let (cell, unary_writes) = register_unary_operator(
                    scope,
                    sym,
                    OperatorForm {
                        signature: list_signature,
                        body: Body::UserDefined(body_expr),
                    },
                    OperatorForm {
                        signature: bridge_signature,
                        body: Body::UserDefined(bridge_body(program, &registries.labels, sym)),
                    },
                    in_group,
                    bind_index,
                    registries,
                )?;
                writes.extend(unary_writes);
                cell
            }
        };
        // One `KFunction`, two writes at the same `BindingIndex` the submission-time placeholder
        // stamps: the bound name and the registered overload are the same operator body.
        if let Some(bound_name) = bound_name {
            writes.push(WriteOp::Value {
                name: bound_name,
                index: bind_index,
                sealed: cell.duplicate(),
            });
        }
        Ok((cell.unseal(), writes))
    }
}

/// One dispatchable form of an operator: the signature naming a surface, and the body that surface
/// reaches. A unary operator is registered from two — the list form and the binary form.
pub(super) struct OperatorForm<'a> {
    pub signature: SignatureDraft<'a>,
    pub body: Body<'a>,
}

/// Register the fixed triple every unary operator consists of: the list-form overload under
/// [`unary_key`], the binary-form overload under [`binary_key`], and the size-1
/// [`ReductionMode::Unary`] group entry (key derived through [`powerset_probes`]). The bodies ride
/// in already built, koan-AST or native alike. Returns the list-form function's object and stored
/// reach: the list body is the operator's primary value.
///
/// Registration derives each bucket key from the signature the caller hands in, so a caller that
/// spells a signature the use site never computes would register into a bucket no koan expression
/// reaches — the operator would silently never dispatch. The signature asserts close that channel;
/// a mismatch can only come from crate code, never from koan source.
///
/// `in_group` is the caller's group context, and must be `false`: a single-member group is the only
/// group a unary operator can be in, because its reduction hands the whole run to one body as a
/// single list, which presupposes the run names no other operator. The door describes that group
/// unconditionally, so it asserts the context rather than trusting it — a grouped caller would
/// write a size-1 `Unary` record under the very key its `GROUP` already claims.
pub(super) fn register_unary_operator<'a>(
    scope: &'a Scope<'a>,
    sym: KeywordSymbol,
    list: OperatorForm<'a>,
    binary: OperatorForm<'a>,
    in_group: bool,
    bind_index: BindingIndex,
    registries: &RunRegistries,
) -> Result<(SealedValue<'a>, Vec<WriteOp<'a>>), KError> {
    let OperatorForm {
        signature: list_signature,
        body: list_body,
    } = list;
    let OperatorForm {
        signature: binary_signature,
        body: binary_body,
    } = binary;
    let spelling = registries.labels.display(sym.symbol());
    assert_eq!(
        list_signature.untyped_key(),
        unary_key(sym),
        "unary operator `{spelling}`: the list-form signature must key the bucket a reduced run or \
         a prefix use computes",
    );
    assert_eq!(
        binary_signature.untyped_key(),
        binary_key(sym),
        "unary operator `{spelling}`: the binary-form signature must key the bucket a two-operand \
         use computes",
    );
    assert!(
        !in_group,
        "unary operator `{spelling}`: a unary operator chains with nothing, so it can only be its \
         own single-member group",
    );
    // The list body first: its function is the operator's primary value, the one an `OP`
    // declaration evaluates to.
    let (cell, list_overload) = register_body(
        scope,
        sym,
        list_signature,
        list_body,
        bind_index,
        registries,
    )?;
    let (_, binary_overload) = register_body(
        scope,
        sym,
        binary_signature,
        binary_body,
        bind_index,
        registries,
    )?;
    let record = scope.birth_operator_group(&[sym], ReductionMode::Unary);
    let mut writes = vec![list_overload, binary_overload];
    writes.push(WriteOp::Group {
        probes: powerset_probes(&[sym], &registries.labels),
        seal: GroupSeal::of_delivered(scope, &record),
        index: bind_index,
    });
    Ok((cell, writes))
}

/// Allocate one operator body as a `KFunction` capturing `scope`, and describe its bucket write
/// through the operator door — [`WriteOp::Overload`] without the builtin-shadow guard, so a user
/// module may declare an operator the root already declares (`OP #(+) OVER :(LIST OF Number)`).
/// Shadowing an operator is **type-gated**, not free: dispatch consults the immutable root bucket
/// first, so the builtin `+` still wins for the operand types it declares and only other operand
/// types reach the module's body. Ordinary user `FN`s keep the guard.
///
/// The callable is born into `scope`'s own region, and its birth's composition is what names that
/// region as a member of the description both doors below carry — the bucket seal and the value
/// wrapper compose from the one envelope, so the two never state the reach independently. Bare-`FN`
/// style: the overload lands in `functions` only, never in `data`.
fn register_body<'a>(
    scope: &'a Scope<'a>,
    sym: KeywordSymbol,
    signature: SignatureDraft<'a>,
    body: Body<'a>,
    bind_index: BindingIndex,
    registries: &RunRegistries,
) -> Result<(SealedValue<'a>, WriteOp<'a>), KError> {
    let cell = KFunction::alloc_captured(scope, signature, body, registries);
    let write = WriteOp::Overload {
        // The `functions` table keys an overload by the operator's spelling, so this is the one
        // place the glyph is resolved back — the parse that classified it interned it.
        name: registries
            .labels
            .resolve(sym.symbol())
            .expect("a parsed operator glyph is interned where it was classified"),
        index: bind_index,
        seal: OverloadSeal::of_delivered(scope, &cell),
        builtin_shadow_guard: false,
    };
    Ok((scope.store_function_cell(&cell), write))
}

/// The bridge body `sym [left right]` — a keyword-first call over a two-element list literal, which
/// dispatches straight to the unary operator's list body. Each parameter is its own one-part
/// expression element: a list literal interns a bare `Identifier` element as a symbol rather than
/// resolving it, so the two operands ride in as element expressions (exactly as a reduced infix run
/// carries its named operands).
///
/// The one runtime site that mints **value-channel** nodes: the operand wrappers are marked
/// `Expression` arms, so the whole body — the operand nodes and the node the parts reach — builds
/// in program storage. The single `'a` is the brand's own lifetime, which is what the mint doors
/// take; `sym` is already the classified glyph, so the keyword part is a copy.
fn bridge_body<'a>(
    program: ProgramBrand<'a>,
    labels: &LabelInterner,
    sym: KeywordSymbol,
) -> KExpression<'a> {
    let brand = program.region();
    let operand = |slot: &StaticName<ValueSymbol>| {
        ExpressionPart::expression(
            program,
            &[Spanned::bare(ExpressionPart::Identifier(
                labels.record(slot),
            ))],
        )
    };
    KExpression::new(
        brand,
        &[
            Spanned::bare(ExpressionPart::Keyword(sym)),
            Spanned::bare(ExpressionPart::ListLiteral(
                brand
                    .allocator()
                    .slice(&[operand(&SLOTS.left), operand(&SLOTS.right)]),
            )),
        ],
    )
}

/// Seal a finalize result as the slot's terminal — the operator function value, built witnessed in
/// its declaring scope's region.
fn op_action<'a>(
    result: Result<(Witnessed<CarriedFamily, CarrierWitness>, Vec<WriteOp<'a>>), KError>,
) -> Action<'a> {
    match result {
        Ok((witnessed, writes)) => {
            Action::done(Ok(StepCarried::born(witnessed))).with_effects(writes)
        }
        Err(e) => Action::done(Err(e)),
    }
}

fn body_binary<'a>(ctx: &BodyCtx<'_, 'a, '_>) -> Action<'a> {
    build(ctx, OpKind::Binary, None)
}

fn body_unary<'a>(ctx: &BodyCtx<'_, 'a, '_>) -> Action<'a> {
    build(ctx, OpKind::Unary, None)
}

/// `LET <name> = OP #(<sym>) OVER <Operand> [-> <Result>] = (<body>)` — one statement whose single
/// binder installs the value name and the operator's bucket key(s). The bound value is the
/// operator's primary function, the same one the declaration evaluates to.
fn body_binary_combined<'a>(ctx: &BodyCtx<'_, 'a, '_>) -> Action<'a> {
    let name = crate::try_action!(crate::builtins::fn_def::combined_bound_name(ctx.args));
    build(ctx, OpKind::Binary, Some(name))
}

/// The `UNARY OP` twin of [`body_binary_combined`]; its binder installs two bucket keys — the
/// keyword-first list key and the binary bridge key.
fn body_unary_combined<'a>(ctx: &BodyCtx<'_, 'a, '_>) -> Action<'a> {
    let name = crate::try_action!(crate::builtins::fn_def::combined_bound_name(ctx.args));
    build(ctx, OpKind::Unary, Some(name))
}

/// `UNARY OP #(<sym>) OVER Operand = (<body>)` — the result segment is mandatory: a unary body
/// consumes a whole list of operands, so its result type is not its operand type and there is
/// nothing to default it to. This overload exists only to say so; without it the shape is a bare
/// dispatch miss.
fn body_unary_missing_result<'a>(ctx: &BodyCtx<'_, 'a, '_>) -> Action<'a> {
    let symbol = crate::try_action!(symbol_from_slot(
        ctx.args,
        "OP",
        &SLOTS.symbol,
        &ctx.registries.labels
    ));
    let sym = ctx.registries.labels.display(symbol.symbol());
    Action::done(Err(KError::new(KErrorKind::ShapeError(format!(
        "`UNARY OP #({sym})` must declare its result type: \
         `UNARY OP #({sym}) OVER <Operand> -> <Result> = (…)`",
    )))))
}

/// The combined twin of [`body_unary_missing_result`], naming the flat spelling in its suggestion.
fn body_unary_missing_result_combined<'a>(ctx: &BodyCtx<'_, 'a, '_>) -> Action<'a> {
    let symbol = crate::try_action!(symbol_from_slot(
        ctx.args,
        "OP",
        &SLOTS.symbol,
        &ctx.registries.labels
    ));
    let sym = ctx.registries.labels.display(symbol.symbol());
    let name = match crate::builtins::fn_def::combined_bound_name(ctx.args) {
        Ok(name) => crate::machine::model::render_label(name.symbol(), ctx.registries),
        Err(_) => "op".to_string(),
    };
    Action::done(Err(KError::new(KErrorKind::ShapeError(format!(
        "`UNARY OP #({sym})` must declare its result type: \
         `LET {name} = UNARY OP #({sym}) OVER <Operand> -> <Result> = (…)`",
    )))))
}

/// The two carriers a type slot arrives on. `OfKind(ProperType)` takes a bare type token (`OVER
/// Number`); `SigiledTypeExpr` takes the sigiled form (`OVER :Number`, `OVER :(LIST OF Elt)`) raw,
/// so the body sub-dispatches it rather than resolving a name that may not be one. Every
/// operand × result combination of the two is registered, mirroring how `fn_def` splits its return
/// slot.
fn type_carriers() -> [KType; 2] {
    [KType::of_kind(KKind::ProperType), KType::SIGILED_TYPE_EXPR]
}

/// The combined statement form of a declaration surface: `LET <name> =` prefixed to its element
/// list. Every surface below is built once and registered under both spellings, so the two can
/// never drift apart. Full-bucket-key matching keeps the combined keys disjoint from plain `LET`
/// and bare `OP`.
fn combined<'a>(
    registries: &RunRegistries,
    mut elements: Vec<SignatureElement>,
) -> SignatureDraft<'a> {
    let mut prefixed = vec![
        kw(registries, "LET"),
        arg(registries, &SLOTS.name, KType::IDENTIFIER),
        kw(registries, "="),
    ];
    prefixed.append(&mut elements);
    sig(KType::ANY, prefixed)
}

pub fn register<'a>(scope: &'a Scope<'a>, registries: &RunRegistries, gate: &mut WriteGate) {
    use crate::builtins::register_builtin;

    // Declared return is `KType::ANY`: an operator declaration evaluates to the function it
    // synthesizes, whose structural type only exists once its signature is known.
    let binary = |operand: KType| {
        vec![
            kw(registries, "OP"),
            arg(registries, &SLOTS.symbol, KType::KEXPRESSION),
            kw(registries, "OVER"),
            arg(registries, &SLOTS.operand, operand),
            kw(registries, "="),
            arg(registries, &SLOTS.body, KType::KEXPRESSION),
        ]
    };
    let binary_with_result = |operand: KType, result: KType| {
        vec![
            kw(registries, "OP"),
            arg(registries, &SLOTS.symbol, KType::KEXPRESSION),
            kw(registries, "OVER"),
            arg(registries, &SLOTS.operand, operand),
            kw(registries, "->"),
            arg(registries, &SLOTS.return_type, result),
            kw(registries, "="),
            arg(registries, &SLOTS.body, KType::KEXPRESSION),
        ]
    };
    let unary = |operand: KType, result: KType| {
        vec![
            kw(registries, "UNARY"),
            kw(registries, "OP"),
            arg(registries, &SLOTS.symbol, KType::KEXPRESSION),
            kw(registries, "OVER"),
            arg(registries, &SLOTS.operand, operand),
            kw(registries, "->"),
            arg(registries, &SLOTS.return_type, result),
            kw(registries, "="),
            arg(registries, &SLOTS.body, KType::KEXPRESSION),
        ]
    };
    let unary_missing_result = |operand: KType| {
        vec![
            kw(registries, "UNARY"),
            kw(registries, "OP"),
            arg(registries, &SLOTS.symbol, KType::KEXPRESSION),
            kw(registries, "OVER"),
            arg(registries, &SLOTS.operand, operand),
            kw(registries, "="),
            arg(registries, &SLOTS.body, KType::KEXPRESSION),
        ]
    };

    for operand in type_carriers() {
        register_builtin(
            scope,
            "OP",
            sig(KType::ANY, binary(operand)),
            body_binary,
            registries,
            gate,
        );
        register_builtin(
            scope,
            "LET",
            combined(registries, binary(operand)),
            body_binary_combined,
            registries,
            gate,
        );
        register_builtin(
            scope,
            "OP",
            sig(KType::ANY, unary_missing_result(operand)),
            body_unary_missing_result,
            registries,
            gate,
        );
        register_builtin(
            scope,
            "LET",
            combined(registries, unary_missing_result(operand)),
            body_unary_missing_result_combined,
            registries,
            gate,
        );
        for result in type_carriers() {
            register_builtin(
                scope,
                "OP",
                sig(KType::ANY, binary_with_result(operand, result)),
                body_binary,
                registries,
                gate,
            );
            register_builtin(
                scope,
                "LET",
                combined(registries, binary_with_result(operand, result)),
                body_binary_combined,
                registries,
                gate,
            );
            register_builtin(
                scope,
                "OP",
                sig(KType::ANY, unary(operand, result)),
                body_unary,
                registries,
                gate,
            );
            register_builtin(
                scope,
                "LET",
                combined(registries, unary(operand, result)),
                body_unary_combined,
                registries,
                gate,
            );
        }
    }
}

#[cfg(test)]
mod tests;
