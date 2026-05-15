mod signature;

use crate::runtime::model::{ExpressionSignature, KObject, KType, SignatureElement};
use crate::runtime::machine::{ArgumentBundle, Body, BodyResult, CombineFinish, KError, KErrorKind, KFunction, Scope, SchedulerHandle};
use crate::runtime::model::types::{elaborate_type_expr, DeferredReturn, ElabResult, Elaborator, ReturnType};

use crate::runtime::machine::kfunction::argument_bundle::{
    extract_kexpression, extract_ktype, extract_type_name_ref,
};
use crate::ast::ExpressionPart;
use super::{arg, err, kw, register_builtin_with_pre_run, sig};

use signature::ParamListOutcome;

pub(crate) use signature::pre_run;

/// Scan a parser-preserved `TypeExpr` for any leaf whose name matches one of
/// `param_names`. Drives the Stage B `Resolved` vs `Deferred(TypeExpr)` decision
/// at FN-def time: a match short-circuits the eager-elaborate path (which would
/// fail or produce the wrong answer against the FN's outer scope, where the
/// parameter is unbound by definition).
fn type_expr_references_any(te: &crate::ast::TypeExpr, param_names: &[String]) -> bool {
    use crate::ast::TypeParams;
    if param_names.iter().any(|n| n == &te.name) {
        return true;
    }
    match &te.params {
        TypeParams::None => false,
        TypeParams::List(items) => items.iter().any(|t| type_expr_references_any(t, param_names)),
        TypeParams::Function { args, ret } => {
            args.iter().any(|t| type_expr_references_any(t, param_names))
                || type_expr_references_any(ret, param_names)
        }
    }
}

/// Companion scan for the parens-form return-type carrier (overload 2). Walks a
/// `KExpression`'s parts recursively into nested `Expression`, `ListLiteral`, and
/// `DictLiteral` parts and returns `true` iff any leaf `Identifier(name)` or
/// `Type(TypeExpr { name, .. })` matches one of `param_names`. Mirrors the
/// recursive walk in `kfunction::invoke::substitute_part`.
fn kexpression_references_any<'a>(
    expr: &crate::ast::KExpression<'a>,
    param_names: &[String],
) -> bool {
    expr.parts.iter().any(|p| part_references_any(p, param_names))
}

fn part_references_any<'a>(
    part: &crate::ast::ExpressionPart<'a>,
    param_names: &[String],
) -> bool {
    match part {
        ExpressionPart::Identifier(name) => param_names.iter().any(|n| n == name),
        ExpressionPart::Type(t) => type_expr_references_any(t, param_names),
        ExpressionPart::Expression(boxed) => kexpression_references_any(boxed, param_names),
        ExpressionPart::ListLiteral(items) => {
            items.iter().any(|p| part_references_any(p, param_names))
        }
        ExpressionPart::DictLiteral(pairs) => pairs.iter().any(|(k, v)| {
            part_references_any(k, param_names) || part_references_any(v, param_names)
        }),
        _ => false,
    }
}

/// Collect parameter names from a signature's `SignatureElement` list. Used by
/// the parameter-name scan during Stage B's `Resolved` vs `Deferred` decision.
fn collect_param_names(elements: &[SignatureElement]) -> Vec<String> {
    elements
        .iter()
        .filter_map(|el| match el {
            SignatureElement::Argument(a) => Some(a.name.clone()),
            _ => None,
        })
        .collect()
}

/// `FN <signature:KExpression> -> <return_type:Type> = <body:KExpression>` — the user-defined
/// function constructor. The signature and body slots are `KType::KExpression`, so the parser's
/// parenthesized sub-expressions match and ride through as data. Two return-type slot
/// overloads share this body:
///
///   1. `KType::TypeExprRef` — `Type(_)` carrier (`-> Number`, `-> List<Str>`, `-> Er`,
///      `-> SomeUserBound`). The carrier's structured `TypeExpr` is preserved through the
///      bundle for either eager elaboration (no parameter refs) or `ReturnType::Deferred(TypeExpr)`
///      (parameter refs detected — Stage B).
///   2. `KType::KExpression` — `Expression(_)` carrier (`-> (MODULE_TYPE_OF Er Type)`,
///      `-> (SIG_WITH Set ((Elt: Er)))`). Captured raw so the parens-form survives FN-def
///      without sub-dispatching against the outer scope where the parameter is unbound.
///      Routes either to `ReturnType::Deferred(Expression)` (parameter refs detected) or
///      to a `defer_via_combine` sub-Dispatch (no parameter refs, lifted to `Resolved`).
///
/// The captured signature `KExpression` is structurally inspected here — never dispatched —
/// to derive the registered function's `ExpressionSignature`. The body `KExpression` is
/// captured raw; `KFunction::invoke` substitutes parameter values into it and re-dispatches
/// at call time.
///
/// Signature shape: each `Keyword` part becomes a `SignatureElement::Keyword` (a fixed token
/// in the call site); each `Identifier` must be followed by `: Type` to form an `Argument`
/// triple, producing a typed parameter slot the caller supplies. Per-param types are
/// dispatch-checked via `Argument::matches`, so a call whose argument types don't satisfy
/// the signature surfaces as `DispatchFailed: no matching function` (same path as builtins);
/// overloads on different parameter types route to the right body via slot-specificity.
/// At least one `Keyword` is required so the signature has a fixed token to dispatch on —
/// a signature of all-Argument slots would shadow `value_lookup`/`value_pass`. Bare
/// identifiers (without `: Type`), stray type tokens, literals, and nested expressions in
/// the signature are rejected with a `ShapeError`.
pub fn body<'a>(
    scope: &'a Scope<'a>,
    sched: &mut dyn SchedulerHandle<'a>,
    mut bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let signature_expr = match extract_kexpression(&mut bundle, "signature") {
        Some(e) => e,
        None => {
            return err(KError::new(KErrorKind::ShapeError(
                "FN signature slot must be a parenthesized expression".to_string(),
            )));
        }
    };
    // The return-type slot accepts three carrier shapes (matching the two FN overloads):
    //
    //  * `KObject::KTypeValue(kt)` — overload 1, eager-resolved leaf or structural type
    //    (`Number`, `List<Str>`, `(SIG_WITH Set ((Elt: Number)))` after construction-time
    //    sub-Dispatch). Lifted directly into `Resolved(kt)`.
    //  * `KObject::TypeNameRef(t, _)` — overload 1, bare-leaf carrier the parser created
    //    because the name isn't in `KType::from_name`'s table (`Point`, `IntOrd`,
    //    `MyList`). Walked by the elaborator below.
    //  * `KObject::KExpression(e)` — overload 2 (Stage B), parens-form return type
    //    (`(MODULE_TYPE_OF Er Type)`, `(SIG_WITH Set ((Elt: Er)))`) captured raw so the
    //    expression survives FN-def without sub-dispatching against the outer scope where
    //    the parameter name is unbound.
    enum ReturnTypeRaw<'a> {
        Resolved(KType),
        TypeExprCarrier(crate::ast::TypeExpr),
        ExprCarrier(crate::ast::KExpression<'a>),
    }
    let return_type_raw = match bundle.get("return_type") {
        Some(KObject::KTypeValue(_)) => match extract_ktype(&mut bundle, "return_type") {
            Some(t) => ReturnTypeRaw::Resolved(t),
            None => unreachable!("get(KTypeValue) then extract_ktype must succeed"),
        },
        Some(KObject::TypeNameRef(_, _)) => match extract_type_name_ref(&mut bundle, "return_type") {
            Some(te) => ReturnTypeRaw::TypeExprCarrier(te),
            None => unreachable!("get(TypeNameRef) then extract_type_name_ref must succeed"),
        },
        Some(KObject::KExpression(_)) => match extract_kexpression(&mut bundle, "return_type") {
            Some(e) => ReturnTypeRaw::ExprCarrier(e),
            None => unreachable!("get(KExpression) then extract_kexpression must succeed"),
        },
        _ => {
            return err(KError::new(KErrorKind::ShapeError(
                "FN return-type slot must be a type expression (e.g. `Number`, `List<Str>`)"
                    .to_string(),
            )));
        }
    };
    let body_expr = match extract_kexpression(&mut bundle, "body") {
        Some(e) => e,
        None => {
            return err(KError::new(KErrorKind::ShapeError(
                "FN body slot must be a parenthesized expression".to_string(),
            )));
        }
    };
    // Multi-statement body desugar: a body whose parts are entirely sub-expressions
    // (`((a) (b) (c))`) right-folds into a chain of `CONS` calls so the scheduler sees a
    // single tail-callable expression. See [`super::cons`] for the contract — TCO holds
    // on the last statement, backward refs across statements work, forward refs do not.
    let body_expr = super::cons::fold_multi_statement(body_expr);

    // Module-system functor-params Stage B: parameter-name scan. Extract parameter
    // names from the signature shape before any elaboration runs, then scan the
    // return-type carrier for any leaf matching a parameter name. A match short-
    // circuits the eager-elaborate path — the return type becomes
    // `ReturnType::Deferred(_)`, carried verbatim through to `KFunction::invoke`,
    // which re-elaborates against the per-call scope where Stage A's dual-write has
    // installed the parameter's type-language identity.
    //
    // The scan reads `signature_expr` raw rather than the elaborated `SignatureElement`
    // list so we can decide before triggering the elaborator's parking machinery — a
    // parked-on-placeholder param type still contributes its name to the scan.
    let param_names = signature::collect_param_names_from_signature(&signature_expr);

    // Phase-3 elaborator: parameter-type leaf names park on outstanding type-binding
    // placeholders, so a `LET MyList = (LIST_OF Number)` dispatched in the same batch
    // wakes the FN's signature elaboration when its body finalizes.
    let mut elaborator = Elaborator::new(scope);

    // Step 1: decide the return-type carrier shape.
    //
    //  * `Deferred(_)` — parameter-name leaf detected in the carrier; the per-call
    //    elaboration runs at the dispatch boundary (see `KFunction::invoke`). Skip
    //    the outer-scope elaborator entirely — running it would surface an `Unbound`
    //    because the parameter is by construction not in the FN's lexical scope.
    //  * `Done(kt)` — fully resolved at FN-def (covers most cases).
    //  * `Pending { te, producers }` — bare-leaf elaboration parked on a placeholder
    //    (forward-LET case); resumed via Combine wake against the now-final scope.
    //  * `ExprSubDispatched { id }` — overload-2 no-parameter-reference path: the
    //    return-type expression sub-dispatched at FN-def time. The Combine finish
    //    reads the result from `results[<id-position>]` and lifts into `Resolved`.
    enum ReturnTypeState<'a> {
        Done(KType),
        Pending {
            te: crate::ast::TypeExpr,
            producers: Vec<crate::runtime::machine::NodeId>,
        },
        Deferred(DeferredReturn<'a>),
        ExprSubDispatched(crate::runtime::machine::NodeId),
    }
    let return_type_state = match return_type_raw {
        ReturnTypeRaw::Resolved(kt) => ReturnTypeState::Done(kt),
        ReturnTypeRaw::TypeExprCarrier(te) => {
            // Stage B param-name scan first. If the surface form references any
            // parameter name, defer regardless of whether the leaf would otherwise
            // resolve against the outer scope — the per-call mapping is what
            // matters.
            if type_expr_references_any(&te, &param_names) {
                ReturnTypeState::Deferred(DeferredReturn::TypeExpr(te))
            } else {
                let name = te.name.clone();
                match elaborate_type_expr(&mut elaborator, &te) {
                    ElabResult::Done(kt) => ReturnTypeState::Done(kt),
                    ElabResult::Park(producers) => ReturnTypeState::Pending { te, producers },
                    ElabResult::Unbound(_) => match KType::from_name(&name) {
                        Some(kt) => ReturnTypeState::Done(kt),
                        None => {
                            return err(KError::new(KErrorKind::ShapeError(format!(
                                "FN return-type slot: unknown type name `{name}`"
                            ))));
                        }
                    },
                }
            }
        }
        ReturnTypeRaw::ExprCarrier(e) => {
            // Stage B overload-2 carrier. Two cases by the parameter-name scan:
            //
            //  * Parameter reference detected → `Deferred(Expression(e))`. Per-call
            //    re-dispatch runs at `KFunction::invoke` time against the per-call
            //    scope where Stage A's dual-write has installed the parameter's
            //    type-language identity.
            //  * No parameter reference → sub-dispatch the expression at FN-def
            //    against the outer scope. The result becomes a `Resolved` return
            //    type at Combine finish. Routes through `ExprSubDispatched` and
            //    composes with any parameter-list pending/sub-dispatch deps so a
            //    single Combine waits on everything.
            //
            // Today's wrap-path (pre-Stage-B) admitted the `Expression(_)` part into
            // overload 1's `TypeExprRef` slot via `accepts_for_wrap`'s relaxation;
            // overload 2's strict win now bypasses that. The synchronous sub-dispatch
            // below preserves the prior behavior end-to-end for the no-param case.
            if kexpression_references_any(&e, &param_names) {
                ReturnTypeState::Deferred(DeferredReturn::Expression(e))
            } else {
                let id = sched.add_dispatch(e, scope);
                ReturnTypeState::ExprSubDispatched(id)
            }
        }
    };

    // Step 2: elaborate the parameter list. Three sub-cases:
    //   * `Done(es)` — every slot resolved synchronously.
    //   * `Pending { park_producers, sub_dispatches }` — at least one slot needs a
    //     scheduler wake (placeholder finalization) or a sub-Dispatch (parens-wrapped
    //     type expression).
    //   * `Err(_)` — structural / unbound failure surfacing immediately.
    let params = match signature::parse_fn_param_list(&signature_expr, &mut elaborator) {
        ParamListOutcome::Done(es) => ParamListResult::Done(es),
        ParamListOutcome::Err(msg) => return err(KError::new(KErrorKind::ShapeError(msg))),
        ParamListOutcome::Pending { park_producers, sub_dispatches } => {
            ParamListResult::Pending { park_producers, sub_dispatches }
        }
    };

    // Step 3: route to synchronous-finalize or Combine-deferred based on parking state.
    match (return_type_state, params) {
        (ReturnTypeState::Done(rt), ParamListResult::Done(elements)) => {
            finalize_fn(scope, elements, ReturnType::Resolved(rt), body_expr)
        }
        (ReturnTypeState::Deferred(d), ParamListResult::Done(elements)) => {
            finalize_fn(scope, elements, ReturnType::Deferred(d), body_expr)
        }
        (ReturnTypeState::ExprSubDispatched(id), ParamListResult::Done(_)) => {
            // Return-type sub-dispatched, params synchronous. The Combine's only
            // dep is the return-type sub-Dispatch; the closure reads `results[0]`.
            defer_via_combine(
                scope,
                sched,
                signature_expr,
                ReturnTypeCapture::ReturnTypeExpr { results_pos: 0 },
                vec![id],
                Vec::new(),
                body_expr,
            )
        }
        (ReturnTypeState::Done(rt), ParamListResult::Pending { park_producers, sub_dispatches }) => {
            defer_via_combine(
                scope,
                sched,
                signature_expr,
                ReturnTypeCapture::Resolved(rt),
                park_producers,
                sub_dispatches,
                body_expr,
            )
        }
        (ReturnTypeState::Deferred(d), ParamListResult::Pending { park_producers, sub_dispatches }) => {
            // Params still parking on outer placeholders, but the return type is
            // per-call-deferred and doesn't need re-elaboration at the Combine wake.
            // Carry the carrier verbatim through to `finalize_fn` once params land.
            defer_via_combine(
                scope,
                sched,
                signature_expr,
                ReturnTypeCapture::Deferred(d),
                park_producers,
                sub_dispatches,
                body_expr,
            )
        }
        (ReturnTypeState::ExprSubDispatched(id), ParamListResult::Pending { mut park_producers, sub_dispatches }) => {
            // Mixed shape: return-type sub-dispatch must join the Combine alongside
            // any parking parameter-types and parameter-type sub-Dispatches. Append
            // the return-type id to `park_producers` first; its `results_pos` is the
            // current `park_producers.len()` before push (i.e. the next slot). The
            // closure reads `results[results_pos]` exactly there.
            let results_pos = park_producers.len();
            park_producers.push(id);
            defer_via_combine(
                scope,
                sched,
                signature_expr,
                ReturnTypeCapture::ReturnTypeExpr { results_pos },
                park_producers,
                sub_dispatches,
                body_expr,
            )
        }
        (ReturnTypeState::Pending { te, producers }, ParamListResult::Done(_)) => {
            // Param-types fully elaborated synchronously, but the return type parked. The
            // Combine finish re-runs both walks against the now-final scope for symmetry.
            // Pick `Unresolved(name)` for the bare-leaf shape (`Point`, `IntOrd`) so the
            // legacy `KType::from_name` fast path still applies; switch to `TypeExpr(te)`
            // when the shape carries parameters so structure survives the boundary.
            let capture = make_capture(te);
            defer_via_combine(
                scope,
                sched,
                signature_expr,
                capture,
                producers,
                Vec::new(),
                body_expr,
            )
        }
        (ReturnTypeState::Pending { te, producers: rt_producers }, ParamListResult::Pending { mut park_producers, sub_dispatches }) => {
            park_producers.extend(rt_producers);
            let capture = make_capture(te);
            defer_via_combine(
                scope,
                sched,
                signature_expr,
                capture,
                park_producers,
                sub_dispatches,
                body_expr,
            )
        }
    }
}

/// Pick the right `ReturnTypeCapture` variant for a parked-during-construction
/// `TypeExpr`. Bare leaves (`Point`, `IntOrd`) route through `Unresolved(name)` so the
/// legacy `KType::from_name` fast path applies on the Combine wake. Parameterized
/// shapes (`List<MyT>`, `Foo<Bar, Baz>`) route through `TypeExpr(te)` so the structured
/// elaboration survives the boundary verbatim.
fn make_capture<'a>(te: crate::ast::TypeExpr) -> ReturnTypeCapture<'a> {
    use crate::ast::TypeParams;
    match te.params {
        TypeParams::None => ReturnTypeCapture::Unresolved(te.name),
        TypeParams::List(_) | TypeParams::Function { .. } => ReturnTypeCapture::TypeExpr(te),
    }
}

/// Local mirror of [`ParamListOutcome`] minus the structural-error variant (which is
/// short-circuited above) and with `Pending`'s payload kept by-value so the routing
/// `match` stays readable.
enum ParamListResult<'a> {
    Done(Vec<SignatureElement>),
    Pending {
        park_producers: Vec<crate::runtime::machine::NodeId>,
        sub_dispatches: Vec<(usize, crate::ast::KExpression<'a>)>,
    },
}

/// Carrier for the return type across the Combine boundary. `Resolved` means we already
/// have a concrete `KType` and the Combine finish skips re-elaboration; `Unresolved` means
/// we parked on a bare leaf name and the finish runs `elaborate_type_expr` against the
/// now-final scope. `TypeExpr` is the structured variant — used when the parser-preserved
/// `TypeExpr` carries non-trivial parameter structure (`List<MyT>`, `Function<(A) -> B>`,
/// `Foo<Bar, Baz>`) whose `TypeParams::List` / `TypeParams::Function` slots need to be
/// preserved verbatim for re-elaboration against the now-final scope. Plumbing the full
/// `TypeExpr` rather than just the leaf name keeps the `params` intact; rendering and
/// re-parsing would round-trip through a string and strip the structure.
///
/// Parens-wrapped return-type expressions like `(SIG_WITH SetSig ((Elt: Number)))` do NOT
/// route through this carrier. The dispatcher's eager-sub-dispatch path
/// (`accepts_for_wrap` + `lazy_eager_indices`) resolves them at FN-construction time and
/// splices the resulting `KTypeValue` into the FN bundle as a `Future(_)`; the FN body
/// then extracts a concrete `KType` via the `Resolved` arm. The structured-`TypeExpr`
/// carrier exists for parked-during-construction leaf-with-parameters shapes where the
/// parser already produced a `TypeExpr` with non-`None` params and we need to wait on a
/// type-binding placeholder before final elaboration.
enum ReturnTypeCapture<'a> {
    Resolved(KType),
    Unresolved(String),
    TypeExpr(crate::ast::TypeExpr),
    /// Module-system functor-params Stage B: parameter-name reference detected in the
    /// return-type carrier at FN-def time. The carrier is held verbatim and propagated
    /// through to the final `ReturnType::Deferred(_)` on the registered `KFunction`'s
    /// signature without elaboration at the Combine wake — per-call elaboration runs at
    /// the dispatch boundary instead.
    Deferred(DeferredReturn<'a>),
    /// Module-system functor-params Stage B: overload-2 return-type carrier whose
    /// parens-form expression doesn't reference any parameter — sub-dispatch the
    /// expression at FN-def and lift the resulting `KTypeValue` into `Resolved` at
    /// Combine finish. The `results_pos` index says where the result lands in the
    /// closure's `&[&'a KObject<'a>]` slice; the FN-def body computes this when it
    /// merges the return-type sub-dispatch into the Combine's overall `deps` order.
    /// Replaces today's wrap-path admission into overload 1's `TypeExprRef` slot
    /// for the parens-form no-parameter case (which overload 2 now wins on the
    /// strict pass).
    ReturnTypeExpr { results_pos: usize },
}

/// Build the `KFunction` and register it in `scope`. Shared between the synchronous
/// (no-park) path and the Combine-finish path.
fn finalize_fn<'a>(
    scope: &'a Scope<'a>,
    elements: Vec<SignatureElement>,
    return_type: ReturnType<'a>,
    body_expr: crate::ast::KExpression<'a>,
) -> BodyResult<'a> {
    // Pick the first Keyword as the data-table key. `Bindings::functions` does the load-
    // bearing dispatch lookup by signature; `Bindings::data` is mostly for discoverability
    // and shadow-by-name semantics, neither of which has a single right answer for a
    // multi-token signature like `(a ADD b)`. First Keyword is a defensible default.
    let name = elements.iter().find_map(|e| match e {
        SignatureElement::Keyword(s) => Some(s.clone()),
        _ => None,
    });
    let name = match name {
        Some(n) => n,
        None => {
            return err(KError::new(KErrorKind::ShapeError(
                "FN signature must contain at least one Keyword (a fixed token to dispatch on)"
                    .to_string(),
            )));
        }
    };

    let user_sig = ExpressionSignature {
        return_type,
        elements,
    };

    let arena = scope.arena;
    let f: &'a KFunction<'a> = arena.alloc_function(KFunction::new(
        user_sig,
        Body::UserDefined(body_expr),
        scope,
    ));
    // `frame: None` here — the lift-on-return logic in the scheduler will populate the Rc
    // when this KFunction value escapes out of a per-call body. For top-level FNs, there's
    // no per-call frame to clone, so None stays.
    let obj: &'a KObject<'a> = arena.alloc_object(KObject::KFunction(f, None));
    if let Err(e) = scope.register_function(name, f, obj) {
        return err(e);
    }
    // Returning the function reference (rather than null) lets callers do
    // `LET f = (FN ...)` to capture a callable handle, which the dispatch fallback for
    // identifier-bound KFunctions can then invoke.
    BodyResult::Value(obj)
}

/// Schedule a `Combine` over `park_producers` plus any newly scheduled sub-Dispatches
/// for parens-wrapped parameter types, then re-run the signature elaboration in the
/// finish closure. Mirrors MODULE / SIG's `BodyResult::DeferTo` shape: the FN's terminal
/// lifts off the Combine's terminal, so the parent scope's binding lands at Combine-finish
/// time.
///
/// Splice protocol: every entry in `sub_dispatches` is scheduled here as
/// `sched.add_dispatch(sub_expr, scope)`; the resulting `NodeId` is appended to the
/// Combine's `deps` vector after the park producers. The closure tracks each
/// sub-Dispatch's `(slot_idx_in_signature_parts, position_in_results)` pairing so that
/// when the Combine wakes, the finish closure splices each result into
/// `signature_expr.parts[slot_idx]` as `Future(obj)` before re-running
/// `parse_fn_param_list` against the now-final scope.
fn defer_via_combine<'a>(
    scope: &'a Scope<'a>,
    sched: &mut dyn SchedulerHandle<'a>,
    signature_expr: crate::ast::KExpression<'a>,
    return_type_capture: ReturnTypeCapture<'a>,
    park_producers: Vec<crate::runtime::machine::NodeId>,
    sub_dispatches: Vec<(usize, crate::ast::KExpression<'a>)>,
    body_expr: crate::ast::KExpression<'a>,
) -> BodyResult<'a> {
    // Schedule sub-Dispatches up front. `splice_layout[k] = (slot_idx, results_pos)` says
    // "splice results[results_pos] into signature.parts[slot_idx] as `Future(_)`".
    // `results_pos` is captured as `deps.len()` immediately before the new dep is pushed,
    // so the offset over `park_producers` falls out naturally — Combine's `results` slice
    // mirrors `deps` order, park producers first.
    let mut deps: Vec<crate::runtime::machine::NodeId> = park_producers;
    let mut splice_layout: Vec<(usize, usize)> = Vec::with_capacity(sub_dispatches.len());
    for (slot_idx, sub_expr) in sub_dispatches {
        let id = sched.add_dispatch(sub_expr, scope);
        splice_layout.push((slot_idx, deps.len()));
        deps.push(id);
    }

    let finish: CombineFinish<'a> = Box::new(move |scope, _sched, results| {
        // Splice each sub-Dispatch's result into the corresponding signature slot as a
        // `Future(_)`. Cloning the `signature_expr` keeps the closure callable on a
        // hypothetical future re-wake (the Combine fires once today, but the
        // `KExpression` clone is cheap and matches the pattern used for `body_expr`).
        let mut spliced_parts = signature_expr.parts.clone();
        for &(slot_idx, results_pos) in &splice_layout {
            let obj = results[results_pos];
            // Reject non-type results early with a focused diagnostic. The downstream
            // `parse_fn_param_list` would also reject (its `Future(other)` arm), but
            // catching here lets us name the offending slot's part-index in the message.
            if !matches!(obj, KObject::KTypeValue(_)) {
                return BodyResult::Err(KError::new(KErrorKind::ShapeError(format!(
                    "FN signature slot at part-index {slot_idx} expected a type expression, \
                     got a {} value",
                    obj.ktype().name(),
                ))));
            }
            spliced_parts[slot_idx] = ExpressionPart::Future(obj);
        }
        let spliced_signature = crate::ast::KExpression { parts: spliced_parts };

        // Park producers have finalized — re-elaborate against the now-stable scope. The
        // elaborator's `Park` arm cannot fire again because every parked producer is
        // terminal by the Combine-finish invariant; if it does, that's a regression
        // worth surfacing as a structured error rather than re-parking forever.
        //
        // Stage B `Deferred(_)` case: the carrier doesn't re-elaborate here — it propagates
        // verbatim through to `ReturnType::Deferred(_)` on the finalized FN. Per-call
        // re-elaboration runs at `KFunction::invoke` time against the dispatch boundary's
        // per-call scope.
        let mut elaborator = Elaborator::new(scope);
        let return_type: ReturnType<'a> = match return_type_capture {
            ReturnTypeCapture::Resolved(kt) => ReturnType::Resolved(kt),
            ReturnTypeCapture::Unresolved(name) => match elaborate_type_expr(
                &mut elaborator,
                &crate::ast::TypeExpr::leaf(name.clone()),
            ) {
                ElabResult::Done(kt) => ReturnType::Resolved(kt),
                ElabResult::Park(_) => {
                    return BodyResult::Err(KError::new(KErrorKind::ShapeError(
                        "FN return type parked after Combine wake".to_string(),
                    )));
                }
                ElabResult::Unbound(_) => match KType::from_name(&name) {
                    Some(kt) => ReturnType::Resolved(kt),
                    None => {
                        return BodyResult::Err(KError::new(KErrorKind::ShapeError(format!(
                            "FN return-type slot: unknown type name `{name}`"
                        ))));
                    }
                },
            },
            // Structured-TypeExpr capture: re-elaborate the full parser-preserved shape
            // against the now-final scope. The Park arm is a protocol error (every parked
            // producer is terminal by Combine-finish invariant); the Unbound arm fails the
            // FN-def because the surface form references a name that didn't resolve
            // anywhere reachable.
            ReturnTypeCapture::TypeExpr(t) => match elaborate_type_expr(
                &mut elaborator,
                &t,
            ) {
                ElabResult::Done(kt) => ReturnType::Resolved(kt),
                ElabResult::Park(_) => {
                    return BodyResult::Err(KError::new(KErrorKind::ShapeError(
                        "FN return type parked after Combine wake".to_string(),
                    )));
                }
                ElabResult::Unbound(msg) => {
                    return BodyResult::Err(KError::new(KErrorKind::ShapeError(format!(
                        "FN return-type slot: {msg}"
                    ))));
                }
            },
            ReturnTypeCapture::Deferred(d) => ReturnType::Deferred(d),
            ReturnTypeCapture::ReturnTypeExpr { results_pos } => {
                // Sub-Dispatch result lives at `results[results_pos]` by capture
                // protocol (the FN-def body computed this when it merged the
                // return-type sub-dispatch into the Combine's `deps` order).
                let obj = results[results_pos];
                match obj {
                    KObject::KTypeValue(kt) => ReturnType::Resolved(kt.clone()),
                    other => {
                        return BodyResult::Err(KError::new(KErrorKind::ShapeError(format!(
                            "FN return-type slot sub-Dispatch expected a type expression, \
                             got a {} value",
                            other.ktype().name(),
                        ))));
                    }
                }
            }
        };
        let elements = match signature::parse_fn_param_list(&spliced_signature, &mut elaborator) {
            ParamListOutcome::Done(es) => es,
            ParamListOutcome::Err(msg) => {
                return BodyResult::Err(KError::new(KErrorKind::ShapeError(msg)));
            }
            ParamListOutcome::Pending { .. } => {
                return BodyResult::Err(KError::new(KErrorKind::ShapeError(
                    "FN signature elaboration still pending after Combine wake".to_string(),
                )));
            }
        };
        finalize_fn(scope, elements, return_type, body_expr.clone())
    });
    let combine_id = sched.add_combine(deps, scope, finish);
    BodyResult::DeferTo(combine_id)
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    // Overload 1: `Type(_)` or `TypeNameRef` return-type carrier (the historical surface).
    // FN-construction-time eager-elaborate path lifts the resolved `KType` into the
    // signature. Module-system functor-params Stage B: if the captured `TypeExpr` carries
    // a parameter-name leaf, the body switches to `ReturnType::Deferred(TypeExpr)` and
    // re-elaborates per call against the dispatch-boundary scope.
    // FN returns a function value, but there's no "any function" KType anymore —
    // a function's structural type only exists once its signature is known. `Any`
    // here lets the constructed `KObject::KFunction`'s projected `ktype()` (which
    // does carry the full signature) flow through any caller's slot.
    register_builtin_with_pre_run(
        scope,
        "FN",
        sig(KType::Any, vec![
            kw("FN"),
            arg("signature", KType::KExpression),
            kw("->"),
            arg("return_type", KType::TypeExprRef),
            kw("="),
            arg("body", KType::KExpression),
        ]),
        body,
        Some(pre_run),
    );
    // Overload 2: `Expression(_)` return-type carrier — parens-form return types
    // (`(MODULE_TYPE_OF Er Type)`, `(SIG_WITH Set ((Elt: Er)))`). The `KExpression` slot
    // accepts the parens-form raw, so FN-def never sub-dispatches the expression against
    // the outer scope (where the parameter name is unbound by construction). The same
    // `body` runs and branches on `bundle.get("return_type")`'s shape.
    //
    // Dispatch shape: a `Type(_)` part strictly matches overload 1's `TypeExprRef` slot;
    // an `Expression(_)` part strictly matches overload 2's `KExpression` slot. The
    // strict pass picks one or the other unambiguously; the wrap-path admission of an
    // `Expression(_)` into `TypeExprRef` (the pre-Stage-B fallback for parens-form return
    // types) becomes a tentative-fallback that loses to overload 2's strict win.
    //
    // `Future(KTypeValue(_))` post-Combine wakes (the picker re-walks against a spliced
    // signature) still admit only against overload 1's `TypeExprRef`, since
    // `KExpression` doesn't accept `Future(_)` of any kind. Pinned by
    // `fn_def/tests/module_stage2.rs` regression coverage.
    register_builtin_with_pre_run(
        scope,
        "FN",
        sig(KType::Any, vec![
            kw("FN"),
            arg("signature", KType::KExpression),
            kw("->"),
            arg("return_type", KType::KExpression),
            kw("="),
            arg("body", KType::KExpression),
        ]),
        body,
        Some(pre_run),
    );
}

#[cfg(test)]
mod tests;
