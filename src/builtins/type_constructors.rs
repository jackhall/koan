//! Keyworded type-constructor builtins reached through the `:(...)` sigil.
//! See [type-language-via-dispatch](../../design/typing/type-language-via-dispatch.md).
//!
//! - `LIST OF :Type` → `KTypeValue(KType::List(_))`
//! - `MAP :Type -> :Type` → `KTypeValue(KType::Dict(_, _))`
//! - `FN <sig> -> :Type` → `KTypeValue(KType::KFunction { .. })`
//! - `FUNCTOR <sig> -> :Type` → `KTypeValue(KType::KFunctor { .. })`
//!
//! Fully-uppercase head keywords keep parameterized-type construction in
//! narrow candidate buckets so user-defined functors overloading short
//! connector words like `OF` don't pay a bucket-walk cost on every dispatched
//! parameterized type.

use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::model::{KObject, KType};
use crate::machine::{
    ArgumentBundle, BodyResult, CombineFinish, KError, KErrorKind, NodeId, Scope, SchedulerHandle,
};

use super::{arg, err, kw, register_builtin, sig};

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

/// `sig` is `KExpression` (lazy) so the parser-emitted nested-parens
/// `(x :Number, y :Str)` arrives unevaluated. Parameter names are dropped at
/// lowering — `KType::KFunction` stays positional; named identity is the
/// [fn-named-identity](../../roadmap/type_language/fn-named-identity.md) follow-up.
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
    build_kfunction_carrier(scope, sched, sig_expr, ret, false)
}

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
    build_kfunction_carrier(scope, sched, sig_expr, ret, true)
}

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

/// By the time the Combine fires every parked producer is terminal, so the
/// resolver's `Park` arm cannot fire again — a re-park is a scheduling
/// invariant break and surfaces as a structured error rather than re-deferring.
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
    let combine_id = sched.add_combine(vec![], producers, scope, finish);
    BodyResult::DeferTo(combine_id)
}

/// `Park` accumulates every blocker in one pass rather than short-circuiting
/// on the first, so the caller can schedule a single Combine over all of them.
enum ExtractOutcome<'a> {
    Done(Vec<KType<'a>>),
    Park(Vec<NodeId>),
    Err(KError),
}

/// Resolution runs through `Scope::resolve_type_expr` (rather than the
/// builtin-only `KType::from_type_expr`) so user-declared signatures,
/// modules, and other scope-bound type identities resolve.
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
        // Uppercase-leading identifiers parse as bare leaf `Type` parts, so
        // either `Identifier` or a parameterless `Type` token is a valid name.
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
                    // Placeholder keeps indices aligned during the park-walk;
                    // discarded by the caller when it switches to the Park arm.
                    parks.extend(producers);
                    out.push(KType::Any);
                }
            },
            // Sub-dispatched type-side carriers arrive as `Future`s after the
            // outer Combine spliced them in.
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

    // Param name `Ty` uses two letters because koan rejects single-uppercase-letter tokens.
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

}
