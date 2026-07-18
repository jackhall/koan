//! Scope-aware type elaboration of FN signatures: signature-bound params, LET→FN ordering, type-value bindings.

use crate::builtins::test_support::{fn_is_registered, lookup_fn, run, run_root_silent};
use crate::machine::run_root_storage;

/// `LET MyList = :(LIST OF Number)` writes the elaborated `KType::list(Number)`
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
    assert_eq!(*kt, KType::list(Box::new(KType::Number)));
}

/// `elaborate_type_identifier` lowers a leaf naming a type-side LET binding back to its
/// stored `KType` (`LET MyList = :(LIST OF Number)` -> `KType::list(Number)`).
#[test]
fn elaborator_lowers_ktype_value_binding() {
    use crate::machine::model::KType;
    use crate::machine::model::TypeIdentifier;
    use crate::machine::model::{elaborate_type_identifier, Elaborator, TypeResolution};
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(scope, "LET MyList = :(LIST OF Number)");
    let mut el = Elaborator::new(scope);
    match elaborate_type_identifier(&mut el, &TypeIdentifier::leaf("MyList".into())) {
        TypeResolution::Done(kt) => assert_eq!(kt, KType::list(Box::new(KType::Number))),
        other => panic!("expected Done(:(List Number)), got {:?}", other),
    }
}

/// A parameter typed `er :Ordered` lowers via `elaborate_type_identifier` into
/// `KType::Signature { content, pinned_slots: [] }` with `content.sig_id` matching the
/// declaring SIG's decl-scope id. Also pins that the SIG and FN can land in the
/// same batch — the FN's signature elaboration parks on the SIG placeholder.
#[test]
fn fn_with_signature_bound_param_records_signature_bound_ktype() {
    use crate::machine::model::{Argument, KType, SignatureElement};
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "SIG Ordered = (VAL compare :Number)\n\
         FN (USE_ORD er :Ordered) -> Null = (PRINT \"ok\")",
    );
    // SIG installs a single type-side identity; read it from `bindings.types`.
    let sig_id = match scope.resolve_type("Ordered") {
        Some(KType::Signature { content, .. }) => content.sig_id,
        other => panic!("Ordered should be a Signature KType, got {:?}", other),
    };
    let f = lookup_fn(scope, "USE_ORD");
    match f.signature.elements.as_slice() {
        [SignatureElement::Keyword(kw), SignatureElement::Argument(Argument { name, ktype })] => {
            assert_eq!(kw, "USE_ORD");
            assert_eq!(name, "er");
            match ktype {
                KType::Signature {
                    content,
                    pinned_slots,
                    ..
                } => {
                    assert_eq!(
                        content.sig_id, sig_id,
                        "sig_id must match the declaring SIG's decl-scope id"
                    );
                    assert_eq!(content.path, "Ordered");
                    assert!(pinned_slots.is_empty(), "bare Ordered has no pinned slots");
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
    use crate::machine::KoanRuntime;
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
