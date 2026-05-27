//! Keyworded type-constructor builtins reached through the `:(...)` sigil.
//!
//! After [type-language-via-dispatch](../../design/typing/type-language-via-dispatch.md)
//! the sigil is a parse-context marker — `:(...)` wraps its inner expression in
//! `ExpressionPart::SigiledTypeExpr`, and the dispatcher unwraps and runs the
//! standard classifier. The four overloads here register the new keyworded
//! shapes that parameterized-type construction routes through:
//!
//! - `LIST OF :Type` — `:(LIST OF Number)` → `KTypeValue(KType::List(Number))`
//! - `MAP :Type -> :Type` — `:(MAP Str -> Number)` → `KTypeValue(KType::Dict(Str, Number))`
//! - `FN <sig> -> :Type` — `:(FN (x :Number) -> Bool)` → `KTypeValue(KType::KFunction { ... })`
//! - `FUNCTOR <sig> -> :Type` — `:(FUNCTOR (T :S) -> M)` → `KTypeValue(KType::KFunctor { ... })`
//!
//! The legacy positional sigil shape `:(List Number)` still parses (every
//! `:(...)` emits `ExpressionPart::SigiledTypeExpr` regardless of shape); the
//! dispatcher's `TypeConstructorCall` arm handles the leaf-Type-headed case.
//! Inside STRUCT/UNION field schemas, the field-walker's `try_synth_legacy`
//! path elaborates legacy positional shapes inline because it carries the
//! SCC threading context the standalone dispatcher does not yet plumb — see
//! `roadmap/dispatch_fix/scc-aware-dispatcher-for-self-recursive-types.md`.
//!
//! Naming: fully-uppercase head keywords (`LIST`, `MAP`, `FN`, `FUNCTOR`) keep
//! parameterized-type construction in narrow candidate buckets so user-defined
//! functors overloading short connector words like `OF` don't pay a
//! bucket-walk cost on every dispatched parameterized type.

use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::model::{KObject, KType};
use crate::machine::{
    ArgumentBundle, BodyResult, CombineFinish, KError, KErrorKind, NodeId, Scope, SchedulerHandle,
};

use super::{arg, err, kw, register_builtin, sig};

/// `LIST OF <elem :Type>` → `KTypeValue(KType::List(elem))`.
fn body_list_of<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let elem = match bundle.require_ktype("elem") {
        Ok(t) => t.clone(),
        Err(e) => return err(e),
    };
    BodyResult::Value(
        scope
            .arena
            .alloc(KObject::KTypeValue(KType::List(Box::new(elem)))),
    )
}

/// `MAP <k :Type> -> <v :Type>` → `KTypeValue(KType::Dict(K, V))`. Surface keyword
/// `MAP` lowers to the same underlying `KType::Dict` identity the legacy `DICT_OF`
/// builtin and the parser-fold-era `:(Dict K V)` form produced — only the construction
/// surface changes.
fn body_map<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let k = match bundle.require_ktype("k") {
        Ok(t) => t.clone(),
        Err(e) => return err(e),
    };
    let v = match bundle.require_ktype("v") {
        Ok(t) => t.clone(),
        Err(e) => return err(e),
    };
    BodyResult::Value(
        scope
            .arena
            .alloc(KObject::KTypeValue(KType::Dict(Box::new(k), Box::new(v)))),
    )
}

/// `FN <sig :KExpression> -> <ret :Type>` → `KTypeValue(KType::KFunction { args, ret })`.
///
/// The `sig` slot is `KExpression` (lazy) so the parser-emitted nested-parens
/// `(x :Number, y :Str)` arrives unevaluated. The body walks the inner parts
/// extracting `<name> :<Type>` pairs and lowers each `:<Type>` into `KType` via
/// the scope's resolver — user-declared signatures (`OrderedSig`) and structural
/// nested types both resolve. Parameter names are dropped at lowering —
/// `KType::KFunction` stays positional this PR. Named identity is the
/// [fn-named-identity](../../roadmap/type_language/fn-named-identity.md) follow-up.
///
/// A forward type reference inside the signature (`:OrderedSig` where the SIG
/// is a sibling declaration not yet finalized) routes through
/// [`defer_via_combine`]: the body schedules a Combine over the parking
/// producers and re-runs `extract_param_types` in the finish closure once they
/// terminalize. Mirrors the VAL / STRUCT / FN-def deferral pattern.
fn body_fn<'a>(
    scope: &'a Scope<'a>,
    sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let sig_expr = match bundle.require_kexpression("sig") {
        Ok(e) => e.clone(),
        Err(e) => return err(e),
    };
    let ret = match bundle.require_ktype("ret") {
        Ok(t) => t.clone(),
        Err(e) => return err(e),
    };
    build_kfunction_carrier(scope, sched, sig_expr, ret, /* is_functor */ false)
}

/// `FUNCTOR <sig :KExpression> -> <ret :Type>` → `KTypeValue(KType::KFunctor { params, ret })`.
/// Symmetric with `FN`; the resulting carrier flags type-side as a functor identity.
fn body_functor<'a>(
    scope: &'a Scope<'a>,
    sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let sig_expr = match bundle.require_kexpression("sig") {
        Ok(e) => e.clone(),
        Err(e) => return err(e),
    };
    let ret = match bundle.require_ktype("ret") {
        Ok(t) => t.clone(),
        Err(e) => return err(e),
    };
    build_kfunction_carrier(scope, sched, sig_expr, ret, /* is_functor */ true)
}

/// Shared body for FN/FUNCTOR sigil overloads. Walks the signature once
/// synchronously; on success returns the `KTypeValue` carrier directly; on
/// forward-reference park, defers through a Combine that re-walks at finish
/// against the now-final scope. `is_functor` flips which `KType` variant is
/// produced (`KFunction` vs `KFunctor`).
fn build_kfunction_carrier<'a>(
    scope: &'a Scope<'a>,
    sched: &mut dyn SchedulerHandle<'a>,
    sig_expr: KExpression<'a>,
    ret: KType<'a>,
    is_functor: bool,
) -> BodyResult<'a> {
    let head = if is_functor { "FUNCTOR" } else { "FN" };
    match extract_param_types(scope, &sig_expr, head) {
        ExtractOutcome::Done(args) => BodyResult::Value(finalize_carrier(scope, args, ret, is_functor)),
        ExtractOutcome::Err(e) => err(e),
        ExtractOutcome::Park(producers) => {
            defer_via_combine(scope, sched, sig_expr, ret, producers, is_functor)
        }
    }
}

/// Build the final `KTypeValue` carrier. Shared between the synchronous arm
/// (no parks) and the Combine-finish arm (after parks terminalize).
fn finalize_carrier<'a>(
    scope: &'a Scope<'a>,
    args: Vec<KType<'a>>,
    ret: KType<'a>,
    is_functor: bool,
) -> &'a KObject<'a> {
    let kt = if is_functor {
        KType::KFunctor { params: args, ret: Box::new(ret) }
    } else {
        KType::KFunction { args, ret: Box::new(ret) }
    };
    scope.arena.alloc(KObject::KTypeValue(kt))
}

/// Schedule a Combine over `producers` (parking sibling slots whose values the
/// re-walk reads but does not own) and re-run `extract_param_types` in the
/// finish closure. By the time the Combine fires, every parked producer is
/// terminal, so the resolver's `Park` arm cannot fire again — a re-park here
/// is a scheduling invariant break and surfaces as a structured error rather
/// than re-deferring.
///
/// Mirrors `val_decl::defer_val_via_combine` and `struct_def::defer_struct_via_combine`.
fn defer_via_combine<'a>(
    scope: &'a Scope<'a>,
    sched: &mut dyn SchedulerHandle<'a>,
    sig_expr: KExpression<'a>,
    ret: KType<'a>,
    producers: Vec<NodeId>,
    is_functor: bool,
) -> BodyResult<'a> {
    let head = if is_functor { "FUNCTOR" } else { "FN" };
    let finish: CombineFinish<'a> = Box::new(move |scope, _sched, _results| {
        match extract_param_types(scope, &sig_expr, head) {
            ExtractOutcome::Done(args) => {
                BodyResult::Value(finalize_carrier(scope, args, ret.clone(), is_functor))
            }
            ExtractOutcome::Err(e) => BodyResult::Err(e),
            ExtractOutcome::Park(_) => BodyResult::Err(KError::new(KErrorKind::ShapeError(format!(
                "{head} parameter type: forward type reference still unresolved after \
                 Combine wake — every producer was terminal by invariant; scheduling \
                 inconsistency"
            )))),
        }
    });
    // Producers are sibling slots this Combine reads at finish-time but does NOT
    // own (mirrors VAL / STRUCT / SIG / FN-def's deferral shape).
    let combine_id = sched.add_combine(vec![], producers, scope, finish);
    BodyResult::DeferTo(combine_id)
}

/// Outcome of one walk over the signature. `Park` carries the parking
/// producer ids so the caller can schedule a single Combine over them; like
/// the field-list walker in `typed_field_list`, the walk continues after a
/// park to accumulate every blocker in one pass rather than short-circuiting
/// on the first.
enum ExtractOutcome<'a> {
    Done(Vec<KType<'a>>),
    Park(Vec<NodeId>),
    Err(KError),
}

/// Walk a `(x :Number, y :Str)`-shaped parameter list and lower each type
/// annotation into a `KType`. Parameter names appear at the surface but are
/// dropped at lowering — `KType::KFunction { args, ret }` and
/// `KType::KFunctor { params, ret }` stay positional this PR. Empty `()`
/// produces an empty arg list (nullary functions / functors).
///
/// Type-name resolution runs through `Scope::resolve_type_expr` (rather than
/// the builtin-only `KType::from_type_expr`) so user-declared signatures
/// (`:OrderedSig`), modules, and other scope-bound type identities resolve.
/// A `Park` from the resolver bubbles up as `ExtractOutcome::Park` so the
/// caller can defer the whole body via Combine — see
/// [`defer_via_combine`].
fn extract_param_types<'a>(
    scope: &'a Scope<'a>,
    sig_expr: &KExpression<'a>,
    head: &str,
) -> ExtractOutcome<'a> {
    use crate::machine::core::ResolveTypeExprOutcome;
    use crate::machine::model::ast::TypeParams;
    let parts = &sig_expr.parts;
    let mut out: Vec<KType<'a>> = Vec::new();
    let mut parks: Vec<NodeId> = Vec::new();
    let mut i = 0;
    while i < parts.len() {
        // A `<name>` is either an `Identifier` or — by the bare-leaf-Type-name
        // rule that lets uppercase-leading identifiers parse as `Type` parts —
        // a leaf `Type` token. Both are valid here.
        let name_present = matches!(
            &parts[i].value,
            ExpressionPart::Identifier(_)
                | ExpressionPart::Type(crate::machine::model::ast::TypeExpr {
                    params: TypeParams::None,
                    ..
                })
        );
        if !name_present {
            return ExtractOutcome::Err(KError::new(KErrorKind::ShapeError(format!(
                "{head} parameter list: expected `<name> :<Type>` at part {i}, \
                 got `{}`",
                parts[i].value.summarize(),
            ))));
        }
        let Some(ty_part) = parts.get(i + 1) else {
            return ExtractOutcome::Err(KError::new(KErrorKind::ShapeError(format!(
                "{head} parameter `{}` requires a `:<Type>` annotation",
                parts[i].value.summarize(),
            ))));
        };
        match &ty_part.value {
            ExpressionPart::Type(t) => match scope.resolve_type_expr(t) {
                ResolveTypeExprOutcome::Done(kt) => out.push(kt.clone()),
                ResolveTypeExprOutcome::Unbound(msg) => {
                    return ExtractOutcome::Err(KError::new(KErrorKind::ShapeError(format!(
                        "{head} parameter type: {msg}"
                    ))));
                }
                ResolveTypeExprOutcome::Park(producers) => {
                    // Forward type reference (e.g. `:OrderedSig` where the SIG
                    // is a sibling declaration not yet finalized). Accumulate
                    // the producers; the caller schedules one Combine over the
                    // merged list and re-runs this walk in the finish closure.
                    // Push a placeholder element so the indices stay aligned;
                    // discarded in the Park arm by the caller.
                    parks.extend(producers);
                    out.push(KType::Any);
                }
            },
            // Sub-dispatched type-side carriers arrive here as `Future`s after the
            // outer Combine spliced them in. `KTypeValue` is the canonical type-side
            // carrier; module / signature / user-type carriers are admissible
            // type-side identities and survive via their reported `ktype()`.
            ExpressionPart::Future(KObject::KTypeValue(kt)) => out.push(kt.clone()),
            other => {
                return ExtractOutcome::Err(KError::new(KErrorKind::ShapeError(format!(
                    "{head} parameter type must be a type expression, got `{}`",
                    other.summarize(),
                ))));
            }
        }
        i += 2;
    }
    if !parks.is_empty() {
        return ExtractOutcome::Park(parks);
    }
    ExtractOutcome::Done(out)
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    register_builtin(
        scope,
        "LIST",
        sig(KType::Type, vec![
            kw("LIST"),
            kw("OF"),
            arg("elem", KType::Type),
        ]),
        body_list_of,
    );
    register_builtin(
        scope,
        "MAP",
        sig(KType::Type, vec![
            kw("MAP"),
            arg("k", KType::Type),
            kw("->"),
            arg("v", KType::Type),
        ]),
        body_map,
    );
    register_builtin(
        scope,
        "FN",
        sig(KType::Type, vec![
            kw("FN"),
            arg("sig", KType::KExpression),
            kw("->"),
            arg("ret", KType::Type),
        ]),
        body_fn,
    );
    register_builtin(
        scope,
        "FUNCTOR",
        sig(KType::Type, vec![
            kw("FUNCTOR"),
            arg("sig", KType::KExpression),
            kw("->"),
            arg("ret", KType::Type),
        ]),
        body_functor,
    );
}

#[cfg(test)]
mod tests {
    use crate::builtins::test_support::{parse_one, run_one, run_root_silent};
    use crate::machine::model::{KObject, KType};
    use crate::machine::RuntimeArena;

    /// `:(LIST OF Number)` evaluates the inner `LIST OF Number` expression and
    /// produces a `KTypeValue` carrying `KType::List(Number)`.
    #[test]
    fn list_of_number_lowers_to_list_number() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let result = run_one(scope, parse_one(":(LIST OF Number)"));
        match result {
            KObject::KTypeValue(kt) => {
                assert_eq!(*kt, KType::List(Box::new(KType::Number)));
            }
            other => panic!("expected KTypeValue, got {:?}", other.ktype()),
        }
    }

    /// `:(MAP Str -> Number)` lowers to `Dict<Str, Number>`. The surface keyword
    /// changes but the underlying carrier is the same `KType::Dict` shape.
    #[test]
    fn map_str_number_lowers_to_dict() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let result = run_one(scope, parse_one(":(MAP Str -> Number)"));
        match result {
            KObject::KTypeValue(kt) => {
                assert_eq!(*kt, KType::Dict(Box::new(KType::Str), Box::new(KType::Number)));
            }
            other => panic!("expected KTypeValue, got {:?}", other.ktype()),
        }
    }

    /// `:(FN (x :Number, y :Str) -> Bool)` lowers to
    /// `KFunction { args: [Number, Str], ret: Bool }` (positional storage).
    #[test]
    fn fn_lowers_to_kfunction() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let result = run_one(scope, parse_one(":(FN (x :Number, y :Str) -> Bool)"));
        match result {
            KObject::KTypeValue(kt) => {
                assert_eq!(
                    *kt,
                    KType::KFunction {
                        args: vec![KType::Number, KType::Str],
                        ret: Box::new(KType::Bool),
                    }
                );
            }
            other => panic!("expected KTypeValue, got {:?}", other.ktype()),
        }
    }

    /// `:(FN () -> Number)` — nullary FN type lowers to a zero-arg KFunction.
    #[test]
    fn fn_nullary_lowers_to_kfunction() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let result = run_one(scope, parse_one(":(FN () -> Number)"));
        match result {
            KObject::KTypeValue(kt) => {
                assert_eq!(
                    *kt,
                    KType::KFunction {
                        args: vec![],
                        ret: Box::new(KType::Number),
                    }
                );
            }
            other => panic!("expected KTypeValue, got {:?}", other.ktype()),
        }
    }

    /// `:(FUNCTOR (Ty :Signature) -> Module)` — sigiled FUNCTOR type form. Maps
    /// to `KType::KFunctor { params: [AnySignature], ret: AnyModule }`. Param
    /// name uses two letters because koan rejects single-uppercase-letter tokens.
    #[test]
    fn functor_lowers_to_kfunctor() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let result = run_one(scope, parse_one(":(FUNCTOR (Ty :Signature) -> Module)"));
        match result {
            KObject::KTypeValue(kt) => {
                assert_eq!(
                    *kt,
                    KType::KFunctor {
                        params: vec![KType::AnySignature],
                        ret: Box::new(KType::AnyModule),
                    }
                );
            }
            other => panic!("expected KTypeValue, got {:?}", other.ktype()),
        }
    }

    // `legacy_list_number_still_works_via_typecall` deleted: with the `TypeCall`
    // arm removed from the dispatcher, the legacy positional `:(List Number)`
    // form no longer resolves through the standalone dispatcher. The field-
    // walker's `try_synth_legacy` path still serves it inline for STRUCT / UNION
    // field schemas, but no standalone-dispatch fallback exists.
}
