//! Bare type tokens (`Number`, `Str`, `Bool`, `Null`) as `:Type`-typed
//! FUNCTOR arguments. Pins the widening at
//! [`KType::accepts_part`](crate::machine::model::types) and the
//! deferred-return re-elaboration path's agnosticism to builtin-vs-nominal
//! carriers.

use crate::builtins::test_support::{parse_one, run, run_one, run_one_type, run_root_silent};
use crate::machine::core::FrameStorage;
use crate::machine::execute::KoanRuntime;
use crate::machine::model::ast::KExpression;
use crate::machine::model::{KObject, KType};
use crate::machine::{KError, KErrorKind, Scope};

/// Tolerates the error surfacing either from `KoanRuntime::execute()` (resolve
/// rejects at admission) or from `read_result` (auto-wrap committed and bind
/// later refused). Compare `test_support::run_one_err`, which panics on the
/// first path.
fn run_expecting_dispatch_error<'a>(scope: &'a Scope<'a>, expr: KExpression<'a>) -> KError {
    let mut sched = KoanRuntime::new();
    let id = sched.dispatch_in_scope(expr, scope);
    match sched.execute() {
        Err(e) => e,
        Ok(()) => match sched.read_result(id) {
            Err(e) => e.clone(),
            Ok(v) => panic!(
                "expected dispatch-level error, got value of type {}",
                v.ktype().name(),
            ),
        },
    }
}

#[test]
fn functor_admits_bare_number_token_at_type_slot() {
    let region = FrameStorage::run_root();
    let scope = run_root_silent(&region);
    run(
        scope,
        "FUNCTOR (MAKETREE Elt :Type) -> Module = (MODULE Generated = (LET inner = 1))",
    );
    let result = run_one_type(scope, parse_one("MAKETREE Number"));
    match result {
        KType::Module { .. } => {}
        other => {
            panic!("expected MAKETREE Number to dispatch and return a module, got {other:?}")
        }
    }
}

#[test]
fn functor_admits_bare_str_bool_null_tokens_at_type_slot() {
    let region = FrameStorage::run_root();
    let scope = run_root_silent(&region);
    run(
        scope,
        "FUNCTOR (MAKETREE Elt :Type) -> Module = (MODULE Generated = (LET inner = 1))",
    );
    for token in ["Str", "Bool", "Null"] {
        let src = format!("MAKETREE {token}");
        let result = run_one_type(scope, parse_one(&src));
        match result {
            KType::Module { .. } => {}
            other => {
                panic!("expected MAKETREE {token} to dispatch and return a module, got {other:?}")
            }
        }
    }
}

#[test]
fn functor_per_call_type_side_bind_is_observable_via_module_type_members() {
    let region = FrameStorage::run_root();
    let scope = run_root_silent(&region);
    run(
        scope,
        "FUNCTOR (MAKETREE Elt :Type) -> Module = \
         (MODULE Generated = ((LET ElemType = Elt) (LET inner = 1)))",
    );
    let result = run_one_type(scope, parse_one("MAKETREE Number"));
    let module = match result {
        KType::Module { module, .. } => *module,
        other => panic!("expected module result, got {other:?}"),
    };
    let tm = module.type_members.borrow();
    match tm.get("ElemType") {
        Some(KType::Number) => {}
        other => panic!(
            "expected ElemType registered as KType::Number on returned module, got {:?}",
            other,
        ),
    }
}

/// Non-type carrier is a dispatch no-match, not a bind-time `TypeMismatch`
/// against a committed pick — non-satisfying typed args fall through the
/// scope walk.
#[test]
fn functor_bare_value_carrier_is_dispatch_no_match_not_typemismatch() {
    let region = FrameStorage::run_root();
    let scope = run_root_silent(&region);
    run(
        scope,
        "FUNCTOR (MAKETREE Elt :Type) -> Module = (MODULE Generated = (LET inner = 1))",
    );
    let err = run_expecting_dispatch_error(scope, parse_one("MAKETREE 7"));
    match &err.kind {
        KErrorKind::DispatchFailed { .. } | KErrorKind::UnboundName(_) => {}
        _ => panic!("expected dispatch no-match (DispatchFailed) for non-type carrier, got {err}",),
    }
}

/// Module carriers stay out of `:Type` slots — the cut-(a) wall at
/// [`KType::accepts_part`]'s `Spliced(Carried::Type(KType::Module
/// { .. }))` arm. Asserts only that no value comes back; either
/// `DispatchFailed` (admission-time reject) or per-node `TypeMismatch`
/// (committed-then-failed bind) satisfies the wall's contract.
#[test]
fn functor_module_carrier_does_not_fill_type_slot() {
    let region = FrameStorage::run_root();
    let scope = run_root_silent(&region);
    run(
        scope,
        "FUNCTOR (MAKETREE Elt :Type) -> Module = (MODULE Generated = (LET inner = 1))\n\
         MODULE IntMod = (LET inner = 1)",
    );
    let _ = run_expecting_dispatch_error(scope, parse_one("MAKETREE IntMod"));
}

/// Deferred-return re-elaboration with a builtin-keyed bind — pins that the
/// unifier seam is agnostic to whether `Elt` was bound from a builtin or a
/// nominal carrier.
#[test]
fn functor_deferred_return_resolves_against_builtin_keyed_bind() {
    use crate::machine::model::ReturnType;
    let region = FrameStorage::run_root();
    let scope = run_root_silent(&region);
    run(scope, "FUNCTOR (BUILD Elt :Type) -> :Elt = (42)");
    let f = crate::builtins::test_support::lookup_fn(scope, "BUILD");
    assert!(
        matches!(f.signature.return_type, ReturnType::Deferred(_)),
        "BUILD's return type should be Deferred, got {:?}",
        f.signature.return_type,
    );
    let result = run_one(scope, parse_one("BUILD Number"));
    match result {
        KObject::Number(n) if *n == 42.0 => {}
        other => panic!(
            "expected Number(42) from BUILD Number, got {:?}",
            other.ktype()
        ),
    }
}

/// Wrong-typed body surfaces the per-call `TypeMismatch` diagnostic (same
/// wording as the nominal-keyed path), pinning that builtin-keyed binds
/// route through the same dep-finish slot check.
#[test]
fn functor_deferred_return_builtin_keyed_mismatch_surfaces_per_call_diagnostic() {
    use crate::machine::execute::KoanRuntime;
    let region = FrameStorage::run_root();
    let scope = run_root_silent(&region);
    run(scope, "FUNCTOR (BUILD Elt :Type) -> :Elt = (42)");
    let mut sched = KoanRuntime::new();
    let id = sched.dispatch_in_scope(parse_one("BUILD Str"), scope);
    sched
        .execute()
        .expect("execute does not surface per-slot errors");
    let err = match sched.read_result(id) {
        Err(e) => e,
        Ok(_) => panic!("BUILD Str should fail the per-call return-type check"),
    };
    match &err.kind {
        KErrorKind::TypeMismatch { arg, expected, .. } => {
            assert_eq!(arg, "<return>");
            assert!(
                expected.contains("per-call return type"),
                "expected per-call return-type diagnostic, got `{expected}`",
            );
        }
        _ => panic!("expected TypeMismatch on <return>, got {err}"),
    }
}
