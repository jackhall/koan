//! Bare type tokens (`Number`, `Str`, `Bool`, `Null`) as `:Type`-typed
//! FUNCTOR arguments. Pins the widening to
//! [`KType::Type::accepts_part`](crate::machine::model::types) — bare
//! `KTypeValue(_)` carriers other than `Module { .. }` / `Signature(_)`
//! admit, the per-call dual-write installs the carried `KType` into
//! `bindings.types`, and the deferred-return re-elaboration path is
//! agnostic to whether the bound carrier is a builtin or a nominal
//! type. See [`roadmap/type_language/bare-type-token-functor-arg.md`].

use crate::builtins::test_support::{parse_one, run, run_one, run_root_silent};
use crate::machine::execute::Scheduler;
use crate::machine::model::ast::KExpression;
use crate::machine::model::{KObject, KType};
use crate::machine::{KError, KErrorKind, RuntimeArena, Scope};

/// Either-path-tolerant error harness. Returns the `KError` whether it
/// surfaces from `Scheduler::execute()` (the resolve-step returns an error
/// directly when the bucket has no satisfying overload) OR from
/// `read_result` (the auto-wrap pass committed to an overload and `bind`
/// later refused the spliced carrier). Local to this file because the
/// negative bare-type-token tests are the only sites today that genuinely
/// need to tolerate both paths — the dispatch-engine refactor that
/// decides which path fires (early-reject at admission vs.
/// committed-then-failed bind) shouldn't break the wall test. Compare
/// `test_support::run_one_err`, which panics if `execute()` returns
/// `Err` (use that when the test KNOWS the error rides the
/// `read_result` path).
fn run_expecting_dispatch_error<'a>(scope: &'a Scope<'a>, expr: KExpression<'a>) -> KError {
    let mut sched = Scheduler::new();
    let id = sched.add_dispatch(expr, scope);
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

/// Positive admission: a bare builtin type token (`Number`) satisfies a
/// `:Type` parameter slot. Pre-widening this dispatched no-match because
/// `KType::Type::accepts_part` admitted only `TaggedUnionType` /
/// `StructType` carriers; the bare `KTypeValue(KType::Number)` carrier fell
/// through. Post-widening it matches the `KTypeValue(_)` arm and the
/// FUNCTOR body runs.
#[test]
fn functor_admits_bare_number_token_at_type_slot() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "FUNCTOR (MAKETREE Elt :Type) -> Module = (MODULE Result = (LET inner = 1))",
    );
    let result = run_one(scope, parse_one("MAKETREE Number"));
    match result {
        KObject::KTypeValue(KType::Module { .. }) => {}
        other => panic!(
            "expected MAKETREE Number to dispatch and return a module, got {:?}",
            other.ktype(),
        ),
    }
}

/// Same as the Number case, repeated for `Str`, `Bool`, and `Null`. Each
/// is a distinct `KType` variant with its own `KTypeValue` carrier shape;
/// they all flow through the same widened `KTypeValue(_)` admission arm.
#[test]
fn functor_admits_bare_str_bool_null_tokens_at_type_slot() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "FUNCTOR (MAKETREE Elt :Type) -> Module = (MODULE Result = (LET inner = 1))",
    );
    for token in ["Str", "Bool", "Null"] {
        let src = format!("MAKETREE {token}");
        let result = run_one(scope, parse_one(&src));
        match result {
            KObject::KTypeValue(KType::Module { .. }) => {}
            other => panic!(
                "expected MAKETREE {token} to dispatch and return a module, got {:?}",
                other.ktype(),
            ),
        }
    }
}

/// Per-call dual-write: with the bare-token admission flipped on, the
/// per-call `bindings.types` entry for `Elt` is the carried `KType` (e.g.
/// `KType::Number`). The body's `(LET ElemType = Elt)` resolves `Elt`
/// through the per-call type-side scope and registers `ElemType` as that
/// same `KType` on the returned module's `type_members`. Reading the
/// registered abstract-type identity back pins that the dual-write landed
/// and is observable through the lifted module.
#[test]
fn functor_per_call_type_side_bind_is_observable_via_module_type_members() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "FUNCTOR (MAKETREE Elt :Type) -> Module = \
         (MODULE Result = ((LET ElemType = Elt) (LET inner = 1)))",
    );
    let result = run_one(scope, parse_one("MAKETREE Number"));
    let module = match result {
        KObject::KTypeValue(KType::Module { module, .. }) => *module,
        other => panic!("expected module result, got {:?}", other.ktype()),
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

/// Negative — non-type carrier: passing a `Number(7)` literal to a `:Type`
/// slot is a dispatch no-match (not a bind-time `TypeMismatch` against a
/// committed pick). The `feedback_dispatch_trust_carried_type` contract
/// says non-satisfying typed args fall through the scope walk; with only
/// one `MAKETREE` overload and no tentative auto-wrap target for a bare
/// literal, the resolver surfaces `DispatchFailed` from `Scheduler::execute`
/// — distinct from the per-node `TypeMismatch` the module-carrier test
/// observes (where the auto-wrap pass commits to the pick and bind-time
/// re-resolves against the spliced `Future(_)` carrier).
#[test]
fn functor_bare_value_carrier_is_dispatch_no_match_not_typemismatch() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "FUNCTOR (MAKETREE Elt :Type) -> Module = (MODULE Result = (LET inner = 1))",
    );
    let err = run_expecting_dispatch_error(scope, parse_one("MAKETREE 7"));
    match &err.kind {
        KErrorKind::DispatchFailed { .. } | KErrorKind::UnboundName(_) => {}
        _ => panic!(
            "expected dispatch no-match (DispatchFailed) for non-type carrier, got {err}",
        ),
    }
}

/// Negative — module carrier: a `KTypeValue(KType::Module { .. })`
/// carrier stays out of `:Type` (the cut-(a) wall). Module values route
/// through `:Module` / `:SatisfiesSignature` / `:Signature` slots; mixing
/// them into `:Type` would collapse the `:Type` vs `:Module` dispatch
/// overload distinction the wall at [`KType::Type::accepts_part`]'s
/// `Future(KObject::KTypeValue(KType::Module { .. }))` arm protects.
///
/// The invariant the wall pins is binary: a module carrier in a `:Type`
/// slot MUST NOT successfully bind. Either surface form is acceptable —
/// `DispatchFailed` from `execute()` (the wall rejects at dispatch
/// admission, no overload survives) or a per-node error surfacing through
/// `read_result` (the auto-wrap pass commits to the lone overload and
/// `bind` later refuses the spliced carrier with a `TypeMismatch`). The
/// previous version of this test pinned the exact `TypeMismatch` shape
/// with `arg=Elt`, `expected=Type`, and a `got` substring; that coupled
/// the assertion to which dispatch path admission-then-bind takes today.
/// A dispatch-engine refactor that hoists the wall earlier (so the call
/// reports `DispatchFailed` at the resolve step instead of falling
/// through to a committed-then-failed bind), or that changes the bind-
/// time diagnostic phrasing without changing the wall semantics, would
/// flip the surface form without violating the invariant. Asserting only
/// that no successful value comes out keeps the test tied to the wall
/// itself.
#[test]
fn functor_module_carrier_does_not_fill_type_slot() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "FUNCTOR (MAKETREE Elt :Type) -> Module = (MODULE Result = (LET inner = 1))\n\
         MODULE IntMod = (LET inner = 1)",
    );
    // `run_expecting_dispatch_error` accepts either surface — see its docstring.
    // We don't assert on `err.kind` here; the wall's contract is "no bound value
    // makes it back to the caller", not "the diagnostic is exactly TypeMismatch".
    let _ = run_expecting_dispatch_error(scope, parse_one("MAKETREE IntMod"));
}

/// Deferred-return re-elaboration with a builtin-keyed bind. The FUNCTOR
/// `(BUILD Elt :Type) -> :Elt = ...` registers with
/// `ReturnType::Deferred(_)`; per-call elaboration resolves `Elt` to the
/// carried `KType` (here `KType::Number`) and the body's return value (a
/// Number) satisfies the per-call return slot. Pins that the unifier /
/// re-elaboration seam is agnostic to whether `Elt` was bound from a
/// builtin or a nominal carrier.
#[test]
fn functor_deferred_return_resolves_against_builtin_keyed_bind() {
    use crate::machine::model::ReturnType;
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    // `-> :Elt` is a TypeExprCarrier referencing the parameter `Elt`;
    // the FN-def routes it through `ReturnType::Deferred(TypeExpr(Elt))`.
    // The FUNCTOR return-slot admission verdict admits because `Elt :Type`
    // is type-denoting.
    run(
        scope,
        "FUNCTOR (BUILD Elt :Type) -> :Elt = (42)",
    );
    let f = crate::builtins::test_support::lookup_fn(scope, "BUILD");
    assert!(
        matches!(f.signature.return_type, ReturnType::Deferred(_)),
        "BUILD's return type should be Deferred, got {:?}",
        f.signature.return_type,
    );
    // Per-call elaboration: `Elt` → `KType::Number`; body returns `Number(42)`;
    // the per-call slot check (`Number.matches_value(Number(42))`) passes.
    let result = run_one(scope, parse_one("BUILD Number"));
    match result {
        KObject::Number(n) if *n == 42.0 => {}
        other => panic!("expected Number(42) from BUILD Number, got {:?}", other.ktype()),
    }
}

/// Companion to the deferred-return positive: a wrong-typed body surfaces
/// the per-call `TypeMismatch` diagnostic (same wording the
/// `functor_deferred_return_type_mismatch_surfaces_per_call_diagnostic`
/// pin uses on the nominal-keyed path). Pins that builtin-keyed binds
/// route through the same Combine-finish slot check.
#[test]
fn functor_deferred_return_builtin_keyed_mismatch_surfaces_per_call_diagnostic() {
    use crate::machine::execute::Scheduler;
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "FUNCTOR (BUILD Elt :Type) -> :Elt = (42)",
    );
    // `BUILD Str` binds `Elt = KType::Str`; body returns `Number(42)`,
    // which fails the per-call slot check.
    let mut sched = Scheduler::new();
    let id = sched.add_dispatch(parse_one("BUILD Str"), scope);
    sched.execute().expect("execute does not surface per-slot errors");
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
