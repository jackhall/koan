//! A bare module name in type position lowers to the module's principal signature
//! (`Signature { SelfOf }`): slots and returns name signatures, so `x :IntOrd` is the structural
//! slot admitting any module whose self-sig subtypes IntOrd's, and `-> Er` (a module-valued
//! parameter) returns a module satisfying `Er`'s interface.

use crate::builtins::test_support::{
    lookup_fn, parse_one, run, run_one, run_one_err, run_root_silent,
};
use crate::machine::core::run_root_storage;
use crate::machine::model::KObject;
use crate::machine::KErrorKind;

/// `-> Er` with a module-valued parameter is a legal return: the deferred `Type` return elaborates
/// per-call to `Signature { SelfOf(Er) }` and the body returns the module value through it.
#[test]
fn deferred_bare_module_param_return_yields_the_module() {
    use crate::machine::model::ReturnType;
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "SIG OrderedSig = (VAL compare :Number)\n\
         MODULE IntOrd = (LET compare = 7)",
    );
    run(scope, "FN (USE_ORD Er :OrderedSig) -> Er = (Er)");
    let f = lookup_fn(scope, "USE_ORD");
    assert!(
        matches!(f.signature.return_type, ReturnType::Deferred(_)),
        "USE_ORD's return type should be Deferred, got {:?}",
        f.signature.return_type,
    );
    match run_one(scope, parse_one("USE_ORD IntOrd")) {
        KObject::Module(m) => assert_eq!(m.path, "IntOrd"),
        other => panic!(
            "USE_ORD IntOrd must return the passed-through module value, got {}",
            other.ktype().name(),
        ),
    }
}

/// The module a `-> Er` return names need not live in the captured scope's region: a FUNCTOR mints
/// its module in its own per-call region, so `Signature { SelfOf(m) }` borrows a region that is
/// neither the callee frame's nor the captured scope's. The home audits against the reach stored on
/// the parameter's own binding — which pins that region — so the module rides the return, and the
/// member read afterwards proves its child scope is still live.
#[test]
fn deferred_bare_module_param_return_admits_a_per_call_region_module() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "SIG OrderedSig = (VAL compare :Number)\n\
         MODULE IntOrd = (LET compare = 7)\n\
         FUNCTOR (MAKESET Er :OrderedSig) -> Module = (MODULE Generated = (LET compare = 3))\n\
         LET IntSet = (MAKESET IntOrd)\n\
         FN (USE_ORD Er :OrderedSig) -> Er = (Er)",
    );
    match run_one(scope, parse_one("USE_ORD IntSet")) {
        KObject::Module(m) => assert_eq!(m.path, "Generated"),
        other => panic!(
            "USE_ORD IntSet must return the functor-minted module, got {}",
            other.ktype().name(),
        ),
    }
    run(scope, "LET Back = (USE_ORD IntSet)");
    assert!(
        matches!(run_one(scope, parse_one("Back.compare")), KObject::Number(n) if *n == 3.0),
        "the returned module's child scope must still be readable",
    );
}

/// The per-call return contract for `-> Er` is the argument module's self-sig: a body producing a
/// non-module value fails the check, and the diagnostic names the module (its self-sig renders as
/// the module path).
#[test]
fn deferred_bare_module_param_return_contract_is_the_self_sig() {
    use crate::machine::execute::KoanRuntime;
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "SIG OrderedSig = (VAL compare :Number)\n\
         MODULE IntOrd = (LET compare = 7)",
    );
    run(scope, "FN (BAD_ORD Er :OrderedSig) -> Er = (1)");
    let mut runtime = KoanRuntime::new();
    let id = runtime.dispatch_in_scope(parse_one("BAD_ORD IntOrd"), scope);
    runtime
        .execute()
        .expect("execute does not surface per-slot errors");
    let error = match runtime.result_error(id) {
        Err(e) => e,
        Ok(()) => panic!("BAD_ORD should fail the per-call return-type check"),
    };
    match &error.kind {
        KErrorKind::TypeMismatch { arg, expected, .. } => {
            assert_eq!(arg, "<return>");
            assert!(
                expected.contains("IntOrd") && expected.contains("per-call return type"),
                "the contract is IntOrd's self-sig, got `{expected}`",
            );
        }
        _ => panic!("expected TypeMismatch on <return>, got {error}"),
    }
}

/// A module-headed slot (`x :IntOrd`) is structural: it admits any module whose self-sig subtypes
/// IntOrd's — no ascription required, and the module need not be IntOrd itself.
#[test]
fn module_headed_slot_admits_a_structurally_satisfying_module() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "MODULE IntOrd = ((LET Carrier = Number) (LET compare = 7))\n\
         MODULE AlsoOrd = ((LET Carrier = Number) (LET compare = 3))",
    );
    run(scope, "FN (TAKE_ORD x :IntOrd) -> Number = (1)");
    assert!(
        matches!(run_one(scope, parse_one("TAKE_ORD IntOrd")), KObject::Number(n) if *n == 1.0),
        "the module itself satisfies its own self-sig",
    );
    assert!(
        matches!(run_one(scope, parse_one("TAKE_ORD AlsoOrd")), KObject::Number(n) if *n == 1.0),
        "a structurally-satisfying module is admitted without ascription",
    );
}

/// The negative half: a module missing a member of the slot's self-sig is a dispatch non-match, so
/// the call falls through to "no overload".
#[test]
fn module_headed_slot_rejects_a_non_satisfying_module() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "MODULE IntOrd = ((LET Carrier = Number) (LET compare = 7))\n\
         MODULE NotOrd = (LET other = 1)",
    );
    run(scope, "FN (TAKE_ORD x :IntOrd) -> Number = (1)");
    let error = run_one_err(scope, parse_one("TAKE_ORD NotOrd"));
    assert!(
        matches!(&error.kind, KErrorKind::DispatchFailed { reason, .. }
            if reason.contains("no matching function")),
        "a non-satisfying module is a dispatch non-match, got {error}",
    );
}

/// Known asymmetry: only `TypeIdentifier` elaboration lowers a module head to its self-sig. In
/// *type-language dispatch* (`:(LIST OF IntOrd)`) the head resolves to a module **value**, which a
/// type slot refuses — so the annotation is an error.
#[test]
fn module_head_in_type_language_dispatch_is_an_error() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(scope, "MODULE IntOrd = (LET compare = 7)");
    let error = run_one_err(scope, parse_one("LET xs :(LIST OF IntOrd) = [1]"));
    assert!(
        matches!(&error.kind, KErrorKind::DispatchFailed { reason, .. }
            if reason.contains("no matching function")),
        "the `OF` type slot refuses the module value, got {error}",
    );
}
