//! Module-system stage 2: `ScopeResolver`, signature-bound parameters, functor lifting.

use crate::runtime::builtins::test_support::{parse_one, run, run_one, run_root_silent};
use crate::runtime::model::KObject;
use crate::runtime::machine::RuntimeArena;

/// Verify that `LET MyList = (LIST_OF Number)` binds a `KTypeValue` carrying the
/// elaborated `KType::List(Number)` directly. Post-`KTypeValue` migration the surface
/// form is gone from the runtime; consumers operate on the structural `KType`.
#[test]
fn list_of_let_binding_is_ktype_value() {
    use crate::runtime::model::KType;
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "LET MyList = (LIST_OF Number)");
    let data = scope.bindings().data();
    let entry = data.get("MyList").expect("MyList should be bound");
    match entry {
        KObject::KTypeValue(kt) => {
            assert_eq!(*kt, KType::List(Box::new(KType::Number)));
        }
        other => panic!("expected KTypeValue, got ktype={}", other.ktype().name()),
    }
}

/// `ScopeResolver` reads a `KTypeValue` binding back as its stored `KType`.
///
/// **Caveat — top-level statement ordering.** Today `LET MyList = (LIST_OF Number)`
/// followed by `FN (USE xs: MyList) ...` doesn't work end-to-end because the LET's
/// `value` slot is an `Expression` (the `(LIST_OF Number)` sub-expression), so the LET
/// becomes a Bind waiting on a sub-Dispatch — and the next top-level statement (the
/// FN) runs before the Bind resolves. Phase 3 of eager-type-elaboration closes this by
/// parking the FN signature elaboration on the LET's placeholder. The resolver itself
/// already does the right thing once the binding is present.
#[test]
fn scope_resolver_lowers_ktype_value_binding() {
    use crate::runtime::model::KType;
    use crate::runtime::model::types::{ScopeResolver, TypeResolver};
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "LET MyList = (LIST_OF Number)");
    let resolver = ScopeResolver::new(scope);
    let resolved = resolver.resolve("MyList").expect("MyList should resolve");
    assert_eq!(resolved, KType::List(Box::new(KType::Number)));
}

/// FN-def integration: a parameter typed `E: OrderedSig` lowers via `ScopeResolver`
/// into `KType::SignatureBound { sig_id, sig_path: "OrderedSig" }`, with `sig_id`
/// equal to the declaring `Signature::sig_id()`. Pins the resolver-to-FN-signature
/// path that drives functor dispatch.
#[test]
fn fn_with_signature_bound_param_records_signature_bound_ktype() {
    use crate::runtime::model::{Argument, KType, SignatureElement};
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    // Phase 3: single batch — the FN's signature elaboration parks on `OrderedSig`'s
    // SIG placeholder and the Combine wakes it once the SIG finalizes. Previously
    // required two batches because the synchronous type-name resolution didn't park.
    run(
        scope,
        "SIG OrderedSig = (LET compare = 0)\n\
         FN (USE_ORD elem: OrderedSig) -> Null = (PRINT \"ok\")",
    );
    let data = scope.bindings().data();
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

/// End-to-end park-on-LET-placeholder: a `LET MyList = (LIST_OF Number)` followed in the
/// same batch by a `FN (USE xs: MyList) -> ...` previously failed because FN-def's
/// signature elaboration ran synchronously and the LET hadn't finalized. Post-phase-3 the
/// FN body parks on the LET's placeholder via a Combine and re-runs the elaboration
/// against the now-final scope.
#[test]
fn let_then_fn_in_same_batch_works() {
    use crate::runtime::builtins::default_scope;
    use crate::runtime::machine::execute::Scheduler;
    use crate::parse::parse;
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let mut sched = Scheduler::new();
    let exprs = parse(
        "LET MyList = (LIST_OF Number)\n\
         FN (USE xs: MyList) -> Number = (1)",
    )
    .unwrap();
    for e in exprs {
        sched.add_dispatch(e, scope);
    }
    sched.execute().unwrap();
    let data = scope.bindings().data();
    assert!(
        data.get("MyList").is_some(),
        "MyList should be bound after the batch executes",
    );
    let use_fn = data.get("USE").expect("USE should be bound by the FN definition");
    assert!(matches!(use_fn, KObject::KFunction(_, _)));
}

/// Module-system stage 2 (functor slice). End-to-end shape mirror of
/// [`crate::runtime::model::values::module::tests::functor_per_call_module_lifts_correctly`]:
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
    let data = scope.bindings().data();
    let m = match data.get("held_set") {
        Some(KObject::KModule(m, _)) => *m,
        other => panic!("held_set should be a module, got {:?}", other.map(|o| o.ktype())),
    };
    let inner = m.child_scope().bindings().data().get("inner").copied();
    assert!(matches!(inner, Some(KObject::Number(n)) if *n == 1.0),
            "held_set.inner must still read 1.0 after subsequent churn");
}
