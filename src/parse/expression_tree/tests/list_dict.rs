//! `list_dict` parse cases for `expression_tree::parse`.

use super::{top, tree};

#[test]
fn literal_inside_parens() {
    assert_eq!(
        tree(r#"print ("hello" to 'world')"#).unwrap(),
        "[t(print) [s(hello) t(to) s(world)]]"
    );
}

#[test]
fn empty_list_literal() {
    assert_eq!(tree("[]").unwrap(), "[L[]]");
}

#[test]
fn flat_list_literal() {
    assert_eq!(tree("[1 2 3]").unwrap(), "[L[n(1) n(2) n(3)]]");
}

#[test]
fn list_literal_with_identifiers_and_strings() {
    assert_eq!(tree(r#"[a "hi" 4]"#).unwrap(), "[L[t(a) s(hi) n(4)]]",);
}

#[test]
fn nested_list_literal() {
    assert_eq!(
        tree("[[1 2] [3 4]]").unwrap(),
        "[L[L[n(1) n(2)] L[n(3) n(4)]]]",
    );
}

#[test]
fn list_inside_paren_expression() {
    assert_eq!(
        tree("(LET xs = [1 2 3])").unwrap(),
        "[[t(LET) t(xs) t(=) L[n(1) n(2) n(3)]]]",
    );
}

#[test]
fn paren_expression_inside_list() {
    assert_eq!(
        tree("[(LET x = 1) y]").unwrap(),
        "[L[[t(LET) t(x) t(=) n(1)] t(y)]]",
    );
}

#[test]
fn open_bracket_without_close_errors() {
    assert!(tree("[1 2 3").is_err());
}

#[test]
fn close_bracket_without_open_errors() {
    assert!(tree("1 2]").is_err());
}

#[test]
fn close_paren_when_innermost_is_list_errors() {
    let err = tree("[1 2)").unwrap_err();
    assert!(
        err.contains("unclosed '['"),
        "expected unclosed-'[' diagnostic, got: {err}",
    );
}

#[test]
fn close_paren_when_innermost_is_dict_errors() {
    let err = tree("{a: 1)").unwrap_err();
    assert!(
        err.contains("unclosed '{'"),
        "expected unclosed-'{{' diagnostic, got: {err}",
    );
}

#[test]
fn open_bracket_glued_to_token_errors() {
    // List literals must stand alone — `foo [2]` for a sibling list.
    assert!(tree("foo[2]").is_err());
}

#[test]
fn close_bracket_glued_to_token_errors() {
    assert!(tree("[1 2]bar").is_err());
}

#[test]
fn open_bracket_glued_to_close_paren_errors() {
    assert!(tree("(x)[2]").is_err());
}

#[test]
fn close_bracket_glued_to_open_paren_errors() {
    assert!(tree("[1](2)").is_err());
}

#[test]
fn open_bracket_after_string_errors() {
    assert!(tree(r#""hi"[1]"#).is_err());
}

#[test]
fn list_after_whitespace_is_fine() {
    assert_eq!(tree("foo [2]").unwrap(), "[t(foo) L[n(2)]]");
}

#[test]
fn list_literal_with_commas() {
    assert_eq!(tree("[1, 2, 3]").unwrap(), "[L[n(1) n(2) n(3)]]");
}

#[test]
fn list_with_and_without_commas_match() {
    assert_eq!(tree("[1, 2, 3]").unwrap(), tree("[1 2 3]").unwrap());
}

#[test]
fn list_literal_with_trailing_comma() {
    assert_eq!(tree("[1, 2,]").unwrap(), "[L[n(1) n(2)]]");
}

#[test]
fn list_literal_with_mixed_separators() {
    assert_eq!(tree("[1 , 2 ,3]").unwrap(), "[L[n(1) n(2) n(3)]]");
}

#[test]
fn adjacent_brackets_in_nested_list_are_fine() {
    assert_eq!(tree("[[1 2]]").unwrap(), "[L[L[n(1) n(2)]]]");
}

// --- Dict literal tests ---

#[test]
fn empty_dict_literal() {
    assert_eq!(tree("{}").unwrap(), "[D{}]");
}

#[test]
fn two_pairs_with_comma() {
    assert_eq!(tree("{a: 1, b: 2}").unwrap(), "[D{t(a): n(1), t(b): n(2)}]",);
}

#[test]
fn two_pairs_without_comma() {
    // Auto-commit: `b` arriving while value=[1] commits the prior pair.
    assert_eq!(tree("{a: 1 b: 2}").unwrap(), "[D{t(a): n(1), t(b): n(2)}]",);
}

#[test]
fn comma_and_no_comma_produce_identical_dict() {
    assert_eq!(tree("{a: 1, b: 2}").unwrap(), tree("{a: 1 b: 2}").unwrap());
}

#[test]
fn string_key_dict() {
    assert_eq!(
        tree(r#"{"a": 1, "b": 2}"#).unwrap(),
        "[D{s(a): n(1), s(b): n(2)}]",
    );
}

#[test]
fn number_and_bool_keys_dict() {
    assert_eq!(
        tree("{1: a, true: b}").unwrap(),
        "[D{n(1): t(a), b(true): t(b)}]",
    );
}

#[test]
fn nested_dict_in_dict() {
    assert_eq!(tree("{a: {b: 1}}").unwrap(), "[D{t(a): D{t(b): n(1)}}]",);
}

#[test]
fn nested_list_in_dict() {
    assert_eq!(tree("{a: [1 2]}").unwrap(), "[D{t(a): L[n(1) n(2)]}]",);
}

#[test]
fn nested_dict_in_list() {
    assert_eq!(
        tree("[{a: 1} {b: 2}]").unwrap(),
        "[L[D{t(a): n(1)} D{t(b): n(2)}]]",
    );
}

#[test]
fn sub_expression_as_key() {
    assert_eq!(tree("{(name): 1}").unwrap(), "[D{[t(name)]: n(1)}]",);
}

#[test]
fn sub_expression_as_value() {
    assert_eq!(
        tree("{a: (LET y = 7)}").unwrap(),
        "[D{t(a): [t(LET) t(y) t(=) n(7)]}]",
    );
}

#[test]
fn trailing_comma_allowed() {
    assert_eq!(tree("{a: 1,}").unwrap(), "[D{t(a): n(1)}]");
}

#[test]
fn unbalanced_colon_errors() {
    assert!(tree("{a: 1: 2}").is_err());
}

#[test]
fn key_without_value_errors() {
    assert!(tree("{a:}").is_err());
}

#[test]
fn key_without_colon_errors() {
    assert!(tree("{a 1}").is_err());
}

#[test]
fn colon_outside_dict_with_space_errors() {
    // Outside a dict, `:` must be glued to its operand (`:Number`, `:(List ...)`).
    // String built from pieces so source-rewrite tooling can't migrate the colon away.
    let bad: String = format!("a{}{} Number", ':', "");
    assert!(tree(&bad).is_err());
}

#[test]
fn glued_colon_outside_dict_emits_type() {
    assert_eq!(tree("a :Number").unwrap(), "[t(a) T(Number)]");
}

#[test]
fn comma_in_expression_is_whitespace() {
    assert_eq!(tree("a, b").unwrap(), tree("a b").unwrap());
    assert_eq!(tree("(a,, b)").unwrap(), tree("(a b)").unwrap());
    assert_eq!(
        tree("(a :Number, b :Str)").unwrap(),
        tree("(a :Number b :Str)").unwrap()
    );
}

#[test]
fn unclosed_dict_errors() {
    assert!(tree("{a = 1").is_err());
}

#[test]
fn close_brace_without_open_errors() {
    assert!(tree("a}").is_err());
}

#[test]
fn open_brace_glued_to_token_errors() {
    assert!(tree("foo{a: 1}").is_err());
}

#[test]
fn close_brace_glued_to_token_errors() {
    assert!(tree("{a: 1}bar").is_err());
}

#[test]
fn multiline_dict_via_top_level_pipeline() {
    // Multi-line continuation lives in `collapse_whitespace`, which `tree` skips — use `top`.
    assert_eq!(
        top("LET d = {\n  a: 1\n  b: 2\n}").unwrap(),
        vec!["[t(LET) t(d) t(=) D{t(a): n(1), t(b): n(2)}]"],
    );
}

// --- Record literal tests (`=` pairs; `:` pairs stay a dict) ---

#[test]
fn single_field_record() {
    assert_eq!(tree("{x = 1}").unwrap(), "[R{x = n(1)}]");
}

#[test]
fn two_fields_with_comma() {
    assert_eq!(
        tree(r#"{x = 1, y = "a"}"#).unwrap(),
        "[R{x = n(1), y = s(a)}]",
    );
}

#[test]
fn two_fields_without_comma_auto_commit() {
    assert_eq!(tree("{x = 1 y = 2}").unwrap(), "[R{x = n(1), y = n(2)}]");
}

#[test]
fn record_comma_and_no_comma_match() {
    assert_eq!(
        tree("{x = 1, y = 2}").unwrap(),
        tree("{x = 1 y = 2}").unwrap()
    );
}

#[test]
fn record_value_as_sub_expression() {
    assert_eq!(
        tree("{x = (LET y = 7)}").unwrap(),
        "[R{x = [t(LET) t(y) t(=) n(7)]}]",
    );
}

#[test]
fn nested_record_in_record() {
    assert_eq!(tree("{a = {b = 1}}").unwrap(), "[R{a = R{b = n(1)}}]");
}

#[test]
fn record_trailing_comma_allowed() {
    assert_eq!(tree("{x = 1,}").unwrap(), "[R{x = n(1)}]");
}

#[test]
fn empty_braces_stay_dict() {
    // No separator to disambiguate → dict; an empty record needs a `:{}` ascription.
    assert_eq!(tree("{}").unwrap(), "[D{}]");
}

#[test]
fn mixed_record_then_dict_delimiters_errors() {
    let err = tree("{x = 1, y: 2}").unwrap_err();
    assert!(
        err.contains("mixed"),
        "expected mixed-delimiter error, got: {err}"
    );
}

#[test]
fn mixed_dict_then_record_delimiters_errors() {
    assert!(tree("{x: 1, y = 2}").is_err());
}

#[test]
fn record_field_without_value_errors() {
    assert!(tree("{x =}").is_err());
}

#[test]
fn non_identifier_record_field_errors() {
    let err = tree("{1 = 2}").unwrap_err();
    assert!(err.contains("bare identifier"), "got: {err}");
}
