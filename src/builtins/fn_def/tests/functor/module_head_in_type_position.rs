//! A module is a value, so its name types nothing on its own: a bare module name in a slot or a
//! return is an error. A module reaches type position through `TYPE OF`, which yields its principal
//! signature as an ordinary type value — so `x :(TYPE OF int_ord)` is the structural slot admitting
//! any module whose self-sig subtypes int_ord's, and `-> :(TYPE OF er)` (a module-valued parameter)
//! returns a module satisfying `er`'s interface.

use crate::builtins::test_support::{
    lookup_fn, parse_one, run, run_one, run_one_err, run_root_silent,
};
use crate::machine::model::KObject;
use crate::machine::{core::run_root_storage, KErrorKind};

/// `-> :(TYPE OF er)` with a module-valued parameter is a legal return: the return type defers as an
/// expression carrier and resolves per-call to `Signature { SelfOf(er) }`, and the body returns the
/// module value through it.
#[test]
fn deferred_type_of_param_return_yields_the_module() {
    use crate::machine::model::ReturnType;
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "SIG Ordered = (VAL compare :Number)\n\
         MODULE int_ord = (LET compare = 7)",
    );
    run(scope, "FN (USE_ORD er :Ordered) -> :(TYPE OF er) = (er)");
    let f = lookup_fn(scope, "USE_ORD");
    assert!(
        matches!(f.signature.return_type, ReturnType::Deferred(_)),
        "USE_ORD's return type should be Deferred, got {:?}",
        f.signature.return_type,
    );
    match run_one(scope, parse_one("USE_ORD int_ord")) {
        KObject::Module(m) => assert_eq!(m.path, "int_ord"),
        other => panic!(
            "USE_ORD int_ord must return the passed-through module value, got {}",
            other.ktype().name(),
        ),
    }
}

/// The module a `-> :(TYPE OF er)` return names need not live in the captured scope's region: a
/// FUNCTOR mints its module in its own per-call region, so `Signature { SelfOf(m) }` borrows a region
/// that is neither the callee frame's nor the captured scope's. `TYPE OF` homes its result under the
/// argument carrier's own stored reach — which pins that region — so the module rides the return, and
/// the member read afterwards proves its child scope is still live.
#[test]
fn deferred_type_of_param_return_admits_a_per_call_region_module() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "SIG Ordered = (VAL compare :Number)\n\
         MODULE int_ord = (LET compare = 7)\n\
         FUNCTOR (MAKESET er :Ordered) -> Module = (MODULE generated = (LET compare = 3))\n\
         LET int_set = (MAKESET int_ord)\n\
         FN (USE_ORD er :Ordered) -> :(TYPE OF er) = (er)",
    );
    match run_one(scope, parse_one("USE_ORD int_set")) {
        KObject::Module(m) => assert_eq!(m.path, "generated"),
        other => panic!(
            "USE_ORD int_set must return the functor-minted module, got {}",
            other.ktype().name(),
        ),
    }
    run(scope, "LET back = (USE_ORD int_set)");
    assert!(
        matches!(run_one(scope, parse_one("back.compare")), KObject::Number(n) if *n == 3.0),
        "the returned module's child scope must still be readable",
    );
}

/// The per-call return contract for `-> :(TYPE OF er)` is the argument module's self-sig: a body
/// producing a non-module value fails the check, and the diagnostic names the module (its self-sig
/// renders as the module path).
#[test]
fn deferred_type_of_param_return_contract_is_the_self_sig() {
    use crate::machine::execute::KoanRuntime;
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "SIG Ordered = (VAL compare :Number)\n\
         MODULE int_ord = (LET compare = 7)",
    );
    run(scope, "FN (BAD_ORD er :Ordered) -> :(TYPE OF er) = (1)");
    let mut runtime = KoanRuntime::new();
    let id = runtime.dispatch_in_scope(parse_one("BAD_ORD int_ord"), scope);
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
                expected.contains("int_ord") && expected.contains("per-call return type"),
                "the contract is int_ord's self-sig, got `{expected}`",
            );
        }
        _ => panic!("expected TypeMismatch on <return>, got {error}"),
    }
}

/// A `TYPE OF`-headed slot (`x :(TYPE OF int_ord)`) is structural: it admits any module whose self-sig
/// subtypes int_ord's — no ascription required, and the module need not be int_ord itself.
#[test]
fn type_of_module_slot_admits_a_structurally_satisfying_module() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "MODULE int_ord = ((LET Carrier = Number) (LET compare = 7))\n\
         MODULE also_ord = ((LET Carrier = Number) (LET compare = 3))",
    );
    run(scope, "FN (TAKE_ORD x :(TYPE OF int_ord)) -> Number = (1)");
    assert!(
        matches!(run_one(scope, parse_one("TAKE_ORD int_ord")), KObject::Number(n) if *n == 1.0),
        "the module itself satisfies its own self-sig",
    );
    assert!(
        matches!(run_one(scope, parse_one("TAKE_ORD also_ord")), KObject::Number(n) if *n == 1.0),
        "a structurally-satisfying module is admitted without ascription",
    );
}

/// The negative half: a module missing a member of the slot's self-sig is a dispatch non-match, so
/// the call falls through to "no overload".
#[test]
fn type_of_module_slot_rejects_a_non_satisfying_module() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "MODULE int_ord = ((LET Carrier = Number) (LET compare = 7))\n\
         MODULE not_ord = (LET other = 1)",
    );
    run(scope, "FN (TAKE_ORD x :(TYPE OF int_ord)) -> Number = (1)");
    let error = run_one_err(scope, parse_one("TAKE_ORD not_ord"));
    assert!(
        matches!(&error.kind, KErrorKind::DispatchFailed { reason, .. }
            if reason.contains("no matching function")),
        "a non-satisfying module is a dispatch non-match, got {error}",
    );
}

/// A module name in a slot does not even lex as a type: the `:` sigil takes a Type token, and a
/// module name is a value token. The mistake is caught at parse time, and the diagnostic names the
/// replacement spelling.
#[test]
fn module_name_in_a_slot_is_a_parse_error() {
    let error = crate::parse::parse("FN (TAKE_ORD x :int_ord) -> Number = (1)")
        .expect_err("a value token after `:` must not parse");
    let message = error.to_string();
    assert!(
        message.contains("must be followed by a type name")
            && message.contains(":(TYPE OF <value>)"),
        "the parse error must name the `TYPE OF` spelling, got {message}",
    );
}

/// A module-valued parameter in return position (`-> er`) is a return slot naming a value. FN's
/// value-named-return overload exists to say so and to name the replacement spelling.
#[test]
fn module_param_in_return_position_errors() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(scope, "SIG Ordered = (VAL compare :Number)");
    let error = run_one_err(scope, parse_one("FN (USE_ORD er :Ordered) -> er = (er)"));
    assert!(
        matches!(&error.kind, KErrorKind::ShapeError(msg)
            if msg.contains("names a type, but `er` is a value")
                && msg.contains("-> :(TYPE OF er)")),
        "the return slot must name the `TYPE OF` respelling, got {error}",
    );
}

/// A parameter's name picks its universe, not its argument: a Type-token parameter names a type, so
/// handing it a module — a value — is refused by the binding maps' token-class partition. The
/// module-valued parameter spells snake_case.
#[test]
fn type_token_param_cannot_take_a_module() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "SIG Ordered = (VAL compare :Number)\n\
         MODULE int_ord = (LET compare = 7)\n\
         FN (USE_ORD Er :Ordered) -> Number = (1)",
    );
    let error = run_one_err(scope, parse_one("USE_ORD int_ord"));
    assert!(
        matches!(&error.kind, KErrorKind::ShapeError(msg)
            if msg.contains("`Er` is a Type token")),
        "a module may not bind to a Type-token parameter, got {error}",
    );
}

/// Type-language dispatch (`:(LIST OF int_ord)`) refuses the module value the same way a slot does:
/// the `OF` type slot takes a type, and a module head resolves to a value.
#[test]
fn module_head_in_type_language_dispatch_is_an_error() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(scope, "MODULE int_ord = (LET compare = 7)");
    let error = run_one_err(scope, parse_one("LET xs :(LIST OF int_ord) = [1]"));
    assert!(
        matches!(&error.kind, KErrorKind::DispatchFailed { reason, .. }
            if reason.contains("no matching function")),
        "the `OF` type slot refuses the module value, got {error}",
    );
}
