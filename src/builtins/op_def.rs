//! `OP #(<sym>) OVER <Operand> = (<body>)` — declare a chainable operator in the enclosing
//! scope. The symbol is **quoted**: `#(+)` is a parse-static
//! [`QuotedExpression`](crate::machine::model::ast::ExpressionPart::QuotedExpression) part, so it
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

use crate::machine::core::kfunction::action::{
    arg_held, require_kexpression, Action, AwaitContinue, BodyCtx, DepPlacement, DepRequest,
    DepTerminal, FinishCtx,
};
use crate::machine::core::kfunction::KFunction;
use crate::machine::core::{BindingIndex, StoredReach};
use crate::machine::execute::StepCarried;
use crate::machine::model::ast::{ExpressionPart, KExpression, TypeIdentifier};
use crate::machine::model::operators::{OperatorGroup, ReductionMode};
use crate::machine::model::types::{ExpressionSignature, KKind, UntypedElement, UntypedKey};
use crate::machine::model::values::CarriedFamily;
use crate::machine::model::{KObject, KType};
use crate::machine::{Body, CarrierWitness, KError, KErrorKind, NodeId, Scope};
use crate::scheduler::DepResults;
use crate::source::Spanned;
use crate::witnessed::Witnessed;

use super::fn_def::return_type::{classify_return_type, extract_type_slot_raw, ReturnTypeState};
use super::resolve_or_await::{classify_type_hit, expect_type_terminal, resolve_at_wake};
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

/// Symbols the `OP` / `GROUP` surface spells with, plus the two ascription sigils. Declaring an
/// operator under one of these would make its own declaration form unreadable. Every other
/// keyword-classified token is a legal operator symbol, including an all-caps alphabetic name
/// (`OP #(MAX) OVER Number` is fine).
const RESERVED_SYMBOLS: [&str; 12] = [
    "OP", "UNARY", "OVER", "GROUP", "FOLD", "PAIRWISE", "LEFT", "RIGHT", "=", "->", ":|", ":!",
];

/// Which surface declared the operator — the one axis the shared body branches on.
#[derive(Clone, Copy, PartialEq, Eq)]
enum OpKind {
    /// `OP #(<sym>) OVER Operand [-> Result] = (<body>)`: binds `left` / `right`.
    Binary,
    /// `UNARY OP #(<sym>) OVER Operand -> Result = (<body>)`: binds `operands`.
    Unary,
}

// ---------- symbol extraction (shared by the body and the binder hook) ----------

/// The operator symbol a quote body carries: exactly one `Keyword` part. The `symbol` slot is
/// typed `:KExpression`, so a `QuotedExpression` part arrives raw and un-dispatched (it makes the
/// declaration a lazy candidate) and its body is read here as data. A multi-part body, a
/// non-keyword token, or a reserved symbol is a shape error.
fn symbol_from_quote_body(inner: &KExpression<'_>) -> Result<String, KError> {
    let [part] = inner.parts.as_slice() else {
        return Err(symbol_shape_error());
    };
    let ExpressionPart::Keyword(sym) = &part.value else {
        return Err(symbol_shape_error());
    };
    if RESERVED_SYMBOLS.contains(&sym.as_str()) {
        return Err(KError::new(KErrorKind::ShapeError(format!(
            "`{sym}` is reserved by the operator-declaration surface and cannot name an operator",
        ))));
    }
    Ok(sym.clone())
}

fn symbol_shape_error() -> KError {
    KError::new(KErrorKind::ShapeError(
        "operator symbol must be one quoted token: `OP #(+) OVER Number = (…)`".to_string(),
    ))
}

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

/// Statement-side symbol read: the declaration's first `QuotedExpression` part. `GROUP` scans its
/// unevaluated body block with this to collect its members; the binder hook uses it to decide
/// whether to install park edges (discarding the diagnostic — the body's own extraction surfaces
/// it).
pub(super) fn symbol_from_parts(expr: &KExpression<'_>) -> Result<String, KError> {
    let quoted = expr
        .parts
        .iter()
        .find_map(|part| match &part.value {
            ExpressionPart::QuotedExpression(inner) => Some(inner.as_ref()),
            _ => None,
        })
        .ok_or_else(symbol_shape_error)?;
    symbol_from_quote_body(quoted)
}

/// True iff the declaration leads with `UNARY`.
fn is_unary_form(expr: &KExpression<'_>) -> bool {
    matches!(
        expr.parts.first().map(|p| &p.value),
        Some(ExpressionPart::Keyword(k)) if k == "UNARY",
    )
}

/// The bucket key a binary use site computes: `[Slot, Keyword(sym), Slot]`.
fn binary_key(sym: &str) -> UntypedKey {
    vec![
        UntypedElement::Slot,
        UntypedElement::Keyword(sym.to_string()),
        UntypedElement::Slot,
    ]
}

/// The bucket key a unary use site computes: `[Keyword(sym), Slot]` — the prefix form
/// `sym [a b c]` and a reduced infix run are the same shape.
fn unary_key(sym: &str) -> UntypedKey {
    vec![
        UntypedElement::Keyword(sym.to_string()),
        UntypedElement::Slot,
    ]
}

/// Submission-time park keys: every bucket this declaration's body registers an overload under, so
/// a later sibling statement using the operator parks on the `OP` slot instead of failing dispatch
/// while the declaration is still finalizing. A `UNARY OP` registers two bodies, so it names two
/// keys.
fn binder_bucket(expr: &KExpression<'_>) -> Option<Vec<UntypedKey>> {
    let sym = symbol_from_parts(expr).ok()?;
    if is_unary_form(expr) {
        Some(vec![unary_key(&sym), binary_key(&sym)])
    } else {
        Some(vec![binary_key(&sym)])
    }
}

// ---------- type slots ----------

/// A type slot's state across the (possible) dep-finish boundary: resolved outright, parked on a
/// still-finalizing type binder, or sub-dispatched as a `:(…)` expression at owned position
/// `owned_pos`.
enum TypeCapture<'a> {
    Done(KType<'a>),
    Park(TypeIdentifier),
    Sub { owned_pos: usize },
}

/// Route one classified type slot into a [`TypeCapture`], accumulating its park producers and its
/// sub-dispatch — whose owned position the capture records — into the deferral lists.
fn capture_type_slot<'a>(
    state: ReturnTypeState<'a>,
    parks: &mut Vec<NodeId>,
    subs: &mut Vec<KExpression<'a>>,
) -> Result<TypeCapture<'a>, KError> {
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

/// The `Done` arm alone — the synchronous path, taken exactly when no slot parked or
/// sub-dispatched.
fn done_type<'a>(capture: TypeCapture<'a>, label: &str) -> Result<KType<'a>, KError> {
    match capture {
        TypeCapture::Done(kt) => Ok(kt),
        _ => Err(KError::new(KErrorKind::ShapeError(format!(
            "{label} is unresolved with no dependency to wait on"
        )))),
    }
}

/// Read a capture back at dep-finish: a parked name re-resolves against the wake-side scope, a
/// sub-dispatched expression reads its terminal's type. That type can embed a borrow into its
/// producer's region, so its carrier's reach is minted into the declaring scope before the type is
/// sealed into the operator's `KFunction` — the same fold `fn_def`'s return-type finish performs.
fn resolve_capture<'a>(
    capture: TypeCapture<'a>,
    fctx: &FinishCtx<'a>,
    results: &DepResults<'_, &DepTerminal<'a>>,
    label: &str,
) -> Result<KType<'a>, KError> {
    match capture {
        TypeCapture::Done(kt) => Ok(kt),
        TypeCapture::Park(te) => resolve_at_wake(fctx.scope, label, |s| {
            classify_type_hit(s.resolve_type_identifier(&te, None))
        }),
        TypeCapture::Sub { owned_pos } => {
            let (kt, carrier) = expect_type_terminal(results, owned_pos, label)?;
            let _ = fctx.scope.host_reach_of(carrier);
            Ok(kt)
        }
    }
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
    ));
    let result_state = if has_result {
        let raw = crate::try_action!(extract_type_slot_raw(ctx.args, "return_type", RESULT_SLOT));
        Some(crate::try_action!(classify_return_type(
            raw,
            &[],
            ctx.scope,
            ctx.chain.clone(),
            RESULT_SLOT,
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
        let operand = crate::try_action!(done_type(operand_capture, OPERAND_SLOT));
        let result = match result_capture {
            Some(capture) => Some(crate::try_action!(done_type(capture, RESULT_SLOT))),
            None => None,
        };
        return op_action(plan.finalize(ctx.scope, operand, result));
    }
    // Dep order is `[park… ++ sub…]` — the harness owns the subs in declaration order, the order
    // `capture_type_slot` recorded their positions in.
    let mut deps: Vec<DepRequest<'a>> = parks.into_iter().map(DepRequest::Existing).collect();
    deps.extend(subs.into_iter().map(|expr| DepRequest::Dispatch {
        expr,
        placement: DepPlacement::OwnScope,
    }));
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
        op_action(plan.finalize(fctx.scope, operand, result))
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
        operand: KType<'a>,
        result: Option<KType<'a>>,
    ) -> Result<Witnessed<CarriedFamily, CarrierWitness>, KError> {
        let OpPlan {
            sym,
            kind,
            body_expr,
            in_group,
            bind_index,
        } = self;
        let (elements, result_type, mode) = match kind {
            OpKind::Binary => (
                vec![
                    arg(LEFT, operand.clone()),
                    kw(&sym),
                    arg(RIGHT, operand.clone()),
                ],
                result.unwrap_or_else(|| operand.clone()),
                ReductionMode::FoldLeft,
            ),
            OpKind::Unary => (
                vec![
                    kw(&sym),
                    arg(OPERANDS, KType::list(Box::new(operand.clone()))),
                ],
                result.ok_or_else(|| {
                    KError::new(KErrorKind::ShapeError(
                        "UNARY OP requires an explicit `-> Result`".to_string(),
                    ))
                })?,
                ReductionMode::Unary,
            ),
        };
        let (obj, stored) = register_body(
            scope,
            &sym,
            sig(result_type.clone(), elements),
            body_expr,
            bind_index,
        )?;
        if kind == OpKind::Unary {
            // The binary bridge: `a ~ b` names one keyword, so it dispatches as a plain keyworded
            // call, not an operator chain — without a two-operand body it would simply miss. Its
            // body is the AST `sym [left right]`, the shape a reduced run takes, so both surfaces
            // land on the one list body the user wrote.
            let bridge = vec![
                arg(LEFT, operand.clone()),
                kw(&sym),
                arg(RIGHT, operand.clone()),
            ];
            register_body(
                scope,
                &sym,
                sig(result_type, bridge),
                bridge_body(&sym),
                bind_index,
            )?;
        }
        if !in_group {
            let members = std::iter::once(sym.clone()).collect();
            let group = scope
                .brand()
                .alloc_operator_group(OperatorGroup::new(members, mode));
            scope.register_group_under_all_subsets(&[sym.as_str()], group, bind_index)?;
        }
        Ok(scope.resident_value_carrier(obj, stored))
    }
}

/// Allocate one operator body as a `KFunction` capturing `scope`, and register it in `scope`'s
/// function bucket through the operator door. The `KFunction` is allocated into `scope`'s own
/// region, so the checked seal always passes and the paired token carries the home-borrow bit the
/// audit walk derives (the captured `&Scope` into home).
fn register_body<'a>(
    scope: &'a Scope<'a>,
    sym: &str,
    signature: ExpressionSignature<'a>,
    body_expr: KExpression<'a>,
    bind_index: BindingIndex,
) -> Result<(&'a KObject<'a>, StoredReach<'a>), KError> {
    let f: &'a KFunction<'a> = scope.brand().alloc_function(KFunction::new(
        signature,
        Body::UserDefined(body_expr),
        scope,
        None,
        None,
    ));
    let (obj, stored) = scope
        .alloc_object_checked_stored(KObject::KFunction(f))
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
fn type_carriers<'a>() -> [KType<'a>; 2] {
    [KType::OfKind(KKind::ProperType), KType::SigiledTypeExpr]
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    use crate::builtins::register_builtin_full;

    // Declared return is `KType::Any`: an operator declaration evaluates to the function it
    // synthesizes, whose structural type only exists once its signature is known.
    let binary = |operand: KType<'a>| {
        sig(
            KType::Any,
            vec![
                kw("OP"),
                arg("symbol", KType::KExpression),
                kw("OVER"),
                arg("operand", operand),
                kw("="),
                arg("body", KType::KExpression),
            ],
        )
    };
    let binary_with_result = |operand: KType<'a>, result: KType<'a>| {
        sig(
            KType::Any,
            vec![
                kw("OP"),
                arg("symbol", KType::KExpression),
                kw("OVER"),
                arg("operand", operand),
                kw("->"),
                arg("return_type", result),
                kw("="),
                arg("body", KType::KExpression),
            ],
        )
    };
    let unary = |operand: KType<'a>, result: KType<'a>| {
        sig(
            KType::Any,
            vec![
                kw("UNARY"),
                kw("OP"),
                arg("symbol", KType::KExpression),
                kw("OVER"),
                arg("operand", operand),
                kw("->"),
                arg("return_type", result),
                kw("="),
                arg("body", KType::KExpression),
            ],
        )
    };
    let unary_missing_result = |operand: KType<'a>| {
        sig(
            KType::Any,
            vec![
                kw("UNARY"),
                kw("OP"),
                arg("symbol", KType::KExpression),
                kw("OVER"),
                arg("operand", operand),
                kw("="),
                arg("body", KType::KExpression),
            ],
        )
    };

    for operand in type_carriers() {
        register_builtin_full(
            scope,
            "OP",
            binary(operand.clone()),
            body_binary,
            None,
            Some(binder_bucket),
        );
        register_builtin_full(
            scope,
            "OP",
            unary_missing_result(operand.clone()),
            body_unary_missing_result,
            None,
            None,
        );
        for result in type_carriers() {
            register_builtin_full(
                scope,
                "OP",
                binary_with_result(operand.clone(), result.clone()),
                body_binary,
                None,
                Some(binder_bucket),
            );
            register_builtin_full(
                scope,
                "OP",
                unary(operand.clone(), result),
                body_unary,
                None,
                Some(binder_bucket),
            );
        }
    }
}

#[cfg(test)]
mod tests;
