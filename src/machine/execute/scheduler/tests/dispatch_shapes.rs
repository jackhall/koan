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
use crate::machine::core::kfunction::{ArgumentBundle, BodyResult, SchedulerHandle as KfHandle};
use crate::machine::core::source::Spanned;
use crate::machine::execute::dispatch::{
    reset_resolve_dispatch_entry_count, resolve_dispatch_entry_count,
};
use crate::machine::execute::Scheduler;
use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::model::types::{
    Argument, ExpressionSignature, KType, ReturnType, SignatureElement,
};
use crate::machine::model::{KObject, Parseable};
use crate::machine::{BindingIndex, KFunction, RuntimeArena, Scope};

fn dispatch_one<'a>(scope: &'a Scope<'a>, expr: KExpression<'a>) -> &'a KObject<'a> {
    let mut sched = Scheduler::new();
    let id = sched.add_dispatch(expr, scope);
    sched.execute().expect("scheduler should succeed");
    sched.read(id)
}

/// Accepts one Number arg and returns it unchanged. The signature is `<n :Number>`
/// (no keywords), which means no koan user surface can call it directly — tests
/// using it only inspect routing, never the call outcome.
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

/// Bind a function value `f` with signature `<n :Number>` on `scope`, giving an
/// Identifier head that resolves to a function value without going through FN/LET.
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
/// fast-lane handler routes through `coerce_type_token_value`.
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
        "BareTypeLeaf must not enter resolve_dispatch",
    );
    assert!(
        matches!(result, KObject::KTypeValue(KType::Number)),
        "(Number) must terminate to KTypeValue(Number); got {}",
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
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
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
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
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
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "LET f = (FN (a :Number PICK b :Number) -> Number = (a))",
    );
    let expr = parse_one("f {a = 1}");
    reset_resolve_dispatch_entry_count();
    let mut sched = Scheduler::new();
    let id = sched.add_dispatch(expr, scope);
    sched
        .execute()
        .expect("scheduler should not surface errors directly");
    let err = match sched.read_result(id) {
        Err(e) => e.clone(),
        Ok(v) => panic!("expected MissingArg error, got value {}", v.summarize()),
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
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
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
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
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
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
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
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
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
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
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
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
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
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
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

/// Tagged-union construction via a LET-bound lowercase alias. The FnValueCall
/// fast lane resolves `maybe` to its `KTypeValue(UserType { Tagged { schema } })`
/// identity and constructs from the schema payload via
/// `constructors::dispatch_construct_tagged`.
///
/// Counter contract: every step in the chain (FnValueCall head resolution +
/// construct-from-identity + LiteralPassThrough on the value-cell) is fast-lane;
/// nothing enters `resolve_dispatch`.
#[test]
fn fast_lane_on_tagged_union_constructs() {
    use crate::builtins::test_support::{run, run_one, run_root_silent};
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "UNION Maybe = (some :Number none :Null)\nLET maybe = Maybe",
    );
    reset_resolve_dispatch_entry_count();
    let result = run_one(scope, parse_one("maybe (some 42)"));
    assert_eq!(
        resolve_dispatch_entry_count(),
        0,
        "tagged-union construction is fully fast-lane: no `resolve_dispatch` \
         entries. Counter was {}",
        resolve_dispatch_entry_count(),
    );
    match result {
        KObject::Tagged { tag, value, .. } => {
            assert_eq!(tag, "some");
            assert!(matches!(&**value, KObject::Number(n) if *n == 42.0));
        }
        other => panic!("expected Tagged, got {:?}", other.ktype()),
    }
}

/// Struct construction via a LET-bound lowercase alias. `(STRUCT Pt = ...)`
/// returns the `KTypeValue(UserType { Struct { fields } })` identity which LET binds
/// under `pt`; the fast lane constructs from the fields payload via
/// `constructors::dispatch_construct_struct`.
///
/// Counter contract: every step is fast-lane (FnValueCall head resolution +
/// construct-from-identity + LiteralPassThrough per value-cell); no entry into
/// `resolve_dispatch`.
#[test]
fn fast_lane_on_struct_type_constructs() {
    use crate::builtins::test_support::{run, run_one, run_root_silent};
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "LET pt = (STRUCT Pt = (x :Number, y :Number))");
    reset_resolve_dispatch_entry_count();
    let result = run_one(scope, parse_one("pt {x = 3, y = 4}"));
    assert_eq!(
        resolve_dispatch_entry_count(),
        0,
        "struct construction is fully fast-lane: no `resolve_dispatch` \
         entries. Counter was {}",
        resolve_dispatch_entry_count(),
    );
    match result {
        KObject::Struct {
            name: type_name,
            fields,
            ..
        } => {
            assert_eq!(type_name, "Pt");
            assert!(matches!(fields.get("x"), Some(KObject::Number(n)) if *n == 3.0));
            assert!(matches!(fields.get("y"), Some(KObject::Number(n)) if *n == 4.0));
        }
        other => panic!("expected Struct, got {:?}", other.ktype()),
    }
}

/// Single-part literal-shaped expressions — `(99)`, `("x")`, `([1 2 3])`,
/// `({a = 1})`, `((inner))` — route through `LiteralPassThrough` instead of
/// bucket-dispatching `value_pass`. The counter must stay at 0 for `(99)`
/// because the fast lane surfaces the literal without consulting buckets.
#[test]
fn literal_pass_through_routes_via_fast_lane() {
    use crate::builtins::test_support::{run_one, run_root_silent};
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
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
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
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
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    reset_resolve_dispatch_entry_count();
    let err = run_one_err(scope, parse_one("undefined {foo = 7}"));
    assert_eq!(resolve_dispatch_entry_count(), 0);
    assert!(
        matches!(&err.kind, KErrorKind::UnboundName(name) if name == "undefined"),
        "expected UnboundName(\"undefined\"), got {err}",
    );
}

/// A closure returned out of its defining call remains invocable. The lifted
/// `KObject::KFunction` carries an `Rc<CallArena>` keeping the per-call arena
/// (where the inner function's storage and captured scope live) alive past
/// frame drop. The fast lane's `KObject::KFunction(f, _)` pattern matches
/// regardless of whether the second field is `Some(rc)` or `None`.
#[test]
fn fast_lane_closure_escapes_outer_call_and_remains_invocable() {
    use crate::builtins::test_support::{run, run_one, run_root_silent};
    use crate::machine::model::Parseable;
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
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
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "FN (MAKE) -> :(FN (x :Number) -> Number) = (FN (ECHO x :Number) -> Number = (x))\n\
         LET f = (MAKE)",
    );
    let result = run_one(scope, parse_one("f {x = 42}"));
    assert!(matches!(result, KObject::Number(n) if *n == 42.0));
}

/// `lift_kobject` must recurse through the `List` variant to attach the dying
/// frame's `Rc<CallArena>` to embedded `KFunction(_, None)` elements; otherwise
/// the inner function's `&KFunction` reference would dangle into the freed
/// per-call arena. Asserting the lifted closure's frame field is `Some` verifies
/// the recursion fired.
#[test]
fn fast_lane_list_of_closures_escapes_outer_call_with_rc_attached() {
    use crate::builtins::test_support::{run, run_one, run_root_silent};
    use crate::machine::model::Parseable;
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
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
    match &items[0] {
        KObject::KFunction(_, frame) => assert!(
            frame.is_some(),
            "list-borne escaping closure must have an :(Rc CallArena) attached by \
             lift_kobject's recursion through the List variant",
        ),
        other => panic!(
            "list element should be a KFunction, got {}",
            other.summarize()
        ),
    }
}

/// `f {x = 7}` submitted as a forward reference: `f` is installed as a
/// `Placeholder` on `scope` before the slot is dispatched. The fast lane's
/// `FunctionValueCall` handler hits the `Placeholder` arm on head-resolution
/// (before the args-shape check), installs a combined park, and never enters
/// `resolve_dispatch`.
#[test]
fn function_value_call_forward_ref_parks() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let mut sched = Scheduler::new();

    // Bind `producer_target` to a value so the producer's own dispatch fast-lanes
    // via the BareIdentifier `Resolution::Value` hit — otherwise the unbound
    // fall-through would advance the counter and defeat the routing assertion.
    let producer_target = scope.arena.alloc(KObject::Number(42.0));
    scope
        .bind_value(
            "producer_target".to_string(),
            producer_target,
            BindingIndex::BUILTIN,
        )
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

    let f_call = parse_one("f {x = 7}");
    let _f_call_id = sched.add_dispatch(f_call, scope);

    reset_resolve_dispatch_entry_count();
    let _ = sched.execute();
    assert_eq!(
        resolve_dispatch_entry_count(),
        0,
        "FunctionValueCall forward-ref park must not enter resolve_dispatch; \
         the head-Placeholder arm fires before any args-shape inspection",
    );
}

/// `(PRINT 5)` — keyword-headed call routes through the candidate path.
/// `resolve_dispatch` runs at least once to find the bucket.
#[test]
fn keyworded_unchanged() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
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
/// body. Classifier must route to `ConstructorCall`, not `Keyworded`.
#[test]
fn classifier_struct_construct_routes_to_type_constructor_call() {
    use crate::machine::execute::dispatch::{classify_dispatch_shape, DispatchShape};
    let expr = parse_one("MyStruct {x = 1, y = 2}");
    assert!(
        matches!(
            classify_dispatch_shape(&expr),
            DispatchShape::ConstructorCall
        ),
        "expected ConstructorCall for `MyStruct {{x = 1, y = 2}}`",
    );
}

/// `(Maybe (some 42))` — leaf-Type head, single nested-`Expression` body
/// holding `(some 42)`. Must route to `ConstructorCall`.
#[test]
fn classifier_tagged_construct_routes_to_type_constructor_call() {
    use crate::machine::execute::dispatch::{classify_dispatch_shape, DispatchShape};
    let expr = parse_one("Maybe (some 42)");
    assert!(
        matches!(
            classify_dispatch_shape(&expr),
            DispatchShape::ConstructorCall
        ),
        "expected ConstructorCall for `Maybe (some 42)`",
    );
}

/// `(Bar (x))` — leaf-Type head, nested-`Expression` body wrapping a single
/// identifier (the newtype-construction shape). Routes to `ConstructorCall`.
#[test]
fn classifier_newtype_construct_routes_to_type_constructor_call() {
    use crate::machine::execute::dispatch::{classify_dispatch_shape, DispatchShape};
    let expr = parse_one("Bar (x)");
    assert!(
        matches!(
            classify_dispatch_shape(&expr),
            DispatchShape::ConstructorCall
        ),
        "expected ConstructorCall for `Bar (x)`",
    );
}

/// `(List Number)` — leaf-Type head, every arg a leaf Type. Every leaf-Type-
/// headed multi-part call routes through `ConstructorCall`. The keyworded
/// `LIST OF` overload is the supported way to elaborate `List<Number>`.
#[test]
fn classifier_legacy_positional_collapses_to_type_constructor_call() {
    use crate::machine::execute::dispatch::{classify_dispatch_shape, DispatchShape};
    let expr = parse_one("(List Number)");
    assert!(
        matches!(
            classify_dispatch_shape(&expr),
            DispatchShape::ConstructorCall
        ),
        "leaf-Type head + leaf-Type args must classify as ConstructorCall",
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
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    bind_identity_fn(scope);

    let expr_a = parse_one("(List MAYBE Number)");
    reset_resolve_dispatch_entry_count();
    let mut sched = Scheduler::new();
    sched.add_dispatch(expr_a, scope);
    let _ = sched.execute();
    assert!(
        resolve_dispatch_entry_count() >= 1,
        "(List MAYBE Number) must route to Keyworded (keyword in body); count was {}",
        resolve_dispatch_entry_count(),
    );

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

/// A Keyworded dispatch whose initial resolve picks an overload but whose
/// value-cell parts need sub-Dispatch evaluation (the Resolved-with-eager-subs
/// arm) must terminate correctly under the stateful driver. Pins that
/// `KeywordedState::WaitingEagerSubs` resumes, re-resolves, and binds inline
/// through `invoke_to_step_pinned`.
///
/// Program: `LET y = (FIRST [1 2 3])`. LET picks at initial resolve; the RHS
/// is an eager sub-Dispatch. After the sub resolves to `1`, the resume handler
/// splices `Future(1)` into the LET expression and re-resolves.
#[test]
fn stateful_keyworded_eager_subs_resumes_through_state() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    crate::builtins::test_support::run(scope, "FN (FIRST xs :(LIST OF Number)) -> Number = (1)");
    let mut sched = Scheduler::new();
    let exprs = crate::parse::parse("LET y = (FIRST [1 2 3])").expect("parse succeeds");
    for e in exprs {
        sched.add_dispatch(e, scope);
    }
    sched
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
/// on the bare-arg shape; the typed `Future(List<Number>)` lands the
/// `:(LIST OF Number)` arm.
#[test]
fn stateful_keyworded_deferred_resolves_after_eager_subs() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    crate::builtins::test_support::run(
        scope,
        "FN (DESCRIBE xs :(LIST OF Number)) -> Str = (\"numbers\")",
    );
    crate::builtins::test_support::run(
        scope,
        "FN (DESCRIBE xs :(LIST OF Str)) -> Str = (\"strings\")",
    );
    let mut sched = Scheduler::new();
    let exprs = crate::parse::parse("LET out = (DESCRIBE [1 2 3])").expect("parse succeeds");
    for e in exprs {
        sched.add_dispatch(e, scope);
    }
    sched
        .execute()
        .expect("DESCRIBE with eager-sub list resolves cleanly on the stateful driver");
    match scope.lookup("out") {
        Some(KObject::KString(s)) => assert_eq!(s.as_str(), "numbers"),
        Some(other) => panic!("expected KString(\"numbers\"), got {}", other.summarize()),
        None => panic!("LET out = ... must bind `out` in scope"),
    }
}

/// For each Keyworded track variant: the drain-end cycle-detection guard in
/// [`Scheduler::execute`] must read the parked slot's carrier expression from
/// `KeywordedState`, not from the placeholder `NodeWork::Dispatch.expr` that
/// `install_eager_subs_track` / `install_bare_name_park` / `install_overload_park`
/// drop to `KExpression::new(Vec::new())`. With only the empty `NodeWork`
/// expression available the deadlock sample would render as `""`;
/// `DispatchState::parked_carrier_expr` must surface the state-carried form.
#[test]
fn keyworded_parked_carrier_expr_reads_state() {
    use crate::machine::execute::dispatch::keyworded::{
        BareNameParkTrack, KeywordedState, OverloadParkTrack,
    };
    use crate::machine::execute::dispatch::{DispatchState, EagerSubsTrack, Initialized};

    fn carrier_expr<'a>() -> KExpression<'a> {
        // `(LIFT_BARE arg)` — a recognizable sample distinct from any other
        // test's expressions, so a regression that drops the carrier shows up
        // as a `""` summary, not a coincidentally-matching sibling expression.
        KExpression::new(vec![
            Spanned::bare(ExpressionPart::Keyword("LIFT_BARE".into())),
            Spanned::bare(ExpressionPart::Identifier("arg".into())),
        ])
    }
    let expected = carrier_expr().summarize();

    let with_eager_subs = DispatchState::Keyworded(Box::new(KeywordedState::with_eager_subs(
        Initialized {
            pre_subs: Vec::new(),
        },
        EagerSubsTrack::keyworded(carrier_expr(), Vec::new()),
    )));
    assert_eq!(
        with_eager_subs
            .parked_carrier_expr()
            .map(Parseable::summarize),
        Some(expected.clone()),
        "eager-subs track must surface `working_expr` as the parked sample",
    );

    let with_bare_name = DispatchState::Keyworded(Box::new(KeywordedState::with_bare_name_park(
        Initialized {
            pre_subs: Vec::new(),
        },
        BareNameParkTrack::new(carrier_expr(), Vec::new()),
    )));
    assert_eq!(
        with_bare_name
            .parked_carrier_expr()
            .map(Parseable::summarize),
        Some(expected.clone()),
        "bare-name-park track must surface `working_expr` as the parked sample",
    );

    let with_overload = DispatchState::Keyworded(Box::new(KeywordedState::with_overload_park(
        Initialized {
            pre_subs: Vec::new(),
        },
        OverloadParkTrack::new(carrier_expr(), Vec::new()),
    )));
    assert_eq!(
        with_overload
            .parked_carrier_expr()
            .map(Parseable::summarize),
        Some(expected),
        "overload-park track must surface its original `expr` as the parked sample",
    );

    // Non-Keyworded variants — and the one-shot Keyworded path that
    // terminalizes without installing a track — never park, so the
    // accessor surfaces `None` and the drain-end guard falls back to
    // the slot's `NodeWork::Dispatch.expr` field.
    let untracked = DispatchState::Keyworded(Box::new(KeywordedState::from_init(Initialized {
        pre_subs: Vec::new(),
    })));
    assert!(
        untracked.parked_carrier_expr().is_none(),
        "Keyworded with no installed track must surface None (fall back to NodeWork expr)",
    );
    assert!(
        DispatchState::initialized(Vec::new())
            .parked_carrier_expr()
            .is_none(),
        "Initialized must surface None (fall back to NodeWork expr)",
    );
}
