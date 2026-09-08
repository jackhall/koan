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
//! a bare `EXPR` body does, and an `OP` statement evaluates to the function it declares.
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
use crate::machine::model::labels::{KeywordSymbol, LabelInterner, TypeSymbol};
use crate::machine::model::{ExpressionPart, KExpression};
use crate::machine::model::{KKind, KType};
use crate::machine::model::{OperatorGroup, ReductionMode, binary_key, unary_key};
use crate::machine::model::{SignatureDraft, SignatureElement};
use crate::machine::{
    Action, AwaitContinue, BodyCtx, DepPlacement, DepTerminal, FinishCtx, SubDispatch,
    require_kexpression,
};
use crate::machine::{Body, CarrierWitness, KError, KErrorKind, Scope};
use crate::source::Spanned;
use crate::witnessed::Witnessed;

use super::fn_def::return_type::{
    ReturnTypeState, TypeSlotThunk, classify_return_type, type_carrier_union,
};
use super::resolve_or_await::{expect_type_terminal, resolve_at_wake};
use super::{arg, arg_labeled, kw, sig};
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
use crate::machine::model::MACHINE_BINDERS;
use crate::machine::model::ReturnType;
pub(super) use crate::machine::model::symbol_from_parts;
use crate::machine::model::symbol_from_quote_body;
use crate::machine::model::{StaticName, ValueSymbol};
use crate::machine::model::{shape_type_of, untyped_key_of};
use crate::machine::{GroupSeal, OverloadSeal};

// This builtin's slot spellings, minted once and read back by symbol. The names an `OP` body
// binds its operands under are not among them: they are machine-fixed binders, declared once in
// the model layer as `MACHINE_BINDERS` and read back from there. A pairwise group's combiner is
// itself an `OP`, so it binds the same pair — but positionally, by the infix shape the reducer
// synthesizes, not by name.
crate::slots! { SLOTS { body, name, operand, return_type, symbol } }

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
        // deferral a definition needs for `-> er` never arises here.
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
        format_args!("the {label}"),
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
            resolve_at_wake(fctx.scope, label, None, fctx.registries, |s, registries| {
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
    // A SIG declares members rather than defining them, so a definition inside one is refused and
    // pointed at its declarator. Guarded here, ahead of any deferral, so the synchronous and
    // dep-finish paths are covered once — the same position `build_fn_like` guards from.
    if ctx.scope.is_in_sig_body() {
        let spelling = head_spelling(kind, ctx.registries.labels.display(sym.symbol()));
        return Action::done(Err(KError::new(KErrorKind::ShapeError(format!(
            "inside a SIG body, an operator is declared rather than defined — drop the \
             `= (<body>)` and write `({spelling})`",
        )))));
    }
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

    let operand_raw = crate::try_action!(TypeSlotThunk::from_slot(
        ctx.args,
        ctx.scope.brand(),
        &SLOTS.operand,
        OPERAND_SLOT,
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
        let raw = crate::try_action!(TypeSlotThunk::from_slot(
            ctx.args,
            ctx.scope.brand(),
            &SLOTS.return_type,
            RESULT_SLOT,
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
        return op_action(
            ctx.scratch,
            plan.finalize(ctx.scope, operand, result, ctx.registries),
        );
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
        op_action(
            fctx.scratch,
            plan.finalize(fctx.scope, operand, result, fctx.registries),
        )
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

/// What an [`OpPlan::finalize`] hands back: the operator's own witnessed carrier, and the
/// at-most-four binding writes its [`OpKind`] calls for — a unary operator's list overload, binary
/// bridge and single-member group, or a binary operator's overload and its own group, plus the
/// combined form's value binding either way.
type FinalizedOp<'a> = (
    Witnessed<CarriedFamily, CarrierWitness>,
    [Option<WriteOp<'a>>; 4],
);

impl<'program: 'a, 'a> OpPlan<'program, 'a> {
    /// Synthesize the operator's `KFunction`(s) and describe the writes they imply — the function
    /// bucket overloads and, outside a group, the size-1 registry entry that makes a run of three or
    /// more operands reduce. Returns the declared function's value beside those writes, which ride
    /// the step outcome.
    ///
    /// The write count is fixed by the two arms: a unary operator's triple is the widest, and the
    /// combined form's value binding sits beside it, so the four ride out as an array and land in
    /// the action's own bump. Nothing between here and there needs a buffer of its own.
    fn finalize(
        self,
        scope: &'a Scope<'a>,
        operand: KType,
        result: Option<KType>,
        registries: &RunRegistries,
    ) -> Result<FinalizedOp<'a>, KError> {
        let OpPlan {
            sym,
            kind,
            body_expr,
            in_group,
            bind_index,
            program,
            bound_name,
        } = self;
        // The cell of the operator's *primary* function — the binary body for a binary operator,
        // the list body for a unary one. It is the value the declaration evaluates to, and, for the
        // combined form, the value the bound name reads.
        let shape = operator_shape(kind, sym, operand, result, registries)?;
        let (cell, registrations) = match shape.list_elements {
            None => {
                let (cell, overload) = register_body(
                    scope,
                    ReturnType::Resolved(shape.result),
                    &shape.binary_elements,
                    Body::UserDefined(body_expr),
                    bind_index,
                    registries,
                )?;
                let group = (!in_group).then(|| {
                    let record = scope.birth_operator_group(&[sym], shape.singleton_mode);
                    WriteOp::Group {
                        probes: powerset_probes(&[sym], &registries.labels),
                        seal: GroupSeal::of_delivered(scope, &record),
                        index: bind_index,
                    }
                });
                (cell, [Some(overload), group, None])
            }
            Some(list_elements) => {
                // `check_group_context` rejects `UNARY OP` inside a `GROUP` before the plan is
                // built, so `in_group` cannot hold here; the door asserts that rather than take
                // it on trust, since it writes the single-member group unconditionally.
                let (cell, [list_overload, binary_overload, group]) = register_unary_operator(
                    scope,
                    sym,
                    OperatorForm {
                        return_type: ReturnType::Resolved(shape.result),
                        elements: &list_elements,
                        body: Body::UserDefined(body_expr),
                    },
                    OperatorForm {
                        return_type: ReturnType::Resolved(shape.result),
                        elements: &shape.binary_elements,
                        body: Body::UserDefined(bridge_body(program, &registries.labels, sym)),
                    },
                    in_group,
                    bind_index,
                    registries,
                )?;
                (
                    cell,
                    [Some(list_overload), Some(binary_overload), Some(group)],
                )
            }
        };
        // One `KFunction`, two writes at the same `BindingIndex` the submission-time placeholder
        // stamps: the bound name and the registered overload are the same operator body.
        let value_write = bound_name.map(|bound_name| WriteOp::Value {
            name: bound_name,
            index: bind_index,
            sealed: cell.duplicate(),
        });
        let [first, second, third] = registrations;
        Ok((cell.unseal(), [first, second, third, value_write]))
    }
}

/// Every dispatchable form an operator declaration writes — the one place a definition and a SIG
/// declaration derive their bucket keys and slot types from, so a head declares exactly the shape
/// the definition satisfying it registers.
///
/// `binary_elements` always keys [`binary_key`]: it is a binary operator's own form, and a unary
/// operator's **bridge** — `a ~ b` names one keyword, so it dispatches as a plain keyworded call
/// rather than an operator chain, and without a two-operand entry it would simply miss.
/// `list_elements` is `Some` for a unary operator only, keying [`unary_key`] with the whole run as
/// one list operand; its presence is what tells the two arms apart past this point.
struct OperatorShape {
    binary_elements: [SignatureElement; 3],
    list_elements: Option<[SignatureElement; 2]>,
    /// The result a body of either form returns. A binary operator with no explicit `-> Result`
    /// folds, so its result is its operand type.
    result: KType,
    /// The mode of the size-1 registry record the declaration writes when it is not a group
    /// member: a binary operator folds left, a unary one takes the whole run.
    singleton_mode: ReductionMode,
}

/// Derive [`OperatorShape`] from the surface's own four facts. The only failure is the unary
/// arm's missing result: a unary body is handed the run as a list, so nothing feeds its result
/// back and there is no operand type to default to.
fn operator_shape(
    kind: OpKind,
    sym: KeywordSymbol,
    operand: KType,
    result: Option<KType>,
    registries: &RunRegistries,
) -> Result<OperatorShape, KError> {
    let binary_elements = [
        arg(registries, &MACHINE_BINDERS.operand_left, operand),
        SignatureElement::Keyword(sym),
        arg(registries, &MACHINE_BINDERS.operand_right, operand),
    ];
    Ok(match kind {
        OpKind::Binary => OperatorShape {
            binary_elements,
            list_elements: None,
            result: result.unwrap_or(operand),
            singleton_mode: ReductionMode::FoldLeft,
        },
        OpKind::Unary => OperatorShape {
            binary_elements,
            list_elements: Some([
                SignatureElement::Keyword(sym),
                arg(
                    registries,
                    &MACHINE_BINDERS.operands,
                    registries.types.list(operand),
                ),
            ]),
            result: result.ok_or_else(|| {
                KError::new(KErrorKind::ShapeError(
                    "UNARY OP requires an explicit `-> Result`".to_string(),
                ))
            })?,
            singleton_mode: ReductionMode::Unary,
        },
    })
}

/// One dispatchable form of an operator: the signature naming a surface, and the body that surface
/// reaches. A unary operator is registered from two — the list form and the binary form.
pub(super) struct OperatorForm<'a, 'e> {
    pub return_type: ReturnType<'a>,
    /// The pre-mint element buffer, borrowed from the caller's own frame — a signature is minted
    /// from it inside the door and the buffer is dead the moment the call returns.
    pub elements: &'e [SignatureElement],
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
    list: OperatorForm<'a, '_>,
    binary: OperatorForm<'a, '_>,
    in_group: bool,
    bind_index: BindingIndex,
    registries: &RunRegistries,
) -> Result<(SealedValue<'a>, [WriteOp<'a>; 3]), KError> {
    let OperatorForm {
        return_type: list_return,
        elements: list_elements,
        body: list_body,
    } = list;
    let OperatorForm {
        return_type: binary_return,
        elements: binary_elements,
        body: binary_body,
    } = binary;
    let spelling = registries.labels.display(sym.symbol());
    assert_eq!(
        untyped_key_of(list_elements),
        unary_key(sym),
        "unary operator `{spelling}`: the list-form signature must key the bucket a reduced run or \
         a prefix use computes",
    );
    assert_eq!(
        untyped_key_of(binary_elements),
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
        list_return,
        list_elements,
        list_body,
        bind_index,
        registries,
    )?;
    let (_, binary_overload) = register_body(
        scope,
        binary_return,
        binary_elements,
        binary_body,
        bind_index,
        registries,
    )?;
    let record = scope.birth_operator_group(&[sym], ReductionMode::Unary);
    let group = WriteOp::Group {
        probes: powerset_probes(&[sym], &registries.labels),
        seal: GroupSeal::of_delivered(scope, &record),
        index: bind_index,
    };
    Ok((cell, [list_overload, binary_overload, group]))
}

/// Allocate one operator body as a `KFunction` capturing `scope`, and describe its bucket write
/// through the operator door — [`WriteOp::Overload`] without the builtin-shadow guard, so a user
/// module may declare an operator the root already declares (`OP #(+) OVER :(LIST OF Number)`).
/// Shadowing an operator is **type-gated**, not free: dispatch consults the immutable root bucket
/// first, so the builtin `+` still wins for the operand types it declares and only other operand
/// types reach the module's body. Ordinary user definitions keep the guard.
///
/// The callable is born into `scope`'s own region, and its birth's composition is what names that
/// region as a member of the description both doors below carry — the bucket seal and the value
/// wrapper compose from the one envelope, so the two never state the reach independently. Bare-`EXPR`
/// style: the overload lands in `functions` only, never in `data`.
fn register_body<'a>(
    scope: &'a Scope<'a>,
    return_type: ReturnType<'a>,
    elements: &[SignatureElement],
    body: Body<'a>,
    bind_index: BindingIndex,
    registries: &RunRegistries,
) -> Result<(SealedValue<'a>, WriteOp<'a>), KError> {
    let cell = KFunction::alloc_captured(scope, return_type, elements, &[], body, registries);
    let write = WriteOp::Overload {
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
            Spanned::bare(ExpressionPart::ListLiteral(brand.allocator().slice(&[
                operand(&MACHINE_BINDERS.operand_left),
                operand(&MACHINE_BINDERS.operand_right),
            ]))),
        ],
    )
}

/// Seal a finalize result as the slot's terminal — the operator function value, built witnessed in
/// its declaring scope's region.
fn op_action<'a>(
    scratch: crate::witnessed::BumpAllocator<'a>,
    result: Result<FinalizedOp<'a>, KError>,
) -> Action<'a> {
    match result {
        Ok((witnessed, writes)) => Action::done(Ok(StepCarried::born(witnessed)))
            .with_effects(scratch, writes.into_iter().flatten()),
        Err(e) => Action::done(Err(e)),
    }
}

/// The bodyless head spelling a diagnostic points at — the declarator for `kind`, with `sym`
/// filled in and the operand and result left as placeholders.
fn head_spelling(kind: OpKind, sym: impl std::fmt::Display) -> String {
    match kind {
        OpKind::Binary => format!("OP #({sym}) OVER <Operand>"),
        OpKind::Unary => format!("UNARY OP #({sym}) OVER <Operand> -> <Result>"),
    }
}

/// The bodyless `OP` / `UNARY OP` head: a SIG body's **operator member**. It declares both halves
/// of what the definition writes — the dispatch bucket(s) under the keys a use site computes, and,
/// outside a SIG group, the size-1 chaining record — derived through the same
/// [`operator_shape`] the definition derives them through, so a head and the `OP` satisfying it
/// cannot spell different shapes.
///
/// The two type slots are ordinary kind expectations rather than the definition's raw-carrier
/// union, so a name resolves once where it is written and a still-finalizing sibling `TYPE Carrier`
/// parks the statement exactly as a `VAL zero :Carrier` slot parks. No deferral of its own is
/// needed, and the step's carrier is the declared primary function type — uniform with what a
/// bodyless `EXPR` head hands back.
fn declare<'a>(ctx: &BodyCtx<'_, 'a, '_>, kind: OpKind, has_result: bool) -> Action<'a> {
    let sym = crate::try_action!(symbol_from_slot(
        ctx.args,
        "OP",
        &SLOTS.symbol,
        &ctx.registries.labels
    ));
    let spelling = ctx.registries.labels.display(sym.symbol());
    if !ctx.scope.is_in_sig_body() {
        let head = head_spelling(kind, spelling);
        return Action::done(Err(KError::new(KErrorKind::ShapeError(format!(
            "a bodyless `{head}` head declares a SIG operator member and is only valid inside a \
             SIG body — write `{head} = (<body>)` to define an operator",
        )))));
    }
    // The SIG group context, the declaration-side twin of `nearest_group_context`: inside one the
    // group is the sole registrar for its members, and a pairwise mode is what admits a
    // heterogeneous `-> Result`.
    let group_mode = ctx.scope.nearest_sig_group();
    crate::try_action!(check_sig_group_context(
        kind, has_result, group_mode, spelling
    ));

    let operand = crate::try_action!(head_slot_type(ctx, &SLOTS.operand, OPERAND_SLOT));
    let result = if has_result {
        Some(crate::try_action!(head_slot_type(
            ctx,
            &SLOTS.return_type,
            RESULT_SLOT
        )))
    } else {
        None
    };
    let shape = crate::try_action!(operator_shape(kind, sym, operand, result, ctx.registries));

    // The keyworded writes: the binary form always, plus a unary head's list form. The step's own
    // carrier is the *primary* function type — the list body for a unary operator, the binary body
    // otherwise — the same choice the definition makes about which function it evaluates to.
    let binary_type = shape_type_of(&shape.binary_elements, &[], shape.result, ctx.registries);
    let mut writes: Vec<WriteOp<'a>> = vec![WriteOp::SigKeyworded { shape: binary_type }];
    let primary = match &shape.list_elements {
        None => binary_type,
        Some(list_elements) => {
            let list_type = shape_type_of(list_elements, &[], shape.result, ctx.registries);
            writes.push(WriteOp::SigKeyworded { shape: list_type });
            list_type
        }
    };
    // Inside a SIG group the group writes the one record over all its members; a head standing on
    // its own declares the singleton its own surface implies.
    if group_mode.is_none() {
        writes.push(WriteOp::SigOperatorGroup {
            members: vec![sym],
            mode: shape.singleton_mode,
        });
    }
    Action::done(Ok(StepCarried::born(
        ctx.scope
            .resident(crate::machine::model::Carried::Type(primary)),
    )))
    .with_effects(ctx.scratch, writes)
}

/// One head type slot, read off the resolved kind-expectation slot and checked to be a proper
/// type — a bare constructor standing unapplied types no value, so it can be neither operand nor
/// result.
fn head_slot_type(
    ctx: &BodyCtx<'_, '_, '_>,
    slot: &StaticName<ValueSymbol>,
    label: &str,
) -> Result<KType, KError> {
    let kt = ctx
        .args
        .ktype(slot)
        .ok_or_else(|| KError::new(KErrorKind::ShapeError(format!("{label} must be a type"))))?;
    checked_value_type(kt, label, ctx.registries)
}

/// [`check_group_context`]'s declaration-side twin, over a SIG group's mode rather than a live
/// [`OperatorGroup`] record. The two rules are the same ones, stated against the same surface: a
/// unary operator chains with nothing, and a heterogeneous `-> Result` only reads under a pairwise
/// fold.
fn check_sig_group_context(
    kind: OpKind,
    has_result: bool,
    group_mode: Option<ReductionMode>,
    sym: impl std::fmt::Display,
) -> Result<(), KError> {
    if kind == OpKind::Unary && group_mode.is_some() {
        return Err(KError::new(KErrorKind::ShapeError(format!(
            "`UNARY OP #({sym})` cannot be declared inside a GROUP: a unary operator takes the \
             whole run as one list, so it chains with nothing",
        ))));
    }
    if kind == OpKind::Binary
        && has_result
        && !matches!(group_mode, Some(ReductionMode::Pairwise { .. }))
    {
        return Err(KError::new(KErrorKind::ShapeError(format!(
            "`OP #({sym})` declares an explicit `-> Result`, which only a PAIRWISE group's \
             members may do — a fold member's result is its operand type. Drop the `->`, or \
             declare the member inside `(GROUP PAIRWISE FOLD #(<combiner>) LEFT = (…))`",
        ))));
    }
    Ok(())
}

fn body_binary<'a>(ctx: &BodyCtx<'_, 'a, '_>) -> Action<'a> {
    build(ctx, OpKind::Binary, None)
}

fn head_binary<'a>(ctx: &BodyCtx<'_, 'a, '_>) -> Action<'a> {
    declare(ctx, OpKind::Binary, false)
}

fn head_binary_with_result<'a>(ctx: &BodyCtx<'_, 'a, '_>) -> Action<'a> {
    declare(ctx, OpKind::Binary, true)
}

fn head_unary<'a>(ctx: &BodyCtx<'_, 'a, '_>) -> Action<'a> {
    declare(ctx, OpKind::Unary, true)
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
    // The SIG-body head forms: the definition spellings minus their `= (<body>)`. Full bucket-key
    // matching keeps each head's key disjoint from every definition spelling, so the two never
    // compete — the shorter key simply is not the longer one, and the bodies guard the rest.
    //
    // The type slots are ordinary kind expectations rather than the definition's raw-carrier
    // union: a declared member's operand and result resolve once, where they are written.
    let head_slot = |name: &StaticName<ValueSymbol>, role: &'static str| {
        arg_labeled(registries, name, KType::of_kind(KKind::AnyType), role)
    };
    let head_binary_sig = || {
        vec![
            kw(registries, "OP"),
            arg(registries, &SLOTS.symbol, KType::KEXPRESSION),
            kw(registries, "OVER"),
            head_slot(&SLOTS.operand, OPERAND_SLOT),
        ]
    };
    let head_binary_result_sig = || {
        let mut elements = head_binary_sig();
        elements.push(kw(registries, "->"));
        elements.push(head_slot(&SLOTS.return_type, RESULT_SLOT));
        elements
    };
    let head_unary_sig = || {
        let mut elements = vec![kw(registries, "UNARY")];
        elements.append(&mut head_binary_result_sig());
        elements
    };
    let carrier = type_carrier_union(registries);
    register_builtin(
        scope,
        sig(KType::ANY, binary(carrier)),
        body_binary,
        registries,
        gate,
    );
    register_builtin(
        scope,
        combined(registries, binary(carrier)),
        body_binary_combined,
        registries,
        gate,
    );
    register_builtin(
        scope,
        sig(KType::ANY, binary_with_result(carrier, carrier)),
        body_binary,
        registries,
        gate,
    );
    register_builtin(
        scope,
        combined(registries, binary_with_result(carrier, carrier)),
        body_binary_combined,
        registries,
        gate,
    );
    register_builtin(
        scope,
        sig(KType::ANY, unary(carrier, carrier)),
        body_unary,
        registries,
        gate,
    );
    register_builtin(
        scope,
        combined(registries, unary(carrier, carrier)),
        body_unary_combined,
        registries,
        gate,
    );
    // A head declares a member rather than a value, so its declared return is the member's own
    // function type — `KType::ANY`, like every other declarator whose result only exists once its
    // signature is known.
    register_builtin(
        scope,
        sig(KType::ANY, head_binary_sig()),
        head_binary,
        registries,
        gate,
    );
    register_builtin(
        scope,
        sig(KType::ANY, head_binary_result_sig()),
        head_binary_with_result,
        registries,
        gate,
    );
    register_builtin(
        scope,
        sig(KType::ANY, head_unary_sig()),
        head_unary,
        registries,
        gate,
    );
}

#[cfg(test)]
mod tests;
