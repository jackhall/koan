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
/// Calling `f (x = 7)` routes through the fast lane's *named-arg admission*: the
/// signature's lone Argument `x` is present with a Number value (keyword `DOUBLE`
/// elided), and the fast lane reconstructs the positional expression via
/// `KFunction::reconstruct_positional` and binds directly with `picked = Some(f)`.
/// **The counter must read 0** — under PR B + Phase 1 of the fast-lane subsumption
/// (`scratch/plan-fast-lane-subsume.md`) the entry-point head resolution short-
/// circuits and the construction primitive doesn't run (it's a function, not a
/// constructor).
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
/// `(a :Number PICK b :Number)` — `b` is missing. Phase 1 of the `call_by_name`
/// subsumption surfaces `MissingArg("b")` from inside the fast-lane handler
/// (`reconstruct_positional`'s structured error), so the counter stays at 0 — no
/// keyworded fall-through. Mirrors the migrated `call_by_name_missing_named_arg`
/// surface assertion below; the routing assertion (counter == 0) is the new piece.
#[test]
fn function_value_call_named_args_missing_short_circuits() {
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
// Migrated `call_by_name` tests (Phase 1 of unified-walk follow-up). Each test
// preserves the surface assertion from the deleted `src/builtins/call_by_name.rs`
// test module verbatim and adds `resolve_dispatch_entry_count == 0` to pin the
// fast-lane routing. See `scratch/plan-fast-lane-subsume.md` Phase 1 commit 3.
// =====================================================================

/// `f (x = 7)` against `(FN (DOUBLE x :Number) ...)` — function-value call via
/// named-arg admission, fast-lane bound directly. Migrated from
/// `call_by_name::tests::fn_callable_via_call_by_name`.
#[test]
fn fast_lane_fn_callable_via_named_args() {
    use crate::builtins::test_support::{run, run_one, run_root_silent};
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "LET f = (FN (DOUBLE x :Number) -> Number = (x))");
    reset_resolve_dispatch_entry_count();
    let result = run_one(scope, parse_one("f (x = 7)"));
    assert_eq!(
        resolve_dispatch_entry_count(),
        0,
        "named-arg call must fast-lane bypass resolve_dispatch_with_chain; \
         counter was {}",
        resolve_dispatch_entry_count(),
    );
    assert!(matches!(result, KObject::Number(n) if *n == 7.0));
}

/// Internal keyword in a non-leading signature slot must be re-woven between
/// reordered args at reconstruction time. Migrated from
/// `call_by_name_weaves_internal_keyword`.
#[test]
fn fast_lane_weaves_internal_keyword() {
    use crate::builtins::test_support::{run, run_one, run_root_silent};
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "LET f = (FN (a :Number PICK b :Number) -> Number = (a))");
    reset_resolve_dispatch_entry_count();
    let result = run_one(scope, parse_one("f (a = 1, b = 2)"));
    assert_eq!(resolve_dispatch_entry_count(), 0);
    assert!(matches!(result, KObject::Number(n) if *n == 1.0));
}

/// Named-arg lookup is by name, not position: `(b = 2, a = 1)` satisfies
/// `(a PICK b)` the same as `(a = 1, b = 2)`. Migrated from
/// `call_by_name_named_args_order_independent`.
#[test]
fn fast_lane_named_args_order_independent() {
    use crate::builtins::test_support::{run, run_one, run_root_silent};
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "LET f = (FN (a :Number PICK b :Number) -> Number = (a))");
    reset_resolve_dispatch_entry_count();
    let result = run_one(scope, parse_one("f (b = 2, a = 1)"));
    assert_eq!(resolve_dispatch_entry_count(), 0);
    assert!(matches!(result, KObject::Number(n) if *n == 1.0));
}

/// Unknown named-arg fires after missing-arg precedence is satisfied. `(a = 1,
/// b = 2, c = 3)` covers required names plus an extra `c`. Migrated from
/// `call_by_name_unknown_named_arg`; the surface predicate is identical (matches
/// "unknown name" + the offending name in backticks).
#[test]
fn fast_lane_unknown_named_arg() {
    use crate::builtins::test_support::{run, run_one_err, run_root_silent};
    use crate::machine::KErrorKind;
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "LET f = (FN (a :Number PICK b :Number) -> Number = (a))");
    reset_resolve_dispatch_entry_count();
    let err = run_one_err(scope, parse_one("f (a = 1, b = 2, c = 3)"));
    assert_eq!(resolve_dispatch_entry_count(), 0);
    assert!(
        matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("unknown name") && msg.contains("`c`")),
        "expected ShapeError on unknown name c, got {err}",
    );
}

/// Malformed pair shape (`f (a 1)` — missing `=` / `:`-separator) surfaces a
/// `ShapeError` from `NamedPairs::parse` inside `reconstruct_positional`. Same
/// substring tolerance as the deleted `call_by_name_missing_colon` test.
#[test]
fn fast_lane_missing_separator() {
    use crate::builtins::test_support::{run, run_one_err, run_root_silent};
    use crate::machine::KErrorKind;
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "LET f = (FN (DOUBLE x :Number) -> Number = (x))");
    reset_resolve_dispatch_entry_count();
    let err = run_one_err(scope, parse_one("f (a 1)"));
    assert_eq!(resolve_dispatch_entry_count(), 0);
    assert!(
        matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("`:`") || msg.contains("separator") || msg.contains("triples")),
        "expected ShapeError on missing colon, got {err}",
    );
}

/// Duplicate named-arg is caught by `NamedPairs::parse` at construction time.
/// Migrated from `call_by_name_duplicate_named_arg`.
#[test]
fn fast_lane_duplicate_named_arg() {
    use crate::builtins::test_support::{run, run_one_err, run_root_silent};
    use crate::machine::KErrorKind;
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "LET f = (FN (DOUBLE x :Number) -> Number = (x))");
    reset_resolve_dispatch_entry_count();
    let err = run_one_err(scope, parse_one("f (x = 1, x = 2)"));
    assert_eq!(resolve_dispatch_entry_count(), 0);
    assert!(
        matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("duplicate") && msg.contains("`x`")),
        "expected ShapeError on duplicate name, got {err}",
    );
}

/// Non-function head resolves to a value-side carrier the fast lane refuses with
/// `TypeMismatch { arg: "verb", expected: "KFunction or Type" }`. Migrated from
/// `call_by_name_on_non_function_returns_error`; verb-precedence (verb resolves
/// before pair parsing) holds because head resolution is the first match arm.
#[test]
fn fast_lane_on_non_function_returns_error() {
    use crate::builtins::test_support::{run, run_one_err, run_root_silent};
    use crate::machine::KErrorKind;
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "LET x = 42");
    reset_resolve_dispatch_entry_count();
    let err = run_one_err(scope, parse_one("x (foo = 7)"));
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

/// Tagged-union construction via a LET-bound lowercase alias. The fast lane
/// resolves `maybe` to `TaggedUnionType`, calls `tagged_union::apply`, and
/// schedules the synthesized tail as a sub-Dispatch through the
/// `tagged_union_construct` primitive. Migrated from
/// `call_by_name_on_tagged_union_constructs`.
///
/// **Counter contract.** The entry-point head resolution short-circuits in the
/// fast lane (no candidate walk for `maybe`). The synthesized tail
/// `[Future(schema), tag, value]` re-dispatches through `tagged_union_construct`,
/// which IS a keyworded dispatch and so advances the counter exactly once. We
/// assert `counter == 1`: 0 walks for the entry-point head, 1 walk for the
/// construction primitive. Compared to the pre-migration path (`call_by_name`
/// resolution + `tagged_union_construct` re-dispatch = 2 walks) the fast lane
/// saves the entry walk.
#[test]
fn fast_lane_on_tagged_union_constructs() {
    use crate::builtins::test_support::{run, run_one, run_root_silent};
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "UNION Maybe = (some :Number none :Null)\nLET maybe = Maybe");
    reset_resolve_dispatch_entry_count();
    let result = run_one(scope, parse_one("maybe (some 42)"));
    assert_eq!(
        resolve_dispatch_entry_count(),
        1,
        "tagged-union construction must fast-lane the entry; the trailing walk \
         is `tagged_union_construct`'s own dispatch. Counter was {}",
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
/// returns the `StructType` value which LET binds under `pt`; the fast lane
/// routes the call through `struct_value::apply`. Migrated from
/// `call_by_name_on_struct_type_constructs`.
///
/// **Counter contract.** `struct_value::apply` synthesizes a tail of shape
/// `[Future(schema), ListLiteral([(<v_1>), ...])]` re-dispatching through the
/// `struct_construct` primitive. The list-literal aggregate spawns one
/// sub-Dispatch per value-cell to route bare identifiers through `value_lookup`
/// and bare literals through `value_pass`. With two fields (`x`, `y`) bound
/// to bare-literal values the resulting walks are: 1 for `struct_construct`'s
/// own dispatch + 1 each for the two value-cell sub-Dispatches + 1 trailing
/// re-dispatch (the synthesized tail's outermost shape itself) = 4. The
/// entry-point head walk for `pt` is the one that's removed vs the
/// pre-migration `call_by_name` path.
#[test]
fn fast_lane_on_struct_type_constructs() {
    use crate::builtins::test_support::{run, run_one, run_root_silent};
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "LET pt = (STRUCT Pt = (x :Number, y :Number))");
    reset_resolve_dispatch_entry_count();
    let result = run_one(scope, parse_one("pt (x = 3, y = 4)"));
    assert_eq!(
        resolve_dispatch_entry_count(),
        4,
        "struct construction must fast-lane the entry; trailing walks are \
         the construction primitive + value-cell sub-Dispatches. Counter was {}",
        resolve_dispatch_entry_count(),
    );
    match result {
        KObject::Struct { name: type_name, fields, .. } => {
            assert_eq!(type_name, "Pt");
            assert!(matches!(fields.get("x"), Some(KObject::Number(n)) if *n == 3.0));
            assert!(matches!(fields.get("y"), Some(KObject::Number(n)) if *n == 4.0));
        }
        other => panic!("expected Struct, got {:?}", other.ktype()),
    }
}

/// Unbound head surfaces `UnboundName(name)` directly from the fast-lane handler
/// (D1.2: no fall-through). Migrated from `call_by_name_unbound_returns_error`.
#[test]
fn fast_lane_unbound_returns_error() {
    use crate::builtins::test_support::{run_one_err, run_root_silent};
    use crate::machine::KErrorKind;
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    reset_resolve_dispatch_entry_count();
    let err = run_one_err(scope, parse_one("undefined (foo = 7)"));
    assert_eq!(resolve_dispatch_entry_count(), 0);
    assert!(
        matches!(&err.kind, KErrorKind::UnboundName(name) if name == "undefined"),
        "expected UnboundName(\"undefined\"), got {err}",
    );
}

/// Closure-lifetime test #1 (migrated verbatim from
/// `call_by_name::tests::closure_escapes_outer_call_and_remains_invocable`):
/// a closure returned out of its defining call remains invocable. The lifted
/// `KObject::KFunction` carries an `Rc<CallArena>` keeping the per-call arena
/// (where the inner function's storage and captured scope live) alive past
/// frame drop. The fast lane's `KObject::KFunction(f, _)` pattern matches
/// regardless of whether the second field is `Some(rc)` or `None`, so the
/// escaped-closure path goes through unchanged.
#[test]
fn fast_lane_closure_escapes_outer_call_and_remains_invocable() {
    use crate::builtins::test_support::{run, run_one, run_root_silent};
    use crate::machine::model::Parseable;
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "FN (MAKE) -> :(Function () -> Str) = (FN (INNER) -> Str = (\"hi\"))\n\
         LET f = (MAKE)",
    );
    let result = run_one(scope, parse_one("f ()"));
    assert!(
        matches!(result, KObject::KString(s) if s == "hi"),
        "expected KString(\"hi\"), got {}",
        result.summarize(),
    );
}

/// Closure-lifetime test #2 (migrated verbatim from
/// `escaped_closure_with_param_returns_body_value`): variant exercising
/// parameter-binding via the captured scope after escape.
#[test]
fn fast_lane_escaped_closure_with_param_returns_body_value() {
    use crate::builtins::test_support::{run, run_one, run_root_silent};
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "FN (MAKE) -> :(Function (Number) -> Number) = (FN (ECHO x :Number) -> Number = (x))\n\
         LET f = (MAKE)",
    );
    let result = run_one(scope, parse_one("f (x = 42)"));
    assert!(matches!(result, KObject::Number(n) if *n == 42.0));
}

/// Closure-lifetime test #3 (migrated verbatim from
/// `list_of_closures_escapes_outer_call_with_rc_attached`): `lift_kobject` must
/// recurse through the `List` variant to attach the dying frame's `Rc<CallArena>`
/// to embedded `KFunction(_, None)` elements; otherwise the inner function's
/// `&KFunction` reference would dangle into the freed per-call arena. Asserting
/// the lifted closure's frame field is `Some` verifies the recursion fired.
#[test]
fn fast_lane_list_of_closures_escapes_outer_call_with_rc_attached() {
    use crate::builtins::test_support::{run, run_one, run_root_silent};
    use crate::machine::model::Parseable;
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "FN (MAKE) -> List = ([(FN (ECHO x :Number) -> Number = (x))])");
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
        other => panic!("list element should be a KFunction, got {}", other.summarize()),
    }
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
