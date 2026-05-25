//! Scope-aware type elaboration of FN signatures: signature-bound params, LET→FN ordering, type-value bindings.

use crate::builtins::test_support::{fn_is_registered, lookup_fn, run, run_root_silent};
use crate::machine::model::KObject;
use crate::machine::RuntimeArena;

/// Verify that `LET MyList = (LIST_OF Number)` registers a type binding carrying the
/// elaborated `KType::List(Number)`. Post-stage-1.7 storage flip the LET TypeExprRef
/// overload writes `bindings.types` (reachable via `Scope::resolve_type`); the prior
/// `KObject::KTypeValue` carrier survives only as a dispatch transport, not as the
/// storage shape.
#[test]
fn list_of_let_binding_is_ktype_value() {
    use crate::machine::model::KType;
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "LET MyList = (LIST_OF Number)");
    let kt = scope
        .resolve_type("MyList")
        .expect("MyList should be bound in bindings.types");
    assert_eq!(*kt, KType::List(Box::new(KType::Number)));
}

/// The scheduler-aware elaborator reads a `KTypeValue` binding back as its stored
/// `KType`: `LET MyList = (LIST_OF Number)` followed by an elaborator walk of the
/// `MyList` leaf returns `KType::List(Number)`. Replaces the previous `ScopeResolver`
/// path that was deleted in phase 5.
#[test]
fn elaborator_lowers_ktype_value_binding() {
    use crate::machine::model::ast::TypeExpr;
    use crate::machine::model::KType;
    use crate::machine::model::types::{elaborate_type_expr, ElabResult, Elaborator};
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "LET MyList = (LIST_OF Number)");
    let mut el = Elaborator::new(scope);
    match elaborate_type_expr(&mut el, &TypeExpr::leaf("MyList".into())) {
        ElabResult::Done(kt) => assert_eq!(kt, KType::List(Box::new(KType::Number))),
        other => panic!("expected Done(:(List Number)), got {:?}", other),
    }
}

/// FN-def integration: a parameter typed `Er: OrderedSig` lowers via the scope-aware
/// `elaborate_type_expr` into `KType::SatisfiesSignature { sig_id, sig_path: "OrderedSig" }`,
/// with `sig_id` equal to the declaring `Signature::sig_id()`. Pins the elaborator-to-
/// FN-signature path that drives functor dispatch.
///
/// Module-system functor-params Stage A: this test was migrated from a lowercase
/// `elem` parameter to Type-class `Er` (the documented surface form). The FN-def
/// parameter parser now admits Type-classified bare-leaf tokens as parameter names
/// in addition to lowercase Identifiers, which is what makes the documented
/// `FN (LIFT Er: OrderedSig) -> ...` surface form actually parse.
#[test]
fn fn_with_signature_bound_param_records_signature_bound_ktype() {
    use crate::machine::model::{Argument, KType, SignatureElement};
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    // Phase 3: single batch — the FN's signature elaboration parks on `OrderedSig`'s
    // SIG placeholder and the Combine wakes it once the SIG finalizes. Previously
    // required two batches because the synchronous type-name resolution didn't park.
    run(
        scope,
        "SIG OrderedSig = (VAL compare :Number)\n\
         FN (USE_ORD Er :OrderedSig) -> Null = (PRINT \"ok\")",
    );
    let data = scope.bindings().data();
    let sig_id = match data.get("OrderedSig") {
        Some(KObject::KSignature(s)) => s.sig_id(),
        other => panic!("OrderedSig should be a signature, got {:?}", other.map(|o| o.ktype())),
    };
    let f = lookup_fn(scope, "USE_ORD");
    match f.signature.elements.as_slice() {
        [SignatureElement::Keyword(kw), SignatureElement::Argument(Argument { name, ktype })] => {
            assert_eq!(kw, "USE_ORD");
            assert_eq!(name, "Er");
            match ktype {
                KType::SatisfiesSignature { sig_id: id, sig_path, pinned_slots } => {
                    assert_eq!(*id, sig_id, "sig_id must match Signature::sig_id()");
                    assert_eq!(sig_path, "OrderedSig");
                    assert!(pinned_slots.is_empty(), "bare OrderedSig has no pinned slots");
                }
                other => panic!("expected SatisfiesSignature, got {:?}", other),
            }
        }
        _ => panic!("expected [Keyword(USE_ORD), Argument(Er :SatisfiesSignature)]"),
    }
}

/// End-to-end park-on-LET-placeholder: a `LET MyList = (LIST_OF Number)` followed in the
/// same batch by a `FN (USE xs: MyList) -> ...` previously failed because FN-def's
/// signature elaboration ran synchronously and the LET hadn't finalized. Post-phase-3 the
/// FN body parks on the LET's placeholder via a Combine and re-runs the elaboration
/// against the now-final scope.
#[test]
fn let_then_fn_in_same_batch_works() {
    use crate::builtins::default_scope;
    use crate::machine::execute::Scheduler;
    use crate::parse::parse;
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let mut sched = Scheduler::new();
    let exprs = parse(
        "LET MyList = (LIST_OF Number)\n\
         FN (USE xs :MyList) -> Number = (1)",
    )
    .unwrap();
    for e in exprs {
        sched.add_dispatch(e, scope);
    }
    sched.execute().unwrap();
    // Post-stage-1.7 the LET TypeExprRef overload writes `MyList` into `bindings.types`,
    // not `data` — check the type-side map. The bare FN-def's `USE` binding lives in the
    // `functions` dispatch bucket (no `data` mirror).
    assert!(
        scope.resolve_type("MyList").is_some(),
        "MyList should be bound in bindings.types after the batch executes",
    );
    assert!(fn_is_registered(scope, "USE"), "USE should be registered by the FN definition");
}
