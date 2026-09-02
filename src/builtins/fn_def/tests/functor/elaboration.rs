//! Scope-aware type elaboration of FN signatures: signature-bound params, LET→FN ordering, type-value bindings.

use crate::builtins::test_support::lookup_type;
use crate::builtins::test_support::type_token;
use crate::builtins::test_support::{TestRun, fn_is_registered, lookup_fn, probe_symbol};
use crate::machine::{program_storage, run_root_storage};

/// `LET MyList = :(LIST OF Number)` writes the elaborated `KType::list(Number)`
/// to `bindings.types` (reachable via `Scope::resolve_type`); the `Held::Type`
/// carrier is only a dispatch transport, not the storage shape.
#[test]
fn list_of_let_binding_is_ktype_value() {
    use crate::machine::model::KType;
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run("LET MyList = :(LIST OF Number)");
    let kt = lookup_type(scope, "MyList").expect("MyList should be bound in bindings.types");
    assert_eq!(kt, test_run.types().list(KType::NUMBER));
}

/// `elaborate_type_identifier` lowers a leaf naming a type-side LET binding back to its
/// stored `KType` (`LET MyList = :(LIST OF Number)` -> `KType::list(Number)`).
#[test]
fn elaborator_lowers_ktype_value_binding() {
    use crate::machine::model::KType;
    use crate::machine::model::{Elaborator, TypeResolution, elaborate_type_identifier};
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run("LET MyList = :(LIST OF Number)");
    let types = test_run.registry_handle();
    let mut el = Elaborator::new(scope);
    match elaborate_type_identifier(&mut el, type_token("MyList"), types.registries()) {
        TypeResolution::Done(kt) => assert_eq!(kt, types.list(KType::NUMBER)),
        other => panic!("expected Done(:(List Number)), got {:?}", other),
    }
}

/// A parameter typed `er :Ordered` lowers via `elaborate_type_identifier` into
/// `KType::Signature { schema, .. }` with `schema.sig_id` matching the
/// declaring SIG's decl-scope id. Also pins that the SIG and FN can land in the
/// same batch — the FN's signature elaboration parks on the SIG placeholder.
#[test]
fn fn_with_signature_bound_param_records_signature_bound_ktype() {
    use crate::machine::model::{Argument, SignatureElement, TypeNode};
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run(
        "SIG Ordered = (VAL compare :Number)\n\
         FN (USE_ORD er :Ordered) -> Null = (PRINT \"ok\")",
    );
    // SIG installs a single type-side identity; read it from `bindings.types`.
    let ordered = lookup_type(scope, "Ordered");
    let sig_id = match ordered.map(|h| test_run.types().node(h)) {
        Some(TypeNode::Signature { schema, .. }) => schema.sig_id,
        _ => panic!("Ordered should be a Signature KType, got {ordered:?}"),
    };
    let f = lookup_fn(scope, "USE_ORD");
    match f.signature.elements() {
        [
            SignatureElement::Keyword(kw),
            SignatureElement::Argument(Argument { name, ktype, .. }),
        ] => {
            assert_eq!(*kw, probe_symbol("USE_ORD"));
            assert_eq!(name.symbol(), crate::machine::model::Symbol::of("er"));
            match test_run.types().node(*ktype) {
                TypeNode::Signature { schema, .. } => {
                    assert_eq!(
                        schema.sig_id, sig_id,
                        "sig_id must match the declaring SIG's decl-scope id"
                    );
                    // The node carries no declaration label (ruling 12); a non-empty interface
                    // renders structurally in member-name order.
                    assert_eq!(ktype.name(test_run.registries()), "SIG (compare: Number)");
                }
                _ => panic!("expected Signature, got {ktype:?}"),
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
    use crate::parse::parse;
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let exprs = parse(
        test_run.program_brand(),
        &test_run.registries().labels,
        "LET MyList = :(LIST OF Number)\n\
         FN (USE xs :MyList) -> Number = (1)",
    )
    .unwrap();
    for e in exprs {
        test_run.dispatch_in_scope(
            crate::machine::model::WorkingExpression::from_ast(scope.brand(), e),
            scope,
        );
    }
    test_run.runtime.execute().unwrap();
    assert!(
        lookup_type(scope, "MyList").is_some(),
        "MyList should be bound in bindings.types after the batch executes",
    );
    assert!(
        fn_is_registered(scope, "USE"),
        "USE should be registered by the FN definition"
    );
}
