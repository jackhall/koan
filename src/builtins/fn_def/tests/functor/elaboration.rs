//! Scope-aware type elaboration of FN signatures: signature-bound params, LET→FN ordering, type-value bindings.

use crate::builtins::test_support::{fn_is_registered, lookup_fn, run, run_root_silent};
use crate::machine::RuntimeArena;

/// `LET MyList = (LIST_OF Number)` writes the elaborated `KType::List(Number)`
/// to `bindings.types` (reachable via `Scope::resolve_type`); the `KTypeValue`
/// carrier is only a dispatch transport, not the storage shape.
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

/// `elaborate_type_expr` lowers a leaf naming a type-side LET binding back to its
/// stored `KType` (`LET MyList = (LIST_OF Number)` -> `KType::List(Number)`).
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

/// A parameter typed `Er :OrderedSig` lowers via `elaborate_type_expr` into
/// `KType::Signature { sig, pinned_slots: [] }` with `sig.sig_id()` matching the
/// declaring `Signature::sig_id()`. Also pins that the SIG and FN can land in the
/// same batch — the FN's signature elaboration parks on the SIG placeholder.
#[test]
fn fn_with_signature_bound_param_records_signature_bound_ktype() {
    use crate::machine::model::{Argument, KType, SignatureElement};
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "SIG OrderedSig = (VAL compare :Number)\n\
         FN (USE_ORD Er :OrderedSig) -> Null = (PRINT \"ok\")",
    );
    // SIG installs a single type-side identity; read it from `bindings.types`.
    let sig_id = match scope.resolve_type("OrderedSig") {
        Some(KType::Signature { sig, .. }) => sig.sig_id(),
        other => panic!("OrderedSig should be a Signature KType, got {:?}", other),
    };
    let f = lookup_fn(scope, "USE_ORD");
    match f.signature.elements.as_slice() {
        [SignatureElement::Keyword(kw), SignatureElement::Argument(Argument { name, ktype })] => {
            assert_eq!(kw, "USE_ORD");
            assert_eq!(name, "Er");
            match ktype {
                KType::Signature { sig, pinned_slots } => {
                    assert_eq!(sig.sig_id(), sig_id, "sig_id must match Signature::sig_id()");
                    assert_eq!(sig.path, "OrderedSig");
                    assert!(pinned_slots.is_empty(), "bare OrderedSig has no pinned slots");
                }
                other => panic!("expected Signature, got {:?}", other),
            }
        }
        _ => panic!("expected [Keyword(USE_ORD), Argument(Er :Signature)]"),
    }
}

/// End-to-end park-on-LET-placeholder: a `LET` followed in the same batch by a
/// `FN` whose signature references it works because FN-def parks on the LET's
/// placeholder and re-runs elaboration against the finalized scope.
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
    assert!(
        scope.resolve_type("MyList").is_some(),
        "MyList should be bound in bindings.types after the batch executes",
    );
    assert!(fn_is_registered(scope, "USE"), "USE should be registered by the FN definition");
}
