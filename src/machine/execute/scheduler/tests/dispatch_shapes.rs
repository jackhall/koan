//! Integration tests for the no-keyword fast-lane dispatch (PR B of the unified-walk
//! roadmap item). Each test exercises one variant of `DispatchShape` and asserts both
//! the surface behavior (correct return value) and the routing claim (fast-lane
//! shapes don't enter `resolve_dispatch_with_chain`, keyworded shapes do).
//!
//! Routing assertions use the test-only counter on
//! [`crate::machine::core::resolve_dispatch_entry_count`]. Each test resets the
//! counter, runs the dispatch, and reads back — `0` proves the fast lane bypassed
//! the candidate machinery; `≥1` proves the keyworded pipeline ran. The counter is
//! thread-local so tests run independently under `cargo test`'s default thread pool.
//!
//! See `roadmap/dispatch_fix/unified-walk.md` and `scratch/plan-unified-walk.md` D4 /
//! D5 / D10 for the routing contract.

use crate::builtins::default_scope;
use crate::builtins::test_support::parse_one;
use crate::machine::core::kfunction::{ArgumentBundle, BodyResult, SchedulerHandle as KfHandle};
use crate::machine::core::{
    reset_resolve_dispatch_entry_count, resolve_dispatch_entry_count,
};
use crate::machine::execute::Scheduler;
use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::model::types::{
    Argument, ExpressionSignature, KType, ReturnType, SignatureElement,
};
use crate::machine::model::{KObject, Parseable};
use crate::machine::{BindingIndex, KFunction, RuntimeArena, Scope};
use crate::machine::core::source::Spanned;

/// Submit `expr` against `scope` and return its terminal value. Mirrors
/// `test_support::run_one` but is in-module so each fast-lane test can call it
/// without re-importing.
fn dispatch_one<'a>(scope: &'a Scope<'a>, expr: KExpression<'a>) -> &'a KObject<'a> {
    let mut sched = Scheduler::new();
    let id = sched.add_dispatch(expr, scope);
    sched.execute().expect("scheduler should succeed");
    sched.read(id)
}

/// `body_identity` — accepts one Number arg and returns it unchanged. Used by the
/// `keyworded_unchanged_with_keyword_in_body` test to register a function value `f`
/// against which the `(f IF x)` keyword-in-body probe routes through the keyworded
/// path. The signature is `<n :Number>` (no keywords), which means no koan user
/// surface can call it directly — the test only inspects routing, never the call
/// outcome.
fn body_identity<'a>(
    _scope: &'a Scope<'a>,
    _sched: &mut dyn KfHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    match bundle.get("n") {
        Some(obj) => BodyResult::Value(_scope.arena.alloc(obj.deep_clone())),
        None => BodyResult::Err(crate::machine::KError::new(
            crate::machine::KErrorKind::MissingArg("n".to_string()),
        )),
    }
}

/// Bind a function value `f` with signature `<n :Number>` on `scope`. Only used by
/// `keyworded_unchanged_with_keyword_in_body` to give `(f IF x)` a real Identifier
/// head to resolve — the named-arg fast-lane tests use the FN/LET user surface
/// directly.
fn bind_identity_fn<'a>(scope: &'a Scope<'a>) {
    let sig = ExpressionSignature {
        return_type: ReturnType::Resolved(KType::Number),
        elements: vec![SignatureElement::Argument(Argument {
            name: "n".into(),
            ktype: KType::Number,
        })],
    };
    let f = scope.arena.alloc_function(KFunction::new(
        sig,
        crate::machine::core::kfunction::Body::Builtin(body_identity),
        scope,
    ));
    let obj = scope.arena.alloc(KObject::KFunction(f, None));
    scope
        .bind_value("f".to_string(), obj, BindingIndex::BUILTIN)
        .expect("bind_value should succeed");
}

/// `(Number)` — single bare leaf Type token. Classifies as `BareTypeLeaf`; the
/// fast-lane handler routes through `coerce_type_token_value`, never entering
/// `resolve_dispatch_with_chain`.
#[test]
fn bare_type_leaf_short_circuits() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let expr = parse_one("(Number)");
    reset_resolve_dispatch_entry_count();
    let result = dispatch_one(scope, expr);
    assert_eq!(
        resolve_dispatch_entry_count(),
        0,
        "BareTypeLeaf must not enter resolve_dispatch_with_chain",
    );
    assert!(
        matches!(result, KObject::KTypeValue(KType::Number)),
        "(Number) must terminate to KTypeValue(Number); got {}",
        result.summarize(),
    );
}

/// `(List Number)` — parens-form type-call with leaf-Type-only args. Classifies as
/// `TypeCall`; the fast lane elaborates `TypeExpr { name: "List", params: List([Number]) }`
/// via `scope.resolve_type_expr` and wraps the result as a `KTypeValue` carrier.
#[test]
fn type_call_short_circuits() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let expr = parse_one("(List Number)");
    reset_resolve_dispatch_entry_count();
    let result = dispatch_one(scope, expr);
    assert_eq!(
        resolve_dispatch_entry_count(),
        0,
        "TypeCall must not enter resolve_dispatch_with_chain",
    );
    match result {
        KObject::KTypeValue(KType::List(elem)) => assert_eq!(
            elem.name(),
            "Number",
            "(List Number) must carry KType::List(Number)",
        ),
        other => panic!(
            "(List Number) must terminate to KTypeValue(List(Number)); got {}",
            other.summarize()
        ),
    }
}

/// User-facing named-arg path. `LET f = (FN (DOUBLE x :Number) -> Number = (x))`
/// registers a function whose signature is `[Keyword("DOUBLE"), Argument(x :Number)]`.
/// Calling `f (x = 7)` routes through the fast lane's *named-arg admission*: positional
/// `matches` fails (1 arg vs 2 elements), `matches_without_keywords` succeeds (the
/// signature's lone Argument `x` is present with a Number value, keyword `DOUBLE`
/// elided), and the fast lane reconstructs the positional expression via
/// `KFunction::reconstruct_positional` and binds directly. **The counter must read
/// 0** — the previous fall-through routed through `call_by_name`, which entered
/// `resolve_dispatch_with_chain` at least twice.
#[test]
fn function_value_call_named_args_short_circuits() {
    use crate::builtins::test_support::{run, run_root_silent};
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "LET f = (FN (DOUBLE x :Number) -> Number = (x))");
    let expr = parse_one("f (x = 7)");
    reset_resolve_dispatch_entry_count();
    let result = dispatch_one(scope, expr);
    assert_eq!(
        resolve_dispatch_entry_count(),
        0,
        "(f (x = 7)) with f = (FN (DOUBLE x :Number) ...) must fast-lane bypass \
         resolve_dispatch_with_chain via matches_without_keywords; counter was {}",
        resolve_dispatch_entry_count(),
    );
    assert!(
        matches!(result, KObject::Number(n) if (*n - 7.0).abs() < 1e-9),
        "(f (x = 7)) must evaluate to 7.0 (DOUBLE returns x); got {}",
        result.summarize(),
    );
}

/// Named-arg path with reordering. `LET f = (FN (a :Number PICK b :Number) ...)` —
/// signature is `[Argument(a), Keyword(PICK), Argument(b)]`. Calling
/// `f (b = 2, a = 1)` reorders the args vs the signature's positional layout;
/// `matches_without_keywords` reports `true` because the name-keyed lookup is
/// order-independent. The reconstructed positional form weaves keywords back in at
/// the right positions.
#[test]
fn function_value_call_named_args_out_of_order_short_circuits() {
    use crate::builtins::test_support::{run, run_root_silent};
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "LET f = (FN (a :Number PICK b :Number) -> Number = (a))");
    let expr = parse_one("f (b = 2, a = 1)");
    reset_resolve_dispatch_entry_count();
    let result = dispatch_one(scope, expr);
    assert_eq!(
        resolve_dispatch_entry_count(),
        0,
        "out-of-order named args must still fast-lane; counter was {}",
        resolve_dispatch_entry_count(),
    );
    assert!(
        matches!(result, KObject::Number(n) if (*n - 1.0).abs() < 1e-9),
        "(f (b = 2, a = 1)) returning `a` must yield 1.0; got {}",
        result.summarize(),
    );
}

/// Named-arg path with a missing required arg. `f (a = 1)` against
/// `(a :Number PICK b :Number)` — `b` is missing. `matches_without_keywords` returns
/// `false`, the fast lane falls through to the keyworded path, and `call_by_name`'s
/// body surfaces `MissingArg("b")`. The counter must advance (the keyworded path
/// ran). Mirrors `call_by_name::tests::call_by_name_missing_named_arg`.
#[test]
fn function_value_call_named_args_missing_falls_through() {
    use crate::builtins::test_support::{run, run_root_silent};
    use crate::machine::KErrorKind;
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "LET f = (FN (a :Number PICK b :Number) -> Number = (a))");
    let expr = parse_one("f (a = 1)");
    reset_resolve_dispatch_entry_count();
    let mut sched = Scheduler::new();
    let id = sched.add_dispatch(expr, scope);
    sched.execute().expect("scheduler should not surface errors directly");
    let err = match sched.read_result(id) {
        Err(e) => e.clone(),
        Ok(v) => panic!("expected MissingArg error, got value {}", v.summarize()),
    };
    assert!(
        resolve_dispatch_entry_count() >= 1,
        "missing-arg case must fall through to the keyworded path (counter advance); \
         counter was {}",
        resolve_dispatch_entry_count(),
    );
    assert!(
        matches!(&err.kind, KErrorKind::MissingArg(name) if name == "b"),
        "expected MissingArg(\"b\"), got {err}",
    );
}

/// `f (x = 7)` submitted as a *forward reference*: `f` is installed as a
/// `Placeholder` on `scope` before the slot is dispatched. The fast lane's
/// `FunctionValueCall` handler hits the `Placeholder` arm (which fires on the
/// head-resolution, before the args-shape check) and installs a combined park,
/// never entering `resolve_dispatch_with_chain`.
///
/// Routing claim: the counter reads `0`. The `f (x = 7)` slot's run_dispatch saw
/// the `Placeholder`, installed the park, and returned without ever touching the
/// candidate machinery. The producer is a `BareIdentifier` shape pointing at a
/// pre-bound value, so the producer's own dispatch also fast-lanes (no counter
/// advance there either).
#[test]
fn function_value_call_forward_ref_parks() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let mut sched = Scheduler::new();

    // Pre-allocate a placeholder-backing slot. The slot's actual content doesn't
    // matter — `f (x = 7)`'s run_dispatch installs a park edge on it and returns
    // without inspecting the args at all (the Placeholder arm fires on the head
    // resolution). Bind `producer_target` to a value first so the producer's own
    // dispatch also fast-lanes (BareIdentifier with a Resolution::Value hit, no
    // counter advance) — otherwise the unbound fall-through would advance the
    // counter and defeat the routing assertion.
    let producer_target = scope.arena.alloc(KObject::Number(42.0));
    scope
        .bind_value("producer_target".to_string(), producer_target, BindingIndex::BUILTIN)
        .expect("bind_value should succeed");
    let producer = sched.add_dispatch(
        KExpression::new(vec![Spanned::bare(ExpressionPart::Identifier(
            "producer_target".into(),
        ))]),
        scope,
    );
    scope
        .install_placeholder("f".to_string(), producer, BindingIndex::BUILTIN)
        .expect("install_placeholder should succeed");

    // `f (x = 7)` — Identifier head plus a single nested-parens part. Classifies
    // as FunctionValueCall; the fast lane sees `Resolution::Placeholder` for `f`
    // and parks.
    let f_call = parse_one("f (x = 7)");
    let _f_call_id = sched.add_dispatch(f_call, scope);

    reset_resolve_dispatch_entry_count();
    let _ = sched.execute();
    assert_eq!(
        resolve_dispatch_entry_count(),
        0,
        "FunctionValueCall forward-ref park must not enter resolve_dispatch_with_chain; \
         the head-Placeholder arm fires before any args-shape inspection",
    );
}

/// `(PRINT 5)` — keyword-headed call routes to the registered `PRINT` builtin
/// through the candidate path. The classifier flags it as `Keyworded` (PRINT in
/// head is a Keyword); `resolve_dispatch_with_chain` runs at least once to find
/// the bucket.
#[test]
fn keyworded_unchanged() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let expr = parse_one("(PRINT 5)");
    reset_resolve_dispatch_entry_count();
    let _ = dispatch_one(scope, expr);
    assert!(
        resolve_dispatch_entry_count() >= 1,
        "Keyworded shape must enter resolve_dispatch_with_chain at least once; \
         count was {}",
        resolve_dispatch_entry_count(),
    );
}

/// Mixed shapes where the head is a fast-lane shape (leaf `Type` or `Identifier`)
/// but a keyword appears later in the parts list. The classifier's step-1 sweep
/// catches these and routes to `Keyworded`. Pins the D4 contract: "sweep first,
/// branch on head second".
///
/// Two probes:
/// - `(List MAYBE Number)`: head `List` is a leaf Type, body has keyword `MAYBE`.
///   No registered `(_ MAYBE _)` overload; the keyworded path surfaces a
///   `DispatchFailed`. We tolerate the error and read the counter.
/// - `(f IF x)`: head `f` is a lowercase Identifier, body has keyword `IF`. Same
///   routing story.
///
/// The assertion is purely routing: the counter must advance in both cases.
#[test]
fn keyworded_unchanged_with_keyword_in_body() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    bind_identity_fn(scope);

    // (List MAYBE Number) — leaf-Type head, keyword `MAYBE` in body.
    let expr_a = parse_one("(List MAYBE Number)");
    reset_resolve_dispatch_entry_count();
    let mut sched = Scheduler::new();
    sched.add_dispatch(expr_a, scope);
    let _ = sched.execute(); // DispatchFailed is fine — routing is what we test.
    assert!(
        resolve_dispatch_entry_count() >= 1,
        "(List MAYBE Number) must route to Keyworded (keyword in body); count was {}",
        resolve_dispatch_entry_count(),
    );

    // (f IF x) — lowercase Identifier head, keyword `IF` in body.
    let expr_b = parse_one("(f IF x)");
    reset_resolve_dispatch_entry_count();
    let mut sched = Scheduler::new();
    sched.add_dispatch(expr_b, scope);
    let _ = sched.execute();
    assert!(
        resolve_dispatch_entry_count() >= 1,
        "(f IF x) must route to Keyworded (keyword in body); count was {}",
        resolve_dispatch_entry_count(),
    );
}
