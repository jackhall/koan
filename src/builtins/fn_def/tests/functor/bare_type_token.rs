//! Bare type tokens (`Number`, `Str`, `Bool`, `Null`) as `:Type`-typed
//! FN arguments. Pins the widening at
//! [`KType::accepts_part`](crate::machine::model::types) and the
//! deferred-return re-elaboration path's agnosticism to builtin-vs-nominal
//! carriers.

use crate::builtins::test_support::{parse_one, run, run_one, run_root_silent};
use crate::machine::model::KExpression;
use crate::machine::model::{KObject, KType, Parseable};
use crate::machine::run_root_storage;
use crate::machine::KoanRuntime;
use crate::machine::{KError, KErrorKind, Scope};

/// Tolerates the error surfacing either from `KoanRuntime::execute()` (resolve
/// rejects at admission) or from `read_result_with` (auto-wrap committed and bind
/// later refused). Compare `test_support::run_one_err`, which panics on the
/// first path.
fn run_expecting_dispatch_error<'a>(scope: &'a Scope<'a>, expr: KExpression<'a>) -> KError {
    let mut runtime = KoanRuntime::new();
    let id = runtime.dispatch_in_scope(expr, scope);
    match runtime.execute() {
        Err(e) => e,
        Ok(()) => match runtime.read_result_with(id, |v| v.ktype().name().to_string()) {
            Err(e) => e.clone(),
            Ok(type_name) => {
                panic!("expected dispatch-level error, got value of type {type_name}",)
            }
        },
    }
}

#[test]
fn functor_admits_bare_number_token_at_type_slot() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "FN (MAKETREE Elt :Type) -> Module = (MODULE generated = (LET inner = 1))",
    );
    let result = run_one(scope, parse_one("MAKETREE Number"));
    match result {
        KObject::Module(_) => {}
        other => {
            panic!(
                "expected MAKETREE Number to dispatch and return a module, got {}",
                other.summarize()
            )
        }
    }
}

#[test]
fn functor_admits_bare_str_bool_null_tokens_at_type_slot() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "FN (MAKETREE Elt :Type) -> Module = (MODULE generated = (LET inner = 1))",
    );
    for token in ["Str", "Bool", "Null"] {
        let src = format!("MAKETREE {token}");
        let result = run_one(scope, parse_one(&src));
        match result {
            KObject::Module(_) => {}
            other => {
                panic!(
                    "expected MAKETREE {token} to dispatch and return a module, got {}",
                    other.summarize()
                )
            }
        }
    }
}

#[test]
fn functor_per_call_type_side_bind_is_observable_via_module_type_members() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "FN (MAKETREE Elt :Type) -> Module = \
         (MODULE generated = ((LET ElemType = Elt) (LET inner = 1)))",
    );
    let result = run_one(scope, parse_one("MAKETREE Number"));
    let module = match result {
        KObject::Module(module) => *module,
        other => panic!("expected module result, got {}", other.summarize()),
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
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "FN (MAKETREE Elt :Type) -> Module = (MODULE generated = (LET inner = 1))",
    );
    let err = run_expecting_dispatch_error(scope, parse_one("MAKETREE 7"));
    match &err.kind {
        KErrorKind::DispatchFailed { .. } | KErrorKind::UnboundName(_) => {}
        _ => panic!("expected dispatch no-match (DispatchFailed) for non-type carrier, got {err}",),
    }
}

/// Module carriers stay out of `:Type` slots — the cut-(a) wall at
/// [`KType::accepts_part`], where a `Spliced` cell carrying the module **value** opens and is
/// refused (a value is never matched by a kind). Asserts only that no value comes back; either
/// `DispatchFailed` (admission-time reject) or per-node `TypeMismatch`
/// (committed-then-failed bind) satisfies the wall's contract.
#[test]
fn functor_module_carrier_does_not_fill_type_slot() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "FN (MAKETREE Elt :Type) -> Module = (MODULE generated = (LET inner = 1))\n\
         MODULE int_mod = (LET inner = 1)",
    );
    let _ = run_expecting_dispatch_error(scope, parse_one("MAKETREE int_mod"));
}

/// Deferred-return re-elaboration with a builtin-keyed bind — pins that the
/// unifier seam is agnostic to whether `Elt` was bound from a builtin or a
/// nominal carrier.
#[test]
fn deferred_return_resolves_against_builtin_keyed_bind() {
    use crate::machine::model::ReturnType;
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(scope, "FN (BUILD Elt :Type) -> :Elt = (42)");
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
fn deferred_return_builtin_keyed_mismatch_surfaces_per_call_diagnostic() {
    use crate::machine::KoanRuntime;
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(scope, "FN (BUILD Elt :Type) -> :Elt = (42)");
    let mut runtime = KoanRuntime::new();
    let id = runtime.dispatch_in_scope(parse_one("BUILD Str"), scope);
    runtime
        .execute()
        .expect("execute does not surface per-slot errors");
    let err = match runtime.result_error(id) {
        Err(e) => e,
        Ok(()) => panic!("BUILD Str should fail the per-call return-type check"),
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
