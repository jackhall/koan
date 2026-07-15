//! Integration tests for the no-keyword fast-lane dispatch. Each test exercises
//! one variant of `DispatchShape` and asserts both the surface behavior and the
//! routing claim (fast-lane shapes don't enter `resolve_dispatch`,
//! keyworded shapes do).
//!
//! Routing assertions use the test-only counter on
//! [`crate::machine::execute::dispatch::resolve_dispatch_entry_count`]. The counter is
//! thread-local so tests run independently under `cargo test`'s default thread
//! pool.

use crate::builtins::default_scope;
use crate::builtins::test_support::parse_one;
use crate::machine::core::run_root_storage;
use crate::machine::core::StoredReach;
use crate::machine::core::{arg_object, Action, BodyCtx};
use crate::machine::execute::dispatch::{
    reset_resolve_dispatch_entry_count, resolve_dispatch_entry_count,
};
use crate::machine::execute::KoanRuntime;
use crate::machine::model::Held;
use crate::machine::model::KExpression;
use crate::machine::model::{Argument, ExpressionSignature, KType, ReturnType, SignatureElement};
use crate::machine::model::{Carried, KObject, Parseable};
use crate::machine::{BindingIndex, KFunction, Scope};

fn dispatch_one<'run>(scope: &'run Scope<'run>, expr: KExpression<'run>) -> &'run KObject<'run> {
    sched_read_carried(scope, expr).object()
}

/// Like [`dispatch_one`] but yields the raw carrier, so a type-producing expression can be
/// inspected on its [`Carried::Type`] arm instead of panicking through `.object()`.
fn dispatch_one_carried<'run>(scope: &'run Scope<'run>, expr: KExpression<'run>) -> Carried<'run> {
    sched_read_carried(scope, expr)
}

fn sched_read_carried<'run>(scope: &'run Scope<'run>, expr: KExpression<'run>) -> Carried<'run> {
    let mut runtime = KoanRuntime::new();
    let id = runtime.dispatch_in_scope(expr, scope);
    runtime.execute().expect("scheduler should succeed");
    // The frameless top-level terminal outlives the local `runtime`; widen the scheduler's `'node`
    // read to the scope lifetime (see `test_support::extract_terminal`).
    crate::builtins::test_support::extract_terminal(&runtime, scope, id)
}

/// Accepts one Number arg and returns it unchanged. The signature is `<n :Number>`
/// (no keywords), which means no koan user surface can call it directly — tests
/// using it only inspect routing, never the call outcome.
fn body_identity<'run>(ctx: &BodyCtx<'run, '_>) -> Action<'run> {
    match arg_object(ctx.args, "n") {
        Some(obj) => Action::done_resident(Carried::Object(
            ctx.scope
                .brand()
                .alloc_object_checked(obj.deep_clone())
                .expect("a deep-cloned Number is always resident-in-self"),
        )),
        None => Action::Done(Err(crate::machine::KError::new(
            crate::machine::KErrorKind::MissingArg("n".to_string()),
        ))),
    }
}

/// Bind a function value `f` with signature `<n :Number>` on `scope`, giving an
/// Identifier head that resolves to a function value without going through FN/LET.
fn bind_identity_fn<'run>(scope: &'run Scope<'run>) {
    let sig = ExpressionSignature {
        return_type: ReturnType::Resolved(KType::Number),
        elements: vec![SignatureElement::Argument(Argument {
            name: "n".into(),
            ktype: KType::Number,
        })],
    };
    let f = scope.brand().alloc_function(KFunction::new(
        sig,
        crate::machine::core::Body::Builtin(body_identity),
        scope,
        None,
        None,
    ));
    let obj = scope
        .brand()
        .alloc_object_checked(KObject::KFunction(f))
        .expect("f was just allocated into region\'s own region");
    scope
        .bind_value(
            "f".to_string(),
            obj,
            BindingIndex::BUILTIN,
            StoredReach::for_test(None, false),
        )
        .expect("bind_value should succeed");
}

/// `(Number)` — single bare leaf Type token. Classifies as `BareTypeLeaf`; the
/// fast-lane handler routes through `Scope::resolve_type_identifier`.
#[test]
fn bare_type_leaf_short_circuits() {
    let region = run_root_storage();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    let expr = parse_one("(Number)");
    reset_resolve_dispatch_entry_count();
    let result = dispatch_one_carried(scope, expr);
    assert_eq!(
        resolve_dispatch_entry_count(),
        0,
        "BareTypeLeaf must not enter resolve_dispatch",
    );
    assert!(
        matches!(result, Carried::Type(KType::Number)),
        "(Number) must terminate to a Number type; got {}",
        result.summarize(),
    );
}

/// User-facing named-arg path. `f {x = 7}` against a signature with a leading
/// keyword `DOUBLE` elides the keyword via the fast lane's named-arg admission,
/// reconstructs the positional expression, and binds directly with
/// `picked = Some(f)` — no entry into `resolve_dispatch`.
#[test]
fn function_value_call_named_args_short_circuits() {
    use crate::builtins::test_support::{run, run_root_silent};
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(scope, "LET f = (FN (DOUBLE x :Number) -> Number = (x))");
    let expr = parse_one("f {x = 7}");
    reset_resolve_dispatch_entry_count();
    let result = dispatch_one(scope, expr);
    assert_eq!(
        resolve_dispatch_entry_count(),
        0,
        "(f {{x = 7}}) with f = (FN (DOUBLE x :Number) ...) must fast-lane bypass \
         resolve_dispatch; counter was {}",
        resolve_dispatch_entry_count(),
    );
    assert!(
        matches!(result, KObject::Number(n) if (*n - 7.0).abs() < 1e-9),
        "(f {{x = 7}}) must evaluate to 7.0 (DOUBLE returns x); got {}",
        result.summarize(),
    );
}

/// Named-arg path with reordering. `f {b = 2, a = 1}` against a signature
/// `(a :Number PICK b :Number)` is order-independent at the name-keyed lookup;
/// reconstruction weaves keywords back in at their signature positions.
#[test]
fn function_value_call_named_args_out_of_order_short_circuits() {
    use crate::builtins::test_support::{run, run_root_silent};
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "LET f = (FN (a :Number PICK b :Number) -> Number = (a))",
    );
    let expr = parse_one("f {b = 2, a = 1}");
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
        "(f {{b = 2, a = 1}}) returning `a` must yield 1.0; got {}",
        result.summarize(),
    );
}

/// Named-arg path with a missing required arg. `MissingArg("b")` surfaces from
/// inside the fast-lane handler (`reconstruct_positional`'s structured error)
/// without falling through to the keyworded pipeline.
#[test]
fn function_value_call_named_args_missing_short_circuits() {
    use crate::builtins::test_support::{run, run_root_silent};
    use crate::machine::KErrorKind;
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "LET f = (FN (a :Number PICK b :Number) -> Number = (a))",
    );
    let expr = parse_one("f {a = 1}");
    reset_resolve_dispatch_entry_count();
    let mut runtime = KoanRuntime::new();
    let id = runtime.dispatch_in_scope(expr, scope);
    runtime
        .execute()
        .expect("scheduler should not surface errors directly");
    let err = match runtime.read_result_with(id, |v| v.summarize()) {
        Err(e) => e.clone(),
        Ok(summary) => panic!("expected MissingArg error, got value {summary}"),
    };
    assert_eq!(
        resolve_dispatch_entry_count(),
        0,
        "missing-arg case must short-circuit in the fast lane via \
         reconstruct_positional's KError; counter was {}",
        resolve_dispatch_entry_count(),
    );
    assert!(
        matches!(&err.kind, KErrorKind::MissingArg(name) if name == "b"),
        "expected MissingArg(\"b\"), got {err}",
    );
}

// =====================================================================
// Surface assertions on the named-arg fast lane, paired with a routing claim
// (`resolve_dispatch_entry_count == 0`) to pin that the fast lane handles each
// case rather than falling through.
// =====================================================================

/// `f {x = 7}` against `(FN (DOUBLE x :Number) ...)` — function-value call via
/// named-arg admission, fast-lane bound directly.
#[test]
fn fast_lane_fn_callable_via_named_args() {
    use crate::builtins::test_support::{run, run_one, run_root_silent};
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(scope, "LET f = (FN (DOUBLE x :Number) -> Number = (x))");
    reset_resolve_dispatch_entry_count();
    let result = run_one(scope, parse_one("f {x = 7}"));
    assert_eq!(
        resolve_dispatch_entry_count(),
        0,
        "named-arg call must fast-lane bypass resolve_dispatch; \
         counter was {}",
        resolve_dispatch_entry_count(),
    );
    assert!(matches!(result, KObject::Number(n) if *n == 7.0));
}

/// Internal keyword in a non-leading signature slot must be re-woven between
/// reordered args at reconstruction time.
#[test]
fn fast_lane_weaves_internal_keyword() {
    use crate::builtins::test_support::{run, run_one, run_root_silent};
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "LET f = (FN (a :Number PICK b :Number) -> Number = (a))",
    );
    reset_resolve_dispatch_entry_count();
    let result = run_one(scope, parse_one("f {a = 1, b = 2}"));
    assert_eq!(resolve_dispatch_entry_count(), 0);
    assert!(matches!(result, KObject::Number(n) if *n == 1.0));
}

/// Named-arg lookup is by name, not position: `(b = 2, a = 1)` satisfies
/// `(a PICK b)` the same as `(a = 1, b = 2)`.
#[test]
fn fast_lane_named_args_order_independent() {
    use crate::builtins::test_support::{run, run_one, run_root_silent};
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "LET f = (FN (a :Number PICK b :Number) -> Number = (a))",
    );
    reset_resolve_dispatch_entry_count();
    let result = run_one(scope, parse_one("f {b = 2, a = 1}"));
    assert_eq!(resolve_dispatch_entry_count(), 0);
    assert!(matches!(result, KObject::Number(n) if *n == 1.0));
}

/// Width-drop: a named arg with no matching declared parameter is dropped, not
/// rejected. `(a = 1, b = 2, c = 3)` covers the required names plus an extra `c`; the
/// surplus `c` goes unbound on the reconstructed exact-arity expression and the call
/// returns `Number(1)`.
#[test]
fn fast_lane_extra_named_arg_dropped() {
    use crate::builtins::test_support::{run, run_one, run_root_silent};
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "LET f = (FN (a :Number PICK b :Number) -> Number = (a))",
    );
    reset_resolve_dispatch_entry_count();
    let result = run_one(scope, parse_one("f {a = 1, b = 2, c = 3}"));
    assert_eq!(resolve_dispatch_entry_count(), 0);
    assert!(matches!(result, KObject::Number(n) if *n == 1.0));
}

/// The legacy paren named-arg form `f (a 1)` no longer binds — a function call's
/// arguments are a record literal `{a = 1}`. The fast lane rejects the paren body
/// loudly (`DispatchFailed`) without entering `resolve_dispatch`.
#[test]
fn fast_lane_legacy_paren_args_rejected() {
    use crate::builtins::test_support::{run, run_one_err, run_root_silent};
    use crate::machine::KErrorKind;
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(scope, "LET f = (FN (DOUBLE x :Number) -> Number = (x))");
    reset_resolve_dispatch_entry_count();
    let err = run_one_err(scope, parse_one("f (a 1)"));
    assert_eq!(resolve_dispatch_entry_count(), 0);
    assert!(
        matches!(&err.kind, KErrorKind::DispatchFailed { reason, .. } if reason.contains("record literal")),
        "expected loud rejection of the paren named-arg form, got {err}",
    );
}

/// Duplicate named-arg is caught by `NamedPairs::from_fields` at construction time.
#[test]
fn fast_lane_duplicate_named_arg() {
    use crate::builtins::test_support::{run, run_one_err, run_root_silent};
    use crate::machine::KErrorKind;
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(scope, "LET f = (FN (DOUBLE x :Number) -> Number = (x))");
    reset_resolve_dispatch_entry_count();
    let err = run_one_err(scope, parse_one("f {x = 1, x = 2}"));
    assert_eq!(resolve_dispatch_entry_count(), 0);
    assert!(
        matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("duplicate") && msg.contains("`x`")),
        "expected ShapeError on duplicate name, got {err}",
    );
}

/// Non-function head resolves to a value-side carrier the fast lane refuses with
/// `TypeMismatch { arg: "verb", expected: "KFunction or Type" }`. Verb-precedence
/// (verb resolves before pair parsing) holds because head resolution is the first
/// match arm.
#[test]
fn fast_lane_on_non_function_returns_error() {
    use crate::builtins::test_support::{run, run_one_err, run_root_silent};
    use crate::machine::KErrorKind;
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(scope, "LET x = 42");
    reset_resolve_dispatch_entry_count();
    let err = run_one_err(scope, parse_one("x {foo = 7}"));
    assert_eq!(resolve_dispatch_entry_count(), 0);
    assert!(
        matches!(
            &err.kind,
            KErrorKind::TypeMismatch { arg, expected, .. }
                if arg == "verb" && expected == "KFunction or Type"
        ),
        "expected TypeMismatch on verb, got {err}",
    );
}

/// Tagged-union construction through the type name. The TypeCall fast lane
/// resolves `Maybe` to its `KTypeValue(UserType { Tagged { schema } })` identity
/// and constructs from the schema payload via
/// `constructors::dispatch_construct_tagged`.
///
/// Counter contract: every step in the chain (TypeCall head resolution +
/// construct-from-identity + LiteralPassThrough on the value-cell) is fast-lane;
/// nothing enters `resolve_dispatch`.
#[test]
fn fast_lane_on_tagged_union_constructs() {
    use crate::builtins::test_support::{run, run_one, run_root_silent};
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(scope, "UNION Maybe = (Some :Number None :Null)");
    reset_resolve_dispatch_entry_count();
    let result = run_one(scope, parse_one("Maybe (Some 42)"));
    assert_eq!(
        resolve_dispatch_entry_count(),
        0,
        "tagged-union construction is fully fast-lane: no `resolve_dispatch` \
         entries. Counter was {}",
        resolve_dispatch_entry_count(),
    );
    // A user-union variant value is an ordinary `Wrapped` over the member `SetRef`.
    match result {
        KObject::Wrapped { inner, type_id } => {
            assert!(matches!(inner.get(), KObject::Number(n) if *n == 42.0));
            match type_id {
                crate::machine::model::KType::SetRef { set, index } => {
                    assert_eq!(set.member(*index).name, "Some");
                }
                other => panic!("expected a member SetRef type_id, got {other:?}"),
            }
        }
        other => panic!("expected Wrapped, got {:?}", other.ktype()),
    }
}

/// Struct construction through the type name. `STRUCT Pt = ...` registers the
/// `KTypeValue(UserType { Struct { fields } })` identity type-side; the TypeCall
/// fast lane constructs from the fields payload via
/// `constructors::dispatch_construct_struct`.
///
/// Counter contract: every step is fast-lane (TypeCall head resolution +
/// construct-from-identity + LiteralPassThrough per value-cell); no entry into
/// `resolve_dispatch`.
#[test]
fn fast_lane_on_newtype_record_type_constructs() {
    use crate::builtins::test_support::{run, run_one, run_root_silent};
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(scope, "NEWTYPE Pt = :{x :Number, y :Number}");
    reset_resolve_dispatch_entry_count();
    let result = run_one(scope, parse_one("Pt {x = 3, y = 4}"));
    assert_eq!(
        resolve_dispatch_entry_count(),
        0,
        "record-repr newtype construction is fully fast-lane: no `resolve_dispatch` \
         entries. Counter was {}",
        resolve_dispatch_entry_count(),
    );
    match result {
        KObject::Wrapped { inner, type_id } => {
            assert_eq!(type_id.name(), "Pt");
            match inner.get() {
                KObject::Record(values, _) => {
                    assert!(
                        matches!(values.get("x"), Some(Held::Object(KObject::Number(n))) if *n == 3.0)
                    );
                    assert!(
                        matches!(values.get("y"), Some(Held::Object(KObject::Number(n))) if *n == 4.0)
                    );
                }
                other => panic!("expected record inner, got {:?}", other.ktype()),
            }
        }
        other => panic!("expected Wrapped, got {:?}", other.ktype()),
    }
}

/// Single-part literal-shaped expressions — `(99)`, `("x")`, `([1 2 3])`,
/// `({a = 1})`, `((inner))` — route through `LiteralPassThrough` instead of
/// bucket-dispatching `value_pass`. The counter must stay at 0 for `(99)`
/// because the fast lane surfaces the literal without consulting buckets.
#[test]
fn literal_pass_through_routes_via_fast_lane() {
    use crate::builtins::test_support::{run_one, run_root_silent};
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    reset_resolve_dispatch_entry_count();
    let result = run_one(scope, parse_one("(99)"));
    assert_eq!(
        resolve_dispatch_entry_count(),
        0,
        "single-literal must bypass bucket dispatch; counter was {}",
        resolve_dispatch_entry_count(),
    );
    assert!(matches!(result, KObject::Number(n) if *n == 99.0));
}

/// `([1 2 3])` parks the slot on a scheduler-side list-literal producer via the
/// `Lift(Pending)` shape, never entering `resolve_dispatch`.
#[test]
fn literal_pass_through_routes_list_literal_via_fast_lane() {
    use crate::builtins::test_support::{run_one, run_root_silent};
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    reset_resolve_dispatch_entry_count();
    let result = run_one(scope, parse_one("([1 2 3])"));
    assert_eq!(resolve_dispatch_entry_count(), 0);
    match result {
        KObject::List(items, _) => {
            assert_eq!(items.len(), 3);
        }
        other => panic!("expected List, got {:?}", other.ktype()),
    }
}

/// Unbound head surfaces `UnboundName(name)` directly from the fast-lane handler;
/// no fall-through to the keyworded pipeline.
#[test]
fn fast_lane_unbound_returns_error() {
    use crate::builtins::test_support::{run_one_err, run_root_silent};
    use crate::machine::KErrorKind;
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    reset_resolve_dispatch_entry_count();
    let err = run_one_err(scope, parse_one("undefined {foo = 7}"));
    assert_eq!(resolve_dispatch_entry_count(), 0);
    assert!(
        matches!(&err.kind, KErrorKind::UnboundName(name) if name == "undefined"),
        "expected UnboundName(\"undefined\"), got {err}",
    );
}

/// A closure returned out of its defining call remains invocable. The escaped
/// `KObject::KFunction` rides a bare `&KFunction` borrow into the per-call region (where the
/// inner function's storage and captured scope live); the result carrier's witness set keeps
/// that region alive past frame drop, so the later invocation does not dangle.
#[test]
fn fast_lane_closure_escapes_outer_call_and_remains_invocable() {
    use crate::builtins::test_support::{run, run_one, run_root_silent};
    use crate::machine::model::Parseable;
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "FN (MAKE) -> :(FN () -> Str) = (FN (INNER) -> Str = (\"hi\"))\n\
         LET f = (MAKE)",
    );
    let result = run_one(scope, parse_one("f {}"));
    assert!(
        matches!(result, KObject::KString(s) if s == "hi"),
        "expected KString(\"hi\"), got {}",
        result.summarize(),
    );
}

/// Closure-lifetime variant exercising parameter-binding via the captured scope
/// after escape.
#[test]
fn fast_lane_escaped_closure_with_param_returns_body_value() {
    use crate::builtins::test_support::{run, run_one, run_root_silent};
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "FN (MAKE) -> :(FN (x :Number) -> Number) = (FN (ECHO x :Number) -> Number = (x))\n\
         LET f = (MAKE)",
    );
    let result = run_one(scope, parse_one("f {x = 42}"));
    assert!(matches!(result, KObject::Number(n) if *n == 42.0));
}

/// A list-borne escaping closure survives leaving the per-call region that built it: `MAKE`
/// returns a `List` holding an inner `KFunction` whose captured scope lived in `MAKE`'s now-freed
/// call region. The closure rides a bare `&KFunction` borrow into that region; the result
/// carrier's witness set keeps the region alive, so reading the element does not dangle. (Under
/// Miri this is the load-bearing no-use-after-free check.)
#[test]
fn fast_lane_list_of_closures_escapes_outer_call() {
    use crate::builtins::test_support::{run, run_one, run_root_silent};
    use crate::machine::model::Parseable;
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "FN (MAKE) -> List = ([(FN (ECHO x :Number) -> Number = (x))])",
    );
    let result = run_one(scope, parse_one("(MAKE)"));
    let items = match result {
        KObject::List(items, _) => items,
        other => panic!("expected MAKE to return a List, got {}", other.summarize()),
    };
    assert_eq!(items.len(), 1, "list should hold the single inner closure");
    assert!(
        matches!(&items[0], Held::Object(KObject::KFunction(_))),
        "list element should be the escaped inner closure, intact after its call region freed",
    );
}

/// `f {x = 7}` submitted as a forward reference: `f` is installed as a `Placeholder`
/// on `scope` before the slot is dispatched. The fast lane's `FunctionValueCall`
/// handler hits the `Placeholder` arm on head-resolution (before the args-shape
/// check), routing without entering `resolve_dispatch`. The producer here finalizes
/// with an error, so the head arm propagates it to the call slot — the reachable
/// ready-producer case. (A ready *ok* producer can't occur: a binder's successful
/// finalize binds the name, which then resolves to a `Value`, not a `Placeholder`.)
#[test]
fn function_value_call_forward_ref_routes_via_placeholder() {
    let region = run_root_storage();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    let mut runtime = KoanRuntime::new();

    // The producer is a `FunctionValueCall` on a non-function value: the fast lane
    // errors with `TypeMismatch` (a `Number` head isn't callable) without entering
    // `resolve_dispatch`, so the producer finalizes `Err` and the routing counter stays
    // clean. `f` is then a backward-visible placeholder pointing at it.
    let producer_target = scope.brand().alloc_object(KObject::Number(42.0));
    scope
        .bind_value(
            "producer_target".to_string(),
            producer_target,
            BindingIndex::BUILTIN,
            StoredReach::for_test(None, false),
        )
        .expect("bind_value should succeed");
    let producer = runtime.dispatch_in_scope(parse_one("producer_target {y = 1}"), scope);
    scope
        .install_placeholder(
            "f".to_string(),
            producer,
            BindingIndex::BUILTIN,
            crate::machine::BindKind::Value,
        )
        .expect("install_placeholder should succeed");

    let f_call_id = runtime.dispatch_in_scope(parse_one("f {x = 7}"), scope);

    reset_resolve_dispatch_entry_count();
    let _ = runtime.execute();
    assert_eq!(
        resolve_dispatch_entry_count(),
        0,
        "FunctionValueCall forward-ref must route through the head-Placeholder arm \
         before any args-shape inspection — never entering resolve_dispatch",
    );
    assert!(
        runtime.result_error(f_call_id).is_err(),
        "the head-Placeholder arm must propagate the ready producer's error to the call slot",
    );
}

/// `(PRINT 5)` — keyword-headed call routes through the candidate path.
/// `resolve_dispatch` runs at least once to find the bucket.
#[test]
fn keyworded_unchanged() {
    let region = run_root_storage();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    let expr = parse_one("(PRINT 5)");
    reset_resolve_dispatch_entry_count();
    let _ = dispatch_one(scope, expr);
    assert!(
        resolve_dispatch_entry_count() >= 1,
        "Keyworded shape must enter resolve_dispatch at least once; \
         count was {}",
        resolve_dispatch_entry_count(),
    );
}

// =====================================================================
// Direct classifier assertions: call `classify_dispatch_shape` and pattern-match
// the returned variant. Pin the classifier's branching directly, vs the routing
// tests above which observe the dispatch counter.
// =====================================================================

/// `(MyStruct {x = 1, y = 2})` — leaf-Type head, single nested-`Expression`
/// body. Classifier must route to `TypeCall`, not `Keyworded`.
#[test]
fn classifier_struct_construct_routes_to_type_call() {
    use crate::machine::execute::dispatch::{classify_dispatch_shape, DispatchShape};
    let expr = parse_one("MyStruct {x = 1, y = 2}");
    assert!(
        matches!(classify_dispatch_shape(&expr), DispatchShape::TypeCall),
        "expected TypeCall for `MyStruct {{x = 1, y = 2}}`",
    );
}

/// `(Maybe (Some 42))` — leaf-Type head, single nested-`Expression` body
/// holding `(Some 42)`. Must route to `TypeCall`.
#[test]
fn classifier_tagged_construct_routes_to_type_call() {
    use crate::machine::execute::dispatch::{classify_dispatch_shape, DispatchShape};
    let expr = parse_one("Maybe (Some 42)");
    assert!(
        matches!(classify_dispatch_shape(&expr), DispatchShape::TypeCall),
        "expected TypeCall for `Maybe (Some 42)`",
    );
}

/// `(Bar (x))` — leaf-Type head, nested-`Expression` body wrapping a single
/// identifier (the newtype-construction shape). Routes to `TypeCall`.
#[test]
fn classifier_newtype_construct_routes_to_type_call() {
    use crate::machine::execute::dispatch::{classify_dispatch_shape, DispatchShape};
    let expr = parse_one("Bar (x)");
    assert!(
        matches!(classify_dispatch_shape(&expr), DispatchShape::TypeCall),
        "expected TypeCall for `Bar (x)`",
    );
}

/// `(List Number)` — leaf-Type head, every arg a leaf Type. Every leaf-Type-
/// headed multi-part call routes through `TypeCall`. The keyworded
/// `LIST OF` overload is the supported way to elaborate `List<Number>`.
#[test]
fn classifier_legacy_positional_collapses_to_type_call() {
    use crate::machine::execute::dispatch::{classify_dispatch_shape, DispatchShape};
    let expr = parse_one("(List Number)");
    assert!(
        matches!(classify_dispatch_shape(&expr), DispatchShape::TypeCall),
        "leaf-Type head + leaf-Type args must classify as TypeCall",
    );
}

/// Mixed shapes where the head is a fast-lane shape (leaf `Type` or `Identifier`)
/// but a keyword appears later in the parts list. The classifier's step-1 sweep
/// catches these and routes to `Keyworded` — sweep first, branch on head second.
///
/// Two probes:
/// - `(List MAYBE Number)`: leaf-Type head, keyword `MAYBE` in body. No
///   registered overload, so the keyworded path surfaces a `DispatchFailed`;
///   we tolerate the error and read the counter.
/// - `(f IF x)`: lowercase Identifier head, keyword `IF` in body.
#[test]
fn keyworded_unchanged_with_keyword_in_body() {
    let region = run_root_storage();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    bind_identity_fn(scope);

    let expr_a = parse_one("(List MAYBE Number)");
    reset_resolve_dispatch_entry_count();
    let mut runtime = KoanRuntime::new();
    runtime.dispatch_in_scope(expr_a, scope);
    let _ = runtime.execute();
    assert!(
        resolve_dispatch_entry_count() >= 1,
        "(List MAYBE Number) must route to Keyworded (keyword in body); count was {}",
        resolve_dispatch_entry_count(),
    );

    let expr_b = parse_one("(f IF x)");
    reset_resolve_dispatch_entry_count();
    let mut runtime = KoanRuntime::new();
    runtime.dispatch_in_scope(expr_b, scope);
    let _ = runtime.execute();
    assert!(
        resolve_dispatch_entry_count() >= 1,
        "(f IF x) must route to Keyworded (keyword in body); count was {}",
        resolve_dispatch_entry_count(),
    );
}

/// A Keyworded dispatch whose initial resolve picks an overload but whose
/// value-cell parts need sub-Dispatch evaluation (the Resolved-with-eager-subs
/// arm) must terminate correctly under the stateful driver. Pins that the
/// eager-subs `AwaitDeps` finish re-resolves and binds inline through
/// `exec::invoke`.
///
/// Program: `LET y = (FIRST [1 2 3])`. LET picks at initial resolve; the RHS
/// is an eager sub-Dispatch. After the sub resolves to `1`, the resume handler
/// splices `Spliced(1)` into the LET expression and re-resolves.
#[test]
fn stateful_keyworded_eager_subs_resumes_through_state() {
    let region = run_root_storage();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    crate::builtins::test_support::run(scope, "FN (FIRST xs :(LIST OF Number)) -> Number = (1)");
    let mut runtime = KoanRuntime::new();
    let exprs = crate::parse::parse("LET y = (FIRST [1 2 3])").expect("parse succeeds");
    for e in exprs {
        runtime.dispatch_in_scope(e, scope);
    }
    runtime
        .execute()
        .expect("LET with eager-sub RHS runs cleanly on the stateful driver");
    assert!(
        matches!(scope.lookup("y"), Some(KObject::Number(n)) if *n == 1.0),
        "LET y = (FIRST [1 2 3]) must bind y to 1.0 via the stateful eager-subs track",
    );
}

/// A `Deferred` outcome at initial resolve installs the eager-subs track with
/// no captured function; the resume handler's re-resolve picks the overload
/// after the spliced sub supplies the discriminating type. Two overloads tie
/// on the bare-arg shape; the typed `Spliced(List<Number>)` lands the
/// `:(LIST OF Number)` arm.
#[test]
fn stateful_keyworded_deferred_resolves_after_eager_subs() {
    let region = run_root_storage();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    crate::builtins::test_support::run(
        scope,
        "FN (DESCRIBE xs :(LIST OF Number)) -> Str = (\"numbers\")",
    );
    crate::builtins::test_support::run(
        scope,
        "FN (DESCRIBE xs :(LIST OF Str)) -> Str = (\"strings\")",
    );
    let mut runtime = KoanRuntime::new();
    let exprs = crate::parse::parse("LET out = (DESCRIBE [1 2 3])").expect("parse succeeds");
    for e in exprs {
        runtime.dispatch_in_scope(e, scope);
    }
    runtime
        .execute()
        .expect("DESCRIBE with eager-sub list resolves cleanly on the stateful driver");
    match scope.lookup("out") {
        Some(KObject::KString(s)) => assert_eq!(s.as_str(), "numbers"),
        Some(other) => panic!("expected KString(\"numbers\"), got {}", other.summarize()),
        None => panic!("LET out = ... must bind `out` in scope"),
    }
}

// =====================================================================
// OperatorChain arm: classification + registry resolution. Recognition is
// parse-cached and structural; the arm resolves the cached operator probe through
// the per-scope registry and either misses (structured error) or reaches the fold
// seam.
// =====================================================================

/// `a + b + c` — slot-led, two `+` keyword positions. Classifies as `OperatorChain`,
/// not `Keyworded`.
#[test]
fn classifier_operator_chain_routes_to_operator_chain() {
    use crate::machine::execute::dispatch::{classify_dispatch_shape, DispatchShape};
    let expr = parse_one("a + b + c");
    assert_eq!(
        classify_dispatch_shape(&expr),
        DispatchShape::OperatorChain,
        "`a + b + c` must classify as OperatorChain",
    );
    assert_eq!(expr.operator_probe(), Some("+"));
}

/// `a + b` — a single operator is one keyword position, so ordinary binary
/// `Keyworded` dispatch, not a chain.
#[test]
fn classifier_single_operator_stays_keyworded() {
    use crate::machine::execute::dispatch::{classify_dispatch_shape, DispatchShape};
    let expr = parse_one("a + b");
    assert_eq!(
        classify_dispatch_shape(&expr),
        DispatchShape::Keyworded,
        "`a + b` is a single operator — Keyworded, not a chain",
    );
}

/// An undeclared operator chain misses the registry and surfaces a structured
/// `DispatchFailed` naming the undeclared operators. `%` is not a member of any builtin
/// group (`default_scope` seeds only comparison/additive/multiplicative), so the probe
/// misses cleanly before any operand is touched — the bare unbound identifiers never
/// need to resolve.
#[test]
fn operator_chain_undeclared_errors_cleanly() {
    let region = run_root_storage();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    let mut runtime = KoanRuntime::new();
    let id = runtime.dispatch_in_scope(parse_one("a % b % c"), scope);
    runtime
        .execute()
        .expect("scheduler drains without deadlock");
    let msg = match runtime.read_result_with(id, |v| v.summarize()) {
        Err(e) => e.to_string(),
        Ok(summary) => {
            panic!("an undeclared operator chain must terminate with an error; got {summary}")
        }
    };
    assert!(
        msg.contains("operator group") || msg.contains("declared together"),
        "expected an undeclared-operator-group error; got: {msg}",
    );
}

/// A `FoldRight` group registered in an inner scope over `-` — a symbol the run-global root
/// already seeds into the builtin additive `FoldLeft` group — reduces that scope's runs
/// right-associated, while the root's default still reduces runs written outside it. This is the
/// registry's innermost-wins walk end to end
/// ([`crate::machine::core::Scope::resolve_operator_group_with_chain`]): the builtin groups are
/// found last, so a declaring scope overrides them. The two associations are observably distinct:
/// `10 - 3 - 2` is `(10 - 3) - 2` = 5 folded left and `10 - (3 - 2)` = 9 folded right.
///
/// Both runs dispatch through the same builtin `-` body — the registry decides the *shape* of the
/// reduction, not which body runs.
#[test]
fn inner_scope_operator_group_overrides_the_builtin_fold_direction() {
    use crate::machine::model::{OperatorGroup, ReductionMode};
    use std::collections::HashSet;

    let region = run_root_storage();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    let inner = scope.brand().alloc_scope(scope.child_for_call());

    let members: HashSet<String> = ["-"].iter().map(|s| s.to_string()).collect();
    let group = scope
        .brand()
        .alloc_operator_group(OperatorGroup::new(members, ReductionMode::FoldRight));
    inner
        .register_operator_group("-".to_string(), group, BindingIndex::value(0))
        .expect("an inner scope may register a builtin operator's probe");

    // One runtime per scope: a run adopts one root scope in its run frame.
    let mut inner_runtime = KoanRuntime::new();
    let inner_id = inner_runtime.dispatch_in_scope(parse_one("10 - 3 - 2"), inner);
    inner_runtime
        .execute()
        .expect("scheduler drains without deadlock");
    let inner_result = inner_runtime
        .read_result_with(inner_id, |v| v.summarize())
        .unwrap_or_else(|e| panic!("a registered FoldRight group must evaluate; got error {e}"));
    assert_eq!(
        inner_result, "9",
        "inside the declaring scope, 10 - 3 - 2 folds right to 9 (10 - (3 - 2)); got {inner_result}"
    );

    let mut root_runtime = KoanRuntime::new();
    let root_id = root_runtime.dispatch_in_scope(parse_one("10 - 3 - 2"), scope);
    root_runtime
        .execute()
        .expect("scheduler drains without deadlock");
    let root_result = root_runtime
        .read_result_with(root_id, |v| v.summarize())
        .unwrap_or_else(|e| panic!("the builtin additive group must evaluate; got error {e}"));
    assert_eq!(
        root_result, "5",
        "outside it, the root's fold-left default stands: (10 - 3) - 2 = 5; got {root_result}"
    );
}

/// A fixture-registered `Unary` group hands its body the whole operand run as one list
/// operand. The infix chain `1 ~ 2 ~ 3 ~ 4` and the prefix call `~ [1 2 3 4]` both lower to the
/// same bare 2-part keyword-first dispatch `[Keyword("~"), ListLiteral(..)]` — exactly the shape
/// `HEAD [1 2 3]` dispatches through (see `fn_with_typed_list_param_accepts_matching_list` in
/// `src/builtins/fn_def/tests/container_types.rs`) — so both reach the same echoing `FN` body,
/// proving the "unary prefix and infix coincide" direction end-to-end.
#[test]
fn operator_chain_registered_unary_group_hands_body_the_list() {
    use crate::builtins::test_support::run;
    use crate::machine::model::{OperatorGroup, ReductionMode};
    use std::collections::HashSet;

    let region = run_root_storage();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    let members: HashSet<String> = ["~"].iter().map(|s| s.to_string()).collect();
    let group = scope
        .brand()
        .alloc_operator_group(OperatorGroup::new(members, ReductionMode::Unary));
    scope
        .register_operator_group("~".to_string(), group, BindingIndex::BUILTIN)
        .expect("register operator group");
    run(
        scope,
        "FN (~ xs :(LIST OF Number)) -> :(LIST OF Number) = (xs)",
    );

    let mut runtime = KoanRuntime::new();
    let infix_id = runtime.dispatch_in_scope(parse_one("1 ~ 2 ~ 3 ~ 4"), scope);
    runtime
        .execute()
        .expect("scheduler drains without deadlock");
    let infix = runtime
        .read_result_with(infix_id, |v| v.summarize())
        .unwrap_or_else(|e| panic!("a registered Unary group must evaluate; got error {e}"));
    assert_eq!(
        infix, "[1, 2, 3, 4]",
        "the infix chain must hand the body the whole run as one list"
    );

    let mut runtime = KoanRuntime::new();
    let prefix_id = runtime.dispatch_in_scope(parse_one("~ [1 2 3 4]"), scope);
    runtime
        .execute()
        .expect("scheduler drains without deadlock");
    let prefix = runtime
        .read_result_with(prefix_id, |v| v.summarize())
        .unwrap_or_else(|e| {
            panic!(
                "the prefix form must dispatch to the same body as the infix chain; got error {e}"
            )
        });
    assert_eq!(
        prefix, infix,
        "the prefix call and the infix chain must dispatch to the same body and agree"
    );
}

// =====================================================================
// HeadDeferred / TypeHeadDeferred / NonCallableHead routing + behavior.
// =====================================================================

/// `TypeCall` construct (regression). `Point {x = 1, y = 2}` — leaf-`Type` head
/// constructs a struct value directly off the resolved identity.
#[test]
fn type_call_constructs_struct() {
    use crate::builtins::test_support::{run, run_one, run_root_silent};
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(scope, "NEWTYPE Point = :{x :Number, y :Number}");
    let out = run_one(scope, parse_one("Point {x = 1, y = 2}"));
    assert_eq!(out.ktype().name(), "Point", "got {}", out.summarize());
}

/// `HeadDeferred` → function. A head that evaluates to a function value
/// (`(GET_F)` returning a `FN`) is applied with named args via the shared tail.
#[test]
fn head_deferred_calls_returned_function() {
    use crate::builtins::test_support::{run, run_one, run_root_silent};
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "FN (GET_F) -> :(FN (n :Number) -> Number) = \
         (FN (INNER n :Number) -> Number = (n))",
    );
    let out = run_one(scope, parse_one("(GET_F) {n = 7}"));
    assert!(
        matches!(out, KObject::Number(n) if (*n - 7.0).abs() < 1e-9),
        "(GET_F) {{n = 7}} must call the returned FN and yield 7.0; got {}",
        out.summarize(),
    );
}

/// `HeadDeferred` → a functor — a module-returning function — returns a module. A head that
/// evaluates to such a function value, applied with named args, yields a module — locking the
/// functor-application-as-function-call decision.
#[test]
fn head_deferred_applies_returned_functor_to_module() {
    use crate::builtins::test_support::{run, run_one, run_root_silent};
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "FN (GET_FUNCTOR) -> Any = \
         (FN (APPLYIT x :Number) -> Module = (MODULE inner = (LET inner = x)))",
    );
    let out = run_one(scope, parse_one("(GET_FUNCTOR) {x = 5}"));
    assert!(
        matches!(out, KObject::Module(_)),
        "applying a functor value must yield a module; got {}",
        out.summarize(),
    );
}

/// `HeadDeferred` → constructor. A head that evaluates to a `KTypeValue(UserType)`
/// (a nested head expression naming a type) routes through the `Constructor` arm.
#[test]
fn head_deferred_constructs_from_returned_type_value() {
    use crate::builtins::test_support::{run, run_one, run_root_silent};
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(scope, "NEWTYPE Point = :{x :Number, y :Number}");
    // `(Point) {x = 1, y = 2}`: the nested-`Expression` head `(Point)` resolves the
    // type leaf to `KTypeValue(Point)`, then the body constructs.
    let out = run_one(scope, parse_one("(Point) {x = 1, y = 2}"));
    assert_eq!(out.ktype().name(), "Point", "got {}", out.summarize());
}

/// `HeadDeferred` → non-callable error. A head that evaluates to a `Number`
/// surfaces a `DispatchFailed` (heads must be callable).
#[test]
fn head_deferred_non_callable_value_errors() {
    use crate::builtins::test_support::{run, run_one_err, run_root_silent};
    use crate::machine::KErrorKind;
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(scope, "FN (GET_NUM) -> Number = (42)");
    let err = run_one_err(scope, parse_one("(GET_NUM) {x = 1}"));
    match &err.kind {
        KErrorKind::DispatchFailed { reason, .. } => assert!(
            reason.contains("non-callable"),
            "expected a non-callable-head DispatchFailed, got {reason}",
        ),
        _ => panic!("expected DispatchFailed, got {err}"),
    }
}

/// `TypeHeadDeferred` → type error. A `:(...)` head whose value is not a
/// constructible type (here `Number`) surfaces a type-shaped
/// `TypeMismatch` — distinct from the `HeadDeferred` non-callable message.
#[test]
fn type_head_deferred_non_type_value_type_mismatches() {
    use crate::builtins::test_support::{run_one_err, run_root_silent};
    use crate::machine::KErrorKind;
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    let err = run_one_err(scope, parse_one(":(Number) {x = 1}"));
    match &err.kind {
        KErrorKind::TypeMismatch { expected, .. } => {
            assert_eq!(
                expected, "Type",
                "expected a type-shaped diagnostic, got {err}"
            )
        }
        _ => panic!("expected TypeMismatch, got {err}"),
    }
}

/// `TypeHeadDeferred` → constructor. A `:(Point)` head resolves to the struct
/// identity; the body constructs the struct value.
#[test]
fn type_head_deferred_constructs_from_sigil_type() {
    use crate::builtins::test_support::{run, run_one, run_root_silent};
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(scope, "NEWTYPE Point = :{x :Number, y :Number}");
    let out = run_one(scope, parse_one(":(Point) {x = 1, y = 2}"));
    assert_eq!(out.ktype().name(), "Point", "got {}", out.summarize());
}

/// `NonCallableHead`. A literal / list head in a multi-part expression is not
/// callable; the dispatch entry finalizes the slot with a `DispatchFailed`
/// (slot-terminal, TRY-catchable), read from the slot. The reason embeds the head
/// summary.
#[test]
fn non_callable_list_head_errors() {
    use crate::builtins::test_support::run_root_silent;
    use crate::machine::KErrorKind;
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    let mut runtime = KoanRuntime::new();
    let root = runtime.dispatch_in_scope(parse_one("[1 2 3] x"), scope);
    runtime
        .execute()
        .expect("a non-callable head is slot-terminal, not a fatal execute error");
    let err = runtime
        .result_error(root)
        .expect_err("a non-callable head must finalize the slot with an error");
    match &err.kind {
        KErrorKind::DispatchFailed { reason, .. } => assert!(
            reason.contains("head is not callable") && reason.contains("[1 2 3]"),
            "expected a non-callable-head DispatchFailed with the head summary, got {reason}",
        ),
        _ => panic!("expected DispatchFailed, got {err}"),
    }
}

/// Counter guard: the `TypeCall` and `HeadDeferred` evaluation branches resolve
/// synchronously / through the shared tail and never advance the
/// `resolve_dispatch` entry counter (mirrors the fast-lane routing claims).
#[test]
fn type_call_and_head_deferred_skip_resolve_dispatch() {
    use crate::builtins::test_support::{run, run_root_silent};
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(scope, "NEWTYPE Point = :{x :Number, y :Number}");

    reset_resolve_dispatch_entry_count();
    let _ = dispatch_one(scope, parse_one("Point {x = 1, y = 2}"));
    assert_eq!(
        resolve_dispatch_entry_count(),
        0,
        "TypeCall construct must not enter resolve_dispatch; counter was {}",
        resolve_dispatch_entry_count(),
    );

    reset_resolve_dispatch_entry_count();
    let _ = dispatch_one(scope, parse_one("(Point) {x = 1, y = 2}"));
    assert_eq!(
        resolve_dispatch_entry_count(),
        0,
        "HeadDeferred construct must not enter resolve_dispatch; counter was {}",
        resolve_dispatch_entry_count(),
    );
}
