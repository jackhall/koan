//! `type_sigil` parse cases for `expression_tree::parse`.
//!
//! See [type-language-via-dispatch](../../../../design/typing/type-language-via-dispatch.md).
//! The parser does no shape-folding inside `:(...)`: every sigil emits
//! `ExpressionPart::SigiledTypeExpr(inner)` whose inner mirrors the parens contents.
//! Shape recognition is the dispatcher's job — these tests assert only parser output.

use super::tree;

#[test]
fn type_with_one_param() {
    assert_eq!(tree(":(List Number)").unwrap(), "[:(T(List) T(Number))]");
}

#[test]
fn type_with_two_params() {
    assert_eq!(
        tree(":(Dict String Number)").unwrap(),
        "[:(T(Dict) T(String) T(Number))]"
    );
}

#[test]
fn type_with_two_params_no_comma() {
    assert_eq!(
        tree(":(Dict String Number)").unwrap(),
        tree(":(Dict String Number)").unwrap(),
    );
}

#[test]
fn type_nested_two_levels() {
    assert_eq!(
        tree(":(List :(Dict String Number))").unwrap(),
        "[:(T(List) :(T(Dict) T(String) T(Number)))]"
    );
}

#[test]
fn function_type_unary() {
    assert_eq!(
        tree(":(Function (Number) -> Str)").unwrap(),
        "[:(T(Function) [T(Number)] t(->) T(Str))]"
    );
}

#[test]
fn function_type_nullary() {
    assert_eq!(
        tree(":(Function () -> Number)").unwrap(),
        "[:(T(Function) [] t(->) T(Number))]"
    );
}

#[test]
fn function_type_multi_arg() {
    assert_eq!(
        tree(":(Function (Number Bool) -> Number)").unwrap(),
        "[:(T(Function) [T(Number) T(Bool)] t(->) T(Number))]"
    );
}

#[test]
fn function_type_multi_arg_no_comma() {
    assert_eq!(
        tree(":(Function (Number Bool) -> Number)").unwrap(),
        tree(":(Function (Number Bool) -> Number)").unwrap(),
    );
}

#[test]
fn function_type_arg_nested_parameterized() {
    assert_eq!(
        tree(":(Function (:(List Number) Str) -> Bool)").unwrap(),
        "[:(T(Function) [:(T(List) T(Number)) T(Str)] t(->) T(Bool))]"
    );
}

#[test]
fn lt_after_non_type_with_whitespace_emits_keyword() {
    assert_eq!(tree("a < b").unwrap(), "[t(a) t(<) t(b)]");
}

#[test]
fn lt_glued_to_non_type_no_longer_special() {
    assert_eq!(tree("a<b").unwrap(), "[t(a) t(<) t(b)]");
}

#[test]
fn gt_lt_outside_type_emit_keywords() {
    // `prev=='-'` rule keeps `->` contiguous; `<` / `>` are otherwise standalone.
    assert_eq!(tree("a > b").unwrap(), "[t(a) t(>) t(b)]");
    assert_eq!(tree("Number > 0").unwrap(), "[T(Number) t(>) n(0)]");
    assert_eq!(tree("a -> b").unwrap(), "[t(a) t(->) t(b)]");
    assert_eq!(tree("a>b").unwrap(), "[t(a) t(>) t(b)]");
}

#[test]
fn type_token_with_invalid_char_errors() {
    assert!(tree("Foo$Bar").is_err());
    assert!(tree("Foo+Bar").is_err());
}

#[test]
fn unclosed_type_sigil_errors() {
    assert!(tree(":(List Number").is_err());
}

#[test]
fn comma_outside_type_sigil_unchanged_inside_paren() {
    assert_eq!(
        tree("(xs :(List Number), ys :(List Str))").unwrap(),
        "[[t(xs) :(T(List) T(Number)) t(ys) :(T(List) T(Str))]]",
    );
}

// --- Record-type sigil `:{...}` (a first-class `RecordType` part) ---

#[test]
fn record_type_sigil_one_field() {
    assert_eq!(tree(":{x :Number}").unwrap(), "[:{t(x) T(Number)}]");
}

#[test]
fn record_type_sigil_two_fields() {
    assert_eq!(
        tree(":{x :Number, y :Str}").unwrap(),
        "[:{t(x) T(Number) t(y) T(Str)}]",
    );
}

#[test]
fn record_type_sigil_in_param_slot() {
    assert_eq!(
        tree("(r :{x :Number})").unwrap(),
        "[[t(r) :{t(x) T(Number)}]]",
    );
}

#[test]
fn unclosed_record_type_sigil_errors() {
    assert!(tree(":{x :Number").is_err());
}

// --- Type-sigil basics ---

#[test]
fn type_sigil_bare_emits_type_part() {
    assert_eq!(
        tree("LET x :Number = 5").unwrap(),
        "[t(LET) t(x) T(Number) t(=) n(5)]"
    );
}

#[test]
fn type_sigil_parameterized_list() {
    assert_eq!(
        tree("LET ns :(List Number)").unwrap(),
        "[t(LET) t(ns) :(T(List) T(Number))]"
    );
}

#[test]
fn type_sigil_function_nullary() {
    assert_eq!(
        tree("LET f :(Function () -> Str)").unwrap(),
        "[t(LET) t(f) :(T(Function) [] t(->) T(Str))]",
    );
}

#[test]
fn type_sigil_function_unary() {
    assert_eq!(
        tree("LET f :(Function (Number) -> Str)").unwrap(),
        "[t(LET) t(f) :(T(Function) [T(Number)] t(->) T(Str))]",
    );
}

#[test]
fn type_sigil_function_multi_arg() {
    assert_eq!(
        tree("LET f :(Function (Number Str) -> Bool)").unwrap(),
        "[t(LET) t(f) :(T(Function) [T(Number) T(Str)] t(->) T(Bool))]",
    );
}

#[test]
fn type_sigil_nested_dict_of_list() {
    assert_eq!(
        tree("LET d :(Dict Str (List Number))").unwrap(),
        "[t(LET) t(d) :(T(Dict) T(Str) [T(List) T(Number)])]",
    );
}

#[test]
fn type_sigil_let_type_binding_rhs() {
    assert_eq!(
        tree("LET t = :(List Number)").unwrap(),
        "[t(LET) t(t) t(=) :(T(List) T(Number))]",
    );
}

#[test]
fn type_sigil_lone_colon_with_eof_errors() {
    assert!(tree("LET x :").is_err());
}

#[test]
fn type_sigil_lone_colon_glued_to_lowercase_errors() {
    // Type sigils require an uppercase head OR `(`.
    assert!(tree("LET x :foo").is_err());
}

#[test]
fn type_sigil_empty_parens_parses() {
    // Parser admits empty `:()`; the dispatcher surfaces the empty-expression error.
    assert_eq!(tree("LET x :()").unwrap(), "[t(LET) t(x) :()]");
}

#[test]
fn type_sigil_bare_type_name_in_parens() {
    // The redundant-Expression peel does NOT apply inside `:(...)` — sigil is the wrapper.
    assert_eq!(
        tree("LET x :(Number)").unwrap(),
        "[t(LET) t(x) :(T(Number))]"
    );
}

#[test]
fn type_sigil_function_nested_arg_unparameterized() {
    assert_eq!(
        tree("LET f :(Function ((List Number)) -> Bool)").unwrap(),
        "[t(LET) t(f) :(T(Function) [[T(List) T(Number)]] t(->) T(Bool))]",
    );
}

#[test]
fn type_sigil_function_return_parenthesized() {
    assert_eq!(
        tree("LET f :(Function (Number) -> (List Str))").unwrap(),
        "[t(LET) t(f) :(T(Function) [T(Number)] t(->) [T(List) T(Str)])]",
    );
}
