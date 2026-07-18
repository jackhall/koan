//! koan-level `==` / `!=` dispatch: binary-only structural equality over `:Any` operands.
//! The comparability gate's intransitivity, nominal identity, the function/module ban, and the
//! `(TYPE OF m) ==` interface idiom all exercise the real dispatch path here.

use crate::builtins::test_support::{parse_one, run, run_one, run_one_err, run_root_silent};
use crate::machine::model::{KObject, Parseable};
use crate::machine::run_root_storage;
use crate::machine::KErrorKind;

fn eval_bool(source_setup: &str, probe: &str) -> bool {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    if !source_setup.is_empty() {
        run(scope, source_setup);
    }
    match run_one(scope, parse_one(probe)) {
        KObject::Bool(b) => *b,
        other => panic!("expected Bool from `{probe}`, got {}", other.summarize()),
    }
}

fn probe_bool(probe: &str) -> bool {
    eval_bool("", probe)
}

// --- scalars ----------------------------------------------------------------------

#[test]
fn number_equality() {
    assert!(probe_bool("1 == 1"));
    assert!(!probe_bool("1 == 2"));
    assert!(probe_bool("1 != 2"));
    assert!(!probe_bool("1 != 1"));
}

#[test]
fn string_and_bool_equality() {
    assert!(probe_bool("\"a\" == \"a\""));
    assert!(!probe_bool("\"a\" == \"b\""));
    assert!(probe_bool("true == true"));
    assert!(probe_bool("true != false"));
}

#[test]
fn cross_type_is_false() {
    assert!(!probe_bool("1 == \"a\""));
    assert!(probe_bool("1 != \"a\""));
}

// --- lists / records / dicts ------------------------------------------------------

#[test]
fn list_equality() {
    assert!(probe_bool("[1 2] == [1 2]"));
    assert!(!probe_bool("[1 2] == [1 3]"));
    assert!(!probe_bool("[1 2] == [1]"));
}

#[test]
fn record_reorder_is_equal() {
    assert!(probe_bool("{x = 1, y = 2} == {y = 2, x = 1}"));
    assert!(!probe_bool("{x = 1} == {x = 2}"));
}

#[test]
fn dict_equality() {
    assert!(probe_bool("{\"a\": 1, \"b\": 2} == {\"b\": 2, \"a\": 1}"));
    assert!(!probe_bool("{\"a\": 1} == {\"a\": 2}"));
}

// The comparability gate's intransitivity (stamped empty lists relating through `Any`) and the
// ban propagating from inside a container are `value_equal` semantics, verified directly in
// `values/kobject/equality/tests.rs`. They are not re-exercised through koan source here: a koan
// list literal resolves an identifier element to its *name string* (`[f]` is `["f"]`), so a
// function value cannot be placed inside a literal container, and stamping an empty list to a
// non-`Any` element type has no ergonomic koan surface.

// --- nominal identity -------------------------------------------------------------

#[test]
fn newtype_is_distinct_from_its_representation() {
    // A `Wrapped` value is never equal to its bare representation, and two distinct newtypes
    // over the same representation are unequal.
    assert!(!eval_bool(
        "NEWTYPE Distance = :{n :Number}\nLET d = (Distance {n = 3})",
        "d == {n = 3}"
    ));
}

#[test]
fn same_newtype_same_repr_is_equal() {
    assert!(eval_bool(
        "NEWTYPE Distance = :{n :Number}\n\
         LET a = (Distance {n = 3})\n\
         LET b = (Distance {n = 3})",
        "a == b"
    ));
}

#[test]
fn two_newtypes_same_repr_are_unequal() {
    assert!(!eval_bool(
        "NEWTYPE Distance = :{n :Number}\n\
         NEWTYPE Weight = :{n :Number}\n\
         LET d = (Distance {n = 3})\n\
         LET w = (Weight {n = 3})",
        "d == w"
    ));
}

// --- module interface via TYPE OF -------------------------------------------------

#[test]
fn identical_module_interfaces_compare_equal_via_type_of() {
    assert!(eval_bool(
        "MODULE m1 = ((LET Elt = Number) (LET zero = 7))\n\
         MODULE m2 = ((LET Elt = Number) (LET zero = 7))",
        "(TYPE OF m1) == (TYPE OF m2)"
    ));
}

#[test]
fn distinct_module_interfaces_compare_unequal_via_type_of() {
    assert!(!eval_bool(
        "MODULE m1 = ((LET Elt = Number) (LET zero = 7))\n\
         MODULE m3 = ((LET Elt = Str) (LET zero = 7))",
        "(TYPE OF m1) == (TYPE OF m3)"
    ));
}

#[test]
fn opaque_views_have_distinct_type_of() {
    // Each `:|` ascription mints a fresh generative identity, so their `TYPE OF` types differ.
    assert!(!eval_bool(
        "SIG Ordered = ((TYPE Elt) (VAL zero :Elt))\n\
         MODULE int_ord = ((LET Elt = Number) (LET zero = 7))\n\
         LET v1 = (int_ord :| Ordered)\n\
         LET v2 = (int_ord :| Ordered)",
        "(TYPE OF v1) == (TYPE OF v2)"
    ));
}

// --- banned operands --------------------------------------------------------------

fn err_kind_user(setup: &str, probe: &str) -> String {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    if !setup.is_empty() {
        run(scope, setup);
    }
    let err = run_one_err(scope, parse_one(probe));
    match &err.kind {
        KErrorKind::User(msg) => msg.clone(),
        _ => panic!("expected a User error, got: {err}"),
    }
}

#[test]
fn module_operand_is_error() {
    let m = lookup_module_setup();
    let msg = err_kind_user(&m, "m1 == m2");
    assert!(
        msg.contains("module") && msg.contains("TYPE OF"),
        "module ban should point at TYPE OF, got: {msg}"
    );
}

fn lookup_module_setup() -> String {
    "MODULE m1 = ((LET zero = 7))\nMODULE m2 = ((LET zero = 7))".to_string()
}

#[test]
fn function_operand_is_error() {
    let msg = err_kind_user("LET f = (FN :{x :Number} -> Number = (x))", "f == f");
    assert!(msg.contains("function"), "function ban message, got: {msg}");
}

// --- chain rejection --------------------------------------------------------------

#[test]
fn equality_does_not_chain() {
    // `==` is in no operator group, so a three-operand chain resolves to nothing and surfaces a
    // real (non-empty) resolution error rather than reducing pairwise.
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    let err = run_one_err(scope, parse_one("1 == 2 == 3"));
    assert!(
        !err.to_string().is_empty(),
        "a chain of `==` should surface a resolution error",
    );
}
