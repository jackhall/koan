//! Module-system stage 2: `ScopeResolver`, signature-bound parameters, functor lifting.

use crate::dispatch::builtins::test_support::{parse_one, run, run_one, run_root_silent};
use crate::dispatch::{KObject, RuntimeArena};

/// Verify that `LET MyList = (LIST_OF Number)` binds a `TypeExprValue` carrying the
/// surface `List` form. The bound value can then be lowered to `KType::List(Number)`
/// via `KType::from_type_expr` — the ScopeResolver path uses exactly that.
#[test]
fn list_of_let_binding_is_type_expr_value() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "LET MyList = (LIST_OF Number)");
    let data = scope.data.borrow();
    let entry = data.get("MyList").expect("MyList should be bound");
    match entry {
        KObject::TypeExprValue(t) => {
            assert_eq!(t.name, "List");
        }
        other => panic!("expected TypeExprValue, got ktype={}", other.ktype().name()),
    }
}

/// `ScopeResolver` lowers a `TypeExprValue` binding to a `KType` when consulted by
/// `from_type_expr`. Direct unit test of the resolver's contract — independent of the
/// scheduler's top-level-statement ordering (see caveat below).
///
/// **Caveat — top-level statement ordering.** Today `LET MyList = (LIST_OF Number)`
/// followed by `FN (USE xs: MyList) ...` doesn't work end-to-end because the LET's
/// `value` slot is an `Expression` (the `(LIST_OF Number)` sub-expression), so the LET
/// becomes a Bind waiting on a sub-Dispatch — and the next top-level statement (the
/// FN) runs before the Bind resolves. Sequencing this requires either ordering top-
/// level statements as deps of one another or hoisting type-expression evaluation
/// ahead of FN-body execution. Tracked as a stage-2 follow-up; the resolver itself
/// already does the right thing once the binding is present.
#[test]
fn scope_resolver_lowers_type_expr_value_binding() {
    use crate::dispatch::KType;
    use crate::dispatch::types::{ScopeResolver, TypeResolver};
    use crate::parse::kexpression::{TypeExpr, TypeParams};
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "LET MyList = (LIST_OF Number)");
    let resolver = ScopeResolver::new(scope);
    // MyList resolves through the scope.
    let resolved = resolver.resolve("MyList").expect("MyList should resolve");
    assert_eq!(resolved, KType::List(Box::new(KType::Number)));
    // from_type_expr forwards to the resolver before falling back to from_name; so
    // `Number` (a builtin) still resolves, and `MyList` resolves via scope.
    let mylist_te = TypeExpr { name: "MyList".into(), params: TypeParams::None };
    let kt = KType::from_type_expr(&mylist_te, &resolver).expect("from_type_expr ok");
    assert_eq!(kt, KType::List(Box::new(KType::Number)));
}

/// FN-def integration: a parameter typed `E: OrderedSig` lowers via `ScopeResolver`
/// into `KType::SignatureBound { sig_id, sig_path: "OrderedSig" }`, with `sig_id`
/// equal to the declaring `Signature::sig_id()`. Pins the resolver-to-FN-signature
/// path that drives functor dispatch.
#[test]
fn fn_with_signature_bound_param_records_signature_bound_ktype() {
    use crate::dispatch::{Argument, KType, SignatureElement};
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    // Two batches: SIG resolves to a `KSignature` binding (its Combine finalizes)
    // before the FN runs and synchronously consults `ScopeResolver`. Same shape as
    // `scope_resolver_lowers_type_expr_value_binding` above — the synchronous
    // type-name resolution doesn't park on placeholders today (caveat noted there).
    run(scope, "SIG OrderedSig = (LET compare = 0)");
    run(scope, "FN (USE_ORD elem: OrderedSig) -> Null = (PRINT \"ok\")");
    let data = scope.data.borrow();
    let sig_id = match data.get("OrderedSig") {
        Some(KObject::KSignature(s)) => s.sig_id(),
        other => panic!("OrderedSig should be a signature, got {:?}", other.map(|o| o.ktype())),
    };
    let entry = data.get("USE_ORD").expect("USE_ORD should be bound");
    let f = match entry {
        KObject::KFunction(f, _) => *f,
        _ => panic!("expected USE_ORD to bind a KFunction"),
    };
    match f.signature.elements.as_slice() {
        [SignatureElement::Keyword(kw), SignatureElement::Argument(Argument { name, ktype })] => {
            assert_eq!(kw, "USE_ORD");
            assert_eq!(name, "elem");
            match ktype {
                KType::SignatureBound { sig_id: id, sig_path } => {
                    assert_eq!(*id, sig_id, "sig_id must match Signature::sig_id()");
                    assert_eq!(sig_path, "OrderedSig");
                }
                other => panic!("expected SignatureBound, got {:?}", other),
            }
        }
        _ => panic!("expected [Keyword(USE_ORD), Argument(elem: SignatureBound)]"),
    }
}

/// Module-system stage 2 (functor slice). End-to-end shape mirror of
/// [`crate::dispatch::values::module::tests::functor_per_call_module_lifts_correctly`]:
/// run a complete functor invocation through the scheduler, hold the returned
/// `KModule` past several subsequent allocations and FN calls, and assert member
/// access on the lifted module still returns the correct value. Pins the closure-
/// escape + per-call-arena story for the functor result module under tree borrows.
#[test]
fn functor_body_module_dispatch_does_not_dangle() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "SIG OrderedSig = (LET compare = 0)\n\
         MODULE IntOrd = (LET compare = 7)",
    );
    run(scope, "LET int_ord_a = (IntOrd :! OrderedSig)");
    run(
        scope,
        "FN (MAKESET elem: OrderedSig) -> Module = (MODULE Result = (LET inner = 1))",
    );
    run(scope, "LET held_set = (MAKESET (int_ord_a))");

    // Subsequent allocations and FN calls churn the run-root arena. The lifted
    // KModule must keep its child-scope arena alive through those churns.
    run(scope, "FN (NOOP) -> Number = (1)");
    for _ in 0..20 {
        run_one(scope, parse_one("NOOP"));
    }
    // Another functor call to allocate more frames (and drop them).
    run(scope, "LET other_set = (MAKESET (int_ord_a))");

    // Now read held_set's `inner` member — child_scope_ptr must still be live.
    let data = scope.data.borrow();
    let m = match data.get("held_set") {
        Some(KObject::KModule(m, _)) => *m,
        other => panic!("held_set should be a module, got {:?}", other.map(|o| o.ktype())),
    };
    let inner = m.child_scope().data.borrow().get("inner").copied();
    assert!(matches!(inner, Some(KObject::Number(n)) if *n == 1.0),
            "held_set.inner must still read 1.0 after subsequent churn");
}
