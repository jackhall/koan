//! The bodyless `FN (<head>) -> <Return>` declarator: a SIG body's keyworded (dispatch-bucket)
//! member. The head parses through the definition form's own path, so these tests pin what the
//! declaration *records* — the bucket key and the `(params) -> ret` type in the signature's stored
//! schema — plus the two guards that keep declaration and definition in their own bodies.

use crate::builtins::test_support::{TestRun, key_keyword, type_name};
use crate::machine::model::{KType, KeyElement, SigSchema, TypeNode, UntypedKey};
use crate::machine::{program_storage, run_root_storage};

/// The stored schema of the signature `name` binds in `scope`.
fn sig_schema(
    scope: &crate::machine::Scope<'_>,
    types: &crate::machine::model::TypeRegistry,
    name: &str,
) -> SigSchema {
    let handle = crate::builtins::test_support::lookup_type(scope, name)
        .unwrap_or_else(|| panic!("{name} must bind a type"));
    match types.node(handle) {
        TypeNode::Signature { schema, .. } => schema,
        _ => panic!("{name} must bind a Signature KType"),
    }
}

/// A bucket key spelled out: `_` is an argument slot, anything else a fixed token.
fn key(spelling: &[&str]) -> UntypedKey {
    spelling
        .iter()
        .map(|part| match *part {
            "_" => KeyElement::Slot,
            token => key_keyword(token),
        })
        .collect()
}

#[test]
fn a_bodyless_head_records_its_key_and_function_type() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run("SIG Pure = ((FN (PURE x :Number) -> Number))");
    let schema = sig_schema(scope, test_run.types(), "Pure");
    let overloads = schema
        .keyworded
        .get(&key(&["PURE", "_"]))
        .expect("the declared head keys `(PURE _)`");
    assert_eq!(overloads.len(), 1);
    let expected = test_run.types().function_type(
        crate::machine::model::Record::from_pairs([(
            crate::builtins::test_support::binder_token("x"),
            KType::NUMBER,
        )]),
        KType::NUMBER,
    );
    assert_eq!(overloads[0], expected);
}

#[test]
fn two_textually_identical_signatures_with_keyworded_members_intern_once() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run(
        "SIG One = ((FN (PURE x :Number) -> Number))\n\
         SIG Two = ((FN (PURE x :Number) -> Number))",
    );
    let one = crate::builtins::test_support::lookup_type(scope, "One").expect("One binds");
    let two = crate::builtins::test_support::lookup_type(scope, "Two").expect("Two binds");
    assert_eq!(one, two, "one interface content, one interned type");
}

#[test]
fn same_key_declarations_at_different_types_accumulate_as_overloads() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run("SIG Pure = ((FN (PURE x :Number) -> Number) (FN (PURE x :Str) -> Str))");
    let schema = sig_schema(scope, test_run.types(), "Pure");
    assert_eq!(
        schema.keyworded.get(&key(&["PURE", "_"])).map(Vec::len),
        Some(2),
    );
}

#[test]
fn an_exact_duplicate_declaration_is_a_rebind() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let error =
        test_run.run_one_err(test_run.parse_one(
            "SIG Pure = ((FN (PURE x :Number) -> Number) (FN (PURE x :Number) -> Number))",
        ));
    assert!(
        matches!(&error.kind, crate::machine::KErrorKind::Rebind { .. }),
        "expected a Rebind naming the head, got {error}",
    );
}

#[test]
fn a_declared_head_may_name_an_abstract_member() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run("SIG Box = ((TYPE Carrier) (FN (PURE x :Carrier) -> Carrier))");
    let schema = sig_schema(scope, test_run.types(), "Box");
    let carrier = *schema
        .abstract_members
        .get(&type_name("Carrier", test_run.registries()))
        .expect("Carrier is an abstract member");
    let overloads = schema
        .keyworded
        .get(&key(&["PURE", "_"]))
        .expect("the declared head keys `(PURE _)`");
    let expected = test_run.types().function_type(
        crate::machine::model::Record::from_pairs([(
            crate::builtins::test_support::binder_token("x"),
            carrier,
        )]),
        carrier,
    );
    assert_eq!(
        overloads[0], expected,
        "the head's types name the SIG's own abstract member, canonicalized with the binder",
    );
}

#[test]
fn a_definition_inside_a_sig_body_points_at_the_declarator() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let error = test_run
        .run_one_err(test_run.parse_one("SIG Pure = ((FN (PURE x :Number) -> Number = (x)))"));
    assert!(
        error.to_string().contains("declared rather than defined"),
        "expected the message to point at the other form, got {error}",
    );
}

#[test]
fn a_bodyless_head_outside_a_sig_body_points_at_the_definition_form() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let error = test_run.run_one_err(test_run.parse_one("(FN (PURE x :Number) -> Number)"));
    assert!(
        error.to_string().contains("only valid inside a SIG body"),
        "expected the message to point at the other form, got {error}",
    );
}

#[test]
fn a_keywordless_head_has_no_bucket_to_declare() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let error = test_run.run_one_err(test_run.parse_one("SIG Pure = ((FN (x :Number) -> Number))"));
    assert!(
        error.to_string().contains("at least one Keyword"),
        "expected the message to point at the other form, got {error}",
    );
}
