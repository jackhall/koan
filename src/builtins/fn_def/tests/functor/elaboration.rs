//! Scope-aware type elaboration of FN signatures: signature-bound params, LET→FN ordering, type-value bindings.

use crate::builtins::test_support::{fn_is_registered, lookup_fn, run, run_root_silent};
use crate::machine::core::run_root_storage;

/// `LET MyList = :(LIST OF Number)` writes the elaborated `KType::List(Number)`
/// to `bindings.types` (reachable via `Scope::resolve_type`); the `KTypeValue`
/// carrier is only a dispatch transport, not the storage shape.
#[test]
fn list_of_let_binding_is_ktype_value() {
    use crate::machine::model::KType;
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(scope, "LET MyList = :(LIST OF Number)");
    let kt = scope
        .resolve_type("MyList")
        .expect("MyList should be bound in bindings.types");
    assert_eq!(*kt, KType::List(Box::new(KType::Number)));
}

/// `elaborate_type_identifier` lowers a leaf naming a type-side LET binding back to its
/// stored `KType` (`LET MyList = :(LIST OF Number)` -> `KType::List(Number)`).
#[test]
fn elaborator_lowers_ktype_value_binding() {
    use crate::machine::model::ast::TypeIdentifier;
    use crate::machine::model::types::{elaborate_type_identifier, Elaborator, TypeResolution};
    use crate::machine::model::KType;
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(scope, "LET MyList = :(LIST OF Number)");
    let mut el = Elaborator::new(scope);
    match elaborate_type_identifier(&mut el, &TypeIdentifier::leaf("MyList".into())) {
        TypeResolution::Done(kt) => assert_eq!(kt, KType::List(Box::new(KType::Number))),
        other => panic!("expected Done(:(List Number)), got {:?}", other),
    }
}

/// A parameter typed `Er :OrderedSig` lowers via `elaborate_type_identifier` into
/// `KType::Signature { sig, pinned_slots: [] }` with `sig.sig_id()` matching the
/// declaring `ModuleSignature::sig_id()`. Also pins that the SIG and FN can land in the
/// same batch — the FN's signature elaboration parks on the SIG placeholder.
#[test]
fn fn_with_signature_bound_param_records_signature_bound_ktype() {
    use crate::machine::model::{Argument, KType, SignatureElement};
    let region = run_root_storage();
    let scope = run_root_silent(&region);
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
                    assert_eq!(
                        sig.sig_id(),
                        sig_id,
                        "sig_id must match ModuleSignature::sig_id()"
                    );
                    assert_eq!(sig.path(), "OrderedSig");
                    assert!(
                        pinned_slots.is_empty(),
                        "bare OrderedSig has no pinned slots"
                    );
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
    use crate::machine::execute::KoanRuntime;
    use crate::parse::parse;
    let region = run_root_storage();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    let mut runtime = KoanRuntime::new();
    let exprs = parse(
        "LET MyList = :(LIST OF Number)\n\
         FN (USE xs :MyList) -> Number = (1)",
    )
    .unwrap();
    for e in exprs {
        runtime.dispatch_in_scope(e, scope);
    }
    runtime.execute().unwrap();
    assert!(
        scope.resolve_type("MyList").is_some(),
        "MyList should be bound in bindings.types after the batch executes",
    );
    assert!(
        fn_is_registered(scope, "USE"),
        "USE should be registered by the FN definition"
    );
}
