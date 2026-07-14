//! `TYPE OF <value>` — the value's own reported type, surfaced as a type value. General over the
//! value channel (scalar, container, module, view), and the door a module takes to type position.

use crate::builtins::test_support::{
    lookup_module, parse_one, run, run_one, run_one_err, run_one_type, run_root_silent,
};
use crate::machine::core::run_root_storage;
use crate::machine::model::types::SigSource;
use crate::machine::model::{KObject, KType, Parseable};
use crate::machine::KErrorKind;

#[test]
fn type_of_number_literal_is_number() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    let result = run_one_type(scope, parse_one("TYPE OF 5"));
    assert_eq!(*result, KType::Number);
}

/// A bound container reports its memoized carried element type, not a walk of its contents.
#[test]
fn type_of_bound_list_is_list_of_element_type() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(scope, "LET xs = [1, 2, 3]");
    let result = run_one_type(scope, parse_one("TYPE OF xs"));
    assert_eq!(*result, KType::list(Box::new(KType::Number)));
}

/// A module's reported type is its principal signature, sourced from the module itself — the
/// introspection a module name in type position no longer performs implicitly.
#[test]
fn type_of_module_is_its_self_sig() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(scope, "MODULE IntOrd = ((LET Elt = Number) (LET zero = 7))");
    let module = lookup_module(scope, "IntOrd");
    match run_one_type(scope, parse_one("TYPE OF IntOrd")) {
        KType::Signature {
            sig: SigSource::SelfOf(m),
            ..
        } => assert!(
            std::ptr::eq(*m, module),
            "the self-sig must source the module itself",
        ),
        other => panic!("expected a module's self-sig, got {other:?}"),
    }
}

/// An opaque view is its own module with its own sealed self-sig, so `TYPE OF` reports the
/// *view's* signature — its abstract per-call identities — not the source module's concrete ones.
#[test]
fn type_of_opaque_view_reports_the_view_not_its_source() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "SIG OrderedSig = ((TYPE Elt) (VAL zero :Elt))\n\
         MODULE IntOrd = ((LET Elt = Number) (LET zero = 7))\n\
         LET View = (IntOrd :| OrderedSig)",
    );
    let view = lookup_module(scope, "View");
    let source = lookup_module(scope, "IntOrd");
    match run_one_type(scope, parse_one("TYPE OF View")) {
        KType::Signature {
            sig: SigSource::SelfOf(m),
            ..
        } => {
            assert!(
                std::ptr::eq(*m, view),
                "TYPE OF must report the view itself"
            );
            assert!(
                !std::ptr::eq(*m, source),
                "the opaque view's sealed self-sig is not the source module's",
            );
            assert!(
                matches!(
                    m.self_sig().manifest_members.get("Elt"),
                    Some(KType::AbstractType { .. })
                ),
                "the opaque view's self-sig holds `Elt` as its per-call abstract identity, \
                 not the source's `Number`",
            );
        }
        other => panic!("expected a Signature type, got {other:?}"),
    }
}

/// A transparent view records its source's concrete types, so its self-sig has no abstract
/// members — the complement of the opaque case above.
#[test]
fn type_of_transparent_view_reports_concrete_slots() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "SIG OrderedSig = ((TYPE Elt) (VAL zero :Elt))\n\
         MODULE IntOrd = ((LET Elt = Number) (LET zero = 7))\n\
         LET View = (IntOrd :! OrderedSig)",
    );
    match run_one_type(scope, parse_one("TYPE OF View")) {
        KType::Signature {
            sig: SigSource::SelfOf(m),
            ..
        } => {
            assert_eq!(
                m.self_sig().manifest_members.get("Elt"),
                Some(&KType::Number),
                "a transparent view records the source's concrete `Elt`",
            );
        }
        other => panic!("expected a Signature type, got {other:?}"),
    }
}

/// A `:(TYPE OF …)` parameter slot eager-sub-dispatches the value-head path and lands the module's
/// signature in the slot, so the module satisfies it as an argument.
#[test]
fn type_of_module_types_a_parameter_slot() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "MODULE IntOrd = ((LET Elt = Number) (LET zero = 7))\n\
         FN (TAKE_ORD m :(TYPE OF IntOrd)) -> Number = (m.zero)",
    );
    let result = run_one(scope, parse_one("TAKE_ORD IntOrd"));
    assert!(
        matches!(result, KObject::Number(n) if *n == 7.0),
        "expected the module's `zero`, got {}",
        result.summarize(),
    );
}

/// `-> :(TYPE OF er)` names the *argument's* signature, resolved per call: the return type defers
/// as an expression carrier over the parameter and the returned module satisfies it.
#[test]
fn type_of_parameter_defers_a_return_type() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "SIG OrderedSig = ((TYPE Elt) (VAL zero :Elt))\n\
         MODULE IntOrd = ((LET Elt = Number) (LET zero = 7))\n\
         FN (USE_ORD er :OrderedSig) -> :(TYPE OF er) = (er)",
    );
    let result = run_one(scope, parse_one("USE_ORD IntOrd"));
    assert!(
        matches!(result, KObject::Module(_)),
        "the deferred return must admit the module it was resolved from, got {}",
        result.summarize(),
    );
}

/// A type argument is refused: `TYPE OF` reads the value channel, and a type's own type is not a
/// question the language asks. The `Any` slot admits both channels, so this is a body diagnostic
/// rather than a dispatch miss.
#[test]
fn type_of_a_type_errors() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    let err = run_one_err(scope, parse_one("TYPE OF Number"));
    assert!(
        matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("is already a type")),
        "expected a value-channel ShapeError, got {err}",
    );
}

/// An empty container carries no stamped element type, so it has no knowable type to report.
#[test]
fn type_of_unstamped_empty_container_errors() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    let err = run_one_err(scope, parse_one("TYPE OF []"));
    assert!(
        matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("unknowable")),
        "expected an unknowable-element-type ShapeError, got {err}",
    );
}
