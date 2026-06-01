//! `literals` parse cases for `expression_tree::parse`.

use super::tree;

#[test]
fn string_literal() {
    assert_eq!(tree(r#"say "hello""#).unwrap(), "[t(say) s(hello)]");
}

#[test]
fn empty_string_literal() {
    assert_eq!(tree(r#""""#).unwrap(), "[s()]");
}

#[test]
fn literal_adjacent_to_token() {
    assert_eq!(tree(r#"foo"bar"baz"#).unwrap(), "[t(foo) s(bar) t(baz)]");
}

#[test]
fn integer_literal() {
    assert_eq!(tree("42").unwrap(), "[n(42)]");
}

#[test]
fn signed_integers() {
    assert_eq!(tree("-5 +7 0 42").unwrap(), "[n(-5) n(7) n(0) n(42)]");
}

#[test]
fn floats_and_scientific_are_number_literals() {
    assert_eq!(
        tree("3.14 1e3 -2.5e-2").unwrap(),
        "[n(3.14) n(1000) n(-0.025)]"
    );
}

#[test]
fn bool_and_null_literals() {
    assert_eq!(tree("true false null").unwrap(), "[b(true) b(false) null]");
}

#[test]
fn inf_and_nan_stay_tokens() {
    // `inf` is lowercase → Identifier; `NaN` is capitalized + has lowercase → Type.
    // Neither classifies as a numeric Literal, which is what this test guards.
    assert_eq!(tree("inf NaN").unwrap(), "[t(inf) T(NaN)]");
}

#[test]
fn capitalized_names_classify_as_types_all_caps_as_keyword() {
    assert_eq!(
        tree("True False Null NULL").unwrap(),
        "[T(True) T(False) T(Null) t(NULL)]"
    );
}

#[test]
fn camelcase_type_names_classify_as_types() {
    assert_eq!(
        tree("Number MyType KFunction Point3D").unwrap(),
        "[T(Number) T(MyType) T(KFunction) T(Point3D)]"
    );
}

#[test]
fn mixed_expression() {
    assert_eq!(
        tree(r#"(set x 42) (set flag true) (set name "bob")"#).unwrap(),
        "[[t(set) t(x) n(42)] [t(set) t(flag) b(true)] [t(set) t(name) s(bob)]]"
    );
}

#[test]
fn identifiers_with_digits_stay_tokens() {
    assert_eq!(tree("x1 foo2bar").unwrap(), "[t(x1) t(foo2bar)]");
}

#[test]
fn identifier_token_with_invalid_char_errors() {
    // Identifiers reject everything except letters, digits, and `_`.
    assert!(tree("a+b").is_err());
    assert!(tree("foo@bar").is_err());
}

#[test]
fn identifier_underscore_allowed() {
    assert_eq!(
        tree("my_var another_one").unwrap(),
        "[t(my_var) t(another_one)]"
    );
}
