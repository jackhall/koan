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

use crate::machine::model::CarriedFamily;
use crate::machine::model::TypeRegistry;
use crate::machine::model::{binary_key, unary_key, OperatorGroup, ReductionMode};
use crate::machine::model::{ExpressionPart, KExpression, TypeIdentifier};
use crate::machine::model::{ExpressionSignature, KKind};
use crate::machine::model::{KObject, KType};
use crate::machine::KFunction;
use crate::machine::StepCarried;
use crate::machine::{
    arg_held, require_kexpression, Action, AwaitContinue, BodyCtx, DepPlacement, DepTerminal,
    FinishCtx, OwnedDispatch,
};
use crate::machine::{BindingIndex, StoredReach};
use crate::machine::{Body, CarrierWitness, KError, KErrorKind, NodeId, Scope};
use crate::scheduler::DepResults;
use crate::scheduler::Deps;
use crate::source::Spanned;
use crate::witnessed::Witnessed;

use super::fn_def::return_type::{classify_return_type, extract_type_slot_raw, ReturnTypeState};
use super::resolve_or_await::{expect_type_terminal, resolve_at_wake};
use super::{arg, kw, sig};

/// The two operand names a binary operator body binds. A pairwise group's combiner is itself an
/// `OP`, so it binds the same pair — but positionally, by the infix shape the reducer synthesizes,
/// not by name.
const LEFT: &str = "left";
const RIGHT: &str = "right";
/// The single parameter a unary operator body binds: the whole run as one list.
const OPERANDS: &str = "operands";

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

use crate::machine::model::binder::op_def_binder_bucket as binder_bucket;
pub(super) use crate::machine::model::symbol_from_parts;
use crate::machine::model::symbol_from_quote_body;

/// Body-side symbol read: a quoted slot's raw `KObject::KExpression` is the quote body. Shared with
/// `GROUP`, whose pairwise `combiner` slot names an operator the same way (`super::group_def`).
pub(super) fn symbol_from_slot(
    args: &KObject<'_>,
    builtin: &str,
    slot: &str,
) -> Result<String, KError> {
    let quoted = require_kexpression(args, builtin, slot)?;
    symbol_from_quote_body(&quoted)
}

// ---------- type slots ----------

/// A type slot's state across the (possible) dep-finish boundary: resolved outright, parked on a
/// still-finalizing type binder, or sub-dispatched as a `:(…)` expression at owned position
/// `owned_pos`.
enum TypeCapture {
    Done(KType),
    Park(TypeIdentifier),
    Sub { owned_pos: usize },
}

/// Route one classified type slot into a [`TypeCapture`], accumulating its park producers and its
/// sub-dispatch — whose owned position the capture records — into the deferral lists.
fn capture_type_slot<'a>(
    state: ReturnTypeState<'a>,
    parks: &mut Vec<NodeId>,
    subs: &mut Vec<KExpression<'a>>,
) -> Result<TypeCapture, KError> {
    match state {
        ReturnTypeState::Done(kt) => Ok(TypeCapture::Done(kt)),
        ReturnTypeState::Pending { te, producers } => {
            parks.extend(producers);
            Ok(TypeCapture::Park(te))
        }
        ReturnTypeState::ExprToSubDispatch(expr) => {
            subs.push(expr);
            Ok(TypeCapture::Sub {
                owned_pos: subs.len() - 1,
            })
        }
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
fn checked_value_type(kt: KType, label: &str, types: &TypeRegistry) -> Result<KType, KError> {
    match crate::machine::model::unsaturated_constructor_message(kt, &format!("the {label}"), types)
    {
        Some(message) => Err(KError::new(KErrorKind::ShapeError(message))),
        None => Ok(kt),
    }
}

/// The `Done` arm alone — the synchronous path, taken exactly when no slot parked or
/// sub-dispatched.
fn done_type(capture: TypeCapture, label: &str, types: &TypeRegistry) -> Result<KType, KError> {
    match capture {
        TypeCapture::Done(kt) => checked_value_type(kt, label, types),
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
    results: &DepResults<'_, &DepTerminal<'a>>,
    label: &str,
) -> Result<KType, KError> {
    let kt = match capture {
        TypeCapture::Done(kt) => kt,
        TypeCapture::Park(te) => resolve_at_wake(fctx.scope, label, |s| {
            s.resolve_type_identifier(&te, None, fctx.types)
        })?,
        TypeCapture::Sub { owned_pos } => {
            expect_type_terminal(results, owned_pos, label, fctx.types)?
        }
    };
    checked_value_type(kt, label, fctx.types)
}

// ---------- body ----------

/// The `OP` body: extract and validate the symbol, check the group context, elaborate the operand
/// (and any explicit result) type, then synthesize and register the operator's `KFunction`(s). A
/// type slot naming a still-finalizing type binder — or spelled as a `:(…)` expression that has to
/// sub-dispatch — defers the whole build to a dep-finish.
fn build<'a>(ctx: &BodyCtx<'a, '_>, kind: OpKind) -> Action<'a> {
    let sym = crate::try_action!(symbol_from_slot(ctx.args, "OP", "symbol"));
    let body_expr = crate::try_action!(require_kexpression(ctx.args, "OP", "body"));
    let has_result = arg_held(ctx.args, "return_type").is_some();
    let group = ctx.scope.nearest_group_context();
    crate::try_action!(check_group_context(kind, has_result, group, &sym));

    let operand_raw = crate::try_action!(extract_type_slot_raw(ctx.args, "operand", OPERAND_SLOT));
    let operand_state = crate::try_action!(classify_return_type(
        operand_raw,
        &[],
        ctx.scope,
        ctx.chain.clone(),
        OPERAND_SLOT,
        ctx.types,
    ));
    let result_state = if has_result {
        let raw = crate::try_action!(extract_type_slot_raw(ctx.args, "return_type", RESULT_SLOT));
        Some(crate::try_action!(classify_return_type(
            raw,
            &[],
            ctx.scope,
            ctx.chain.clone(),
            RESULT_SLOT,
            ctx.types,
        )))
    } else {
        None
    };

    let mut parks: Vec<NodeId> = Vec::new();
    let mut subs: Vec<KExpression<'a>> = Vec::new();
    let operand_capture =
        crate::try_action!(capture_type_slot(operand_state, &mut parks, &mut subs));
    let result_capture = match result_state {
        Some(state) => Some(crate::try_action!(capture_type_slot(
            state, &mut parks, &mut subs
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
    };
    if parks.is_empty() && subs.is_empty() {
        let operand = crate::try_action!(done_type(operand_capture, OPERAND_SLOT, ctx.types));
        let result = match result_capture {
            Some(capture) => Some(crate::try_action!(done_type(
                capture,
                RESULT_SLOT,
                ctx.types
            ))),
            None => None,
        };
        return op_action(plan.finalize(ctx.scope, operand, result, ctx.types));
    }
    // Builds the structural `[park… ++ sub…]` split directly: parks first, then the subs owned in
    // declaration order — the order `capture_type_slot` recorded their positions in.
    let mut deps = Deps::from_parks(parks);
    for expr in subs {
        deps.own(OwnedDispatch {
            expr,
            placement: DepPlacement::OwnScope,
        });
    }
    let finish: AwaitContinue<'a> = Box::new(move |fctx, results| {
        let operand = crate::try_action!(resolve_capture(
            operand_capture,
            fctx,
            &results,
            OPERAND_SLOT
        ));
        let result = match result_capture {
            Some(capture) => Some(crate::try_action!(resolve_capture(
                capture,
                fctx,
                &results,
                RESULT_SLOT
            ))),
            None => None,
        };
        op_action(plan.finalize(fctx.scope, operand, result, fctx.types))
    });
    Action::AwaitDeps { deps, finish }
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
    group: Option<&OperatorGroup>,
    sym: &str,
) -> Result<(), KError> {
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
struct OpPlan<'a> {
    sym: String,
    kind: OpKind,
    body_expr: KExpression<'a>,
    /// Inside a `GROUP` body the group owns the registry entry for every member, so the declaration
    /// writes the function bucket only.
    in_group: bool,
    bind_index: BindingIndex,
}

impl<'a> OpPlan<'a> {
    /// Synthesize the operator's `KFunction`(s), register them in `scope`'s function bucket, and —
    /// outside a group — write the size-1 registry entry that makes a run of three or more operands
    /// reduce. Returns the declared function's value.
    fn finalize(
        self,
        scope: &'a Scope<'a>,
        operand: KType,
        result: Option<KType>,
        types: &TypeRegistry,
    ) -> Result<Witnessed<CarriedFamily, CarrierWitness>, KError> {
        let OpPlan {
            sym,
            kind,
            body_expr,
            in_group,
            bind_index,
        } = self;
        let (obj, stored) = match kind {
            OpKind::Binary => {
                let elements = vec![arg(LEFT, operand), kw(&sym), arg(RIGHT, operand)];
                let result_type = result.unwrap_or(operand);
                let registered = register_body(
                    scope,
                    &sym,
                    sig(result_type, elements),
                    Body::UserDefined(body_expr),
                    bind_index,
                    types,
                )?;
                if !in_group {
                    let members = std::iter::once(sym.clone()).collect();
                    let group = scope
                        .brand()
                        .alloc_operator_group(OperatorGroup::new(members, ReductionMode::FoldLeft));
                    scope.register_group_under_all_subsets(&[sym.as_str()], group, bind_index)?;
                }
                registered
            }
            OpKind::Unary => {
                let result_type = result.ok_or_else(|| {
                    KError::new(KErrorKind::ShapeError(
                        "UNARY OP requires an explicit `-> Result`".to_string(),
                    ))
                })?;
                let list_signature = sig(
                    result_type,
                    vec![kw(&sym), arg(OPERANDS, types.list(operand))],
                );
                // The binary bridge: `a ~ b` names one keyword, so it dispatches as a plain
                // keyworded call, not an operator chain — without a two-operand body it would
                // simply miss. Its body is the AST `sym [left right]`, the shape a reduced run
                // takes, so both surfaces land on the one list body the user wrote.
                let bridge_signature = sig(
                    result_type,
                    vec![arg(LEFT, operand), kw(&sym), arg(RIGHT, operand)],
                );
                // `check_group_context` rejects `UNARY OP` inside a `GROUP` before the plan is
                // built, so `in_group` cannot hold here; the door asserts that rather than take
                // it on trust, since it writes the single-member group unconditionally.
                register_unary_operator(
                    scope,
                    &sym,
                    OperatorForm {
                        signature: list_signature,
                        body: Body::UserDefined(body_expr),
                    },
                    OperatorForm {
                        signature: bridge_signature,
                        body: Body::UserDefined(bridge_body(&sym)),
                    },
                    in_group,
                    bind_index,
                    types,
                )?
            }
        };
        Ok(scope.resident_value_carrier(obj, stored))
    }
}

/// One dispatchable form of an operator: the signature naming a surface, and the body that surface
/// reaches. A unary operator is registered from two — the list form and the binary form.
pub(super) struct OperatorForm<'a> {
    pub signature: ExpressionSignature<'a>,
    pub body: Body<'a>,
}

/// Register the fixed triple every unary operator consists of: the list-form overload under
/// [`unary_key`], the binary-form overload under [`binary_key`], and the size-1
/// [`ReductionMode::Unary`] group entry (key derived through
/// [`Scope::register_group_under_all_subsets`]). The bodies are the caller's own — `UNARY OP`
/// synthesizes koan-AST bodies, the builtin `|` supplies native ones. Returns the list-form
/// function's object and stored reach: the list body is the operator's primary value.
///
/// Registration derives each bucket key from the signature the caller hands in, so a caller that
/// spells a signature the use site never computes would register into a bucket no koan expression
/// reaches — the operator would silently never dispatch. The signature asserts close that channel;
/// a mismatch can only come from crate code, never from koan source.
///
/// `in_group` is the caller's group context, and must be `false`: a single-member group is the only
/// group a unary operator can be in, because its reduction hands the whole run to one body as a
/// single list, which presupposes the run names no other operator. The door writes that group
/// unconditionally, so it asserts the context rather than trusting it — a grouped caller would
/// write a size-1 `Unary` record under the very key its `GROUP` already claims.
pub(super) fn register_unary_operator<'a>(
    scope: &'a Scope<'a>,
    sym: &str,
    list: OperatorForm<'a>,
    binary: OperatorForm<'a>,
    in_group: bool,
    bind_index: BindingIndex,
    types: &TypeRegistry,
) -> Result<(&'a KObject<'a>, StoredReach<'a>), KError> {
    let OperatorForm {
        signature: list_signature,
        body: list_body,
    } = list;
    let OperatorForm {
        signature: binary_signature,
        body: binary_body,
    } = binary;
    assert_eq!(
        list_signature.untyped_key(),
        unary_key(sym),
        "unary operator `{sym}`: the list-form signature must key the bucket a reduced run or a \
         prefix use computes",
    );
    assert_eq!(
        binary_signature.untyped_key(),
        binary_key(sym),
        "unary operator `{sym}`: the binary-form signature must key the bucket a two-operand use \
         computes",
    );
    assert!(
        !in_group,
        "unary operator `{sym}`: a unary operator chains with nothing, so it can only be its own \
         single-member group",
    );
    // The list body first: its function is the operator's primary value, the one an `OP`
    // declaration evaluates to.
    let (obj, stored) = register_body(scope, sym, list_signature, list_body, bind_index, types)?;
    register_body(scope, sym, binary_signature, binary_body, bind_index, types)?;
    let members = std::iter::once(sym.to_string()).collect();
    let group = scope
        .brand()
        .alloc_operator_group(OperatorGroup::new(members, ReductionMode::Unary));
    scope.register_group_under_all_subsets(&[sym], group, bind_index)?;
    Ok((obj, stored))
}

/// Allocate one operator body as a `KFunction` capturing `scope`, and register it in `scope`'s
/// function bucket through the operator door. The `KFunction` is allocated into `scope`'s own
/// region, so the checked seal always passes and the paired token carries the home-borrow bit the
/// audit walk derives (the captured `&Scope` into home).
fn register_body<'a>(
    scope: &'a Scope<'a>,
    sym: &str,
    signature: ExpressionSignature<'a>,
    body: Body<'a>,
    bind_index: BindingIndex,
    types: &TypeRegistry,
) -> Result<(&'a KObject<'a>, StoredReach<'a>), KError> {
    let f: &'a KFunction<'a> = scope
        .brand()
        .alloc_function(KFunction::new(signature, body, scope, None, None, types));
    let (obj, stored) = scope
        .alloc_object_checked_stored(KObject::KFunction(f), types)
        .expect("f was just allocated into scope's own region");
    scope.register_operator_function(sym.to_string(), f, obj, bind_index)?;
    Ok((obj, stored))
}

/// The bridge body `sym [left right]` — a keyword-first call over a two-element list literal, which
/// dispatches straight to the unary operator's list body. Each parameter is its own one-part
/// expression element: a list literal interns a bare `Identifier` element as a symbol rather than
/// resolving it, so the two operands ride in as element expressions (exactly as a reduced infix run
/// carries its named operands).
fn bridge_body<'a>(sym: &str) -> KExpression<'a> {
    let operand = |name: &str| {
        ExpressionPart::Expression(Box::new(KExpression::new(vec![Spanned::bare(
            ExpressionPart::Identifier(name.to_string()),
        )])))
    };
    KExpression::new(vec![
        Spanned::bare(ExpressionPart::Keyword(sym.to_string())),
        Spanned::bare(ExpressionPart::ListLiteral(vec![
            operand(LEFT),
            operand(RIGHT),
        ])),
    ])
}

/// Seal a finalize result as the slot's terminal — the operator function value, built witnessed in
/// its declaring scope's region.
fn op_action<'a>(result: Result<Witnessed<CarriedFamily, CarrierWitness>, KError>) -> Action<'a> {
    match result {
        Ok(witnessed) => Action::Done(Ok(StepCarried::born(witnessed))),
        Err(e) => Action::Done(Err(e)),
    }
}

fn body_binary<'a>(ctx: &BodyCtx<'a, '_>) -> Action<'a> {
    build(ctx, OpKind::Binary)
}

fn body_unary<'a>(ctx: &BodyCtx<'a, '_>) -> Action<'a> {
    build(ctx, OpKind::Unary)
}

/// `UNARY OP #(<sym>) OVER Operand = (<body>)` — the result segment is mandatory: a unary body
/// consumes a whole list of operands, so its result type is not its operand type and there is
/// nothing to default it to. This overload exists only to say so; without it the shape is a bare
/// dispatch miss.
fn body_unary_missing_result<'a>(ctx: &BodyCtx<'a, '_>) -> Action<'a> {
    let sym = crate::try_action!(symbol_from_slot(ctx.args, "OP", "symbol"));
    Action::Done(Err(KError::new(KErrorKind::ShapeError(format!(
        "`UNARY OP #({sym})` must declare its result type: \
         `UNARY OP #({sym}) OVER <Operand> -> <Result> = (…)`",
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

pub fn register<'a>(scope: &'a Scope<'a>, types: &TypeRegistry) {
    use crate::builtins::register_builtin_full;

    // Declared return is `KType::ANY`: an operator declaration evaluates to the function it
    // synthesizes, whose structural type only exists once its signature is known.
    let binary = |operand: KType| {
        sig(
            KType::ANY,
            vec![
                kw("OP"),
                arg("symbol", KType::KEXPRESSION),
                kw("OVER"),
                arg("operand", operand),
                kw("="),
                arg("body", KType::KEXPRESSION),
            ],
        )
    };
    let binary_with_result = |operand: KType, result: KType| {
        sig(
            KType::ANY,
            vec![
                kw("OP"),
                arg("symbol", KType::KEXPRESSION),
                kw("OVER"),
                arg("operand", operand),
                kw("->"),
                arg("return_type", result),
                kw("="),
                arg("body", KType::KEXPRESSION),
            ],
        )
    };
    let unary = |operand: KType, result: KType| {
        sig(
            KType::ANY,
            vec![
                kw("UNARY"),
                kw("OP"),
                arg("symbol", KType::KEXPRESSION),
                kw("OVER"),
                arg("operand", operand),
                kw("->"),
                arg("return_type", result),
                kw("="),
                arg("body", KType::KEXPRESSION),
            ],
        )
    };
    let unary_missing_result = |operand: KType| {
        sig(
            KType::ANY,
            vec![
                kw("UNARY"),
                kw("OP"),
                arg("symbol", KType::KEXPRESSION),
                kw("OVER"),
                arg("operand", operand),
                kw("="),
                arg("body", KType::KEXPRESSION),
            ],
        )
    };

    for operand in type_carriers() {
        register_builtin_full(
            scope,
            "OP",
            binary(operand),
            body_binary,
            None,
            Some(binder_bucket),
            types,
        );
        register_builtin_full(
            scope,
            "OP",
            unary_missing_result(operand),
            body_unary_missing_result,
            None,
            None,
            types,
        );
        for result in type_carriers() {
            register_builtin_full(
                scope,
                "OP",
                binary_with_result(operand, result),
                body_binary,
                None,
                Some(binder_bucket),
                types,
            );
            register_builtin_full(
                scope,
                "OP",
                unary(operand, result),
                body_unary,
                None,
                Some(binder_bucket),
                types,
            );
        }
    }
}

#[cfg(test)]
mod tests;
