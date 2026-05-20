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
    assert_eq!(
        tree(r#"[a "hi" 4]"#).unwrap(),
        "[L[t(a) s(hi) n(4)]]",
    );
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
    // Sub-expressions inside list literals stay as Expression elements; the scheduler is
    // responsible for resolving them at runtime via the Combine node path.
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
    // `[1 2)` opens a List frame then hits `)` before a `]` — the bracket was never
    // closed. The diagnostic reports the unclosed `[`, not an internal frame mismatch.
    let err = tree("[1 2)").unwrap_err();
    assert!(
        err.contains("unclosed '['"),
        "expected unclosed-'[' diagnostic, got: {err}",
    );
}

#[test]
fn close_paren_when_innermost_is_dict_errors() {
    // Symmetric to the list case for `{a: 1)`.
    let err = tree("{a: 1)").unwrap_err();
    assert!(
        err.contains("unclosed '{'"),
        "expected unclosed-'{{' diagnostic, got: {err}",
    );
}

#[test]
fn open_bracket_glued_to_token_errors() {
    // List literals must stand alone — `foo[2]` is no longer valid (was compound
    // indexing). The user must write `foo [2]` if they actually want a sibling list.
    assert!(tree("foo[2]").is_err());
}

#[test]
fn close_bracket_glued_to_token_errors() {
    assert!(tree("[1 2]bar").is_err());
}

#[test]
fn open_bracket_glued_to_close_paren_errors() {
    // `(x)[2]` is also forbidden: the result of a paren-expression can't be glued to a
    // list literal.
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
    // Commas inside a list act as whitespace.
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
    // `[[1 2]]` is two `[` then two `]` — each `[` is preceded by `(` or `[`, and each
    // `]` is followed by `]` or `)`. All adjacency rules satisfied.
    assert_eq!(tree("[[1 2]]").unwrap(), "[L[L[n(1) n(2)]]]");
}

// --- Dict literal tests ---

#[test]
fn empty_dict_literal() {
    assert_eq!(tree("{}").unwrap(), "[D{}]");
}

#[test]
fn two_pairs_with_comma() {
    assert_eq!(
        tree("{a: 1, b: 2}").unwrap(),
        "[D{t(a): n(1), t(b): n(2)}]",
    );
}

#[test]
fn two_pairs_without_comma() {
    // Auto-commit rule: `b` arriving while value=[1] commits the prior pair.
    assert_eq!(
        tree("{a: 1 b: 2}").unwrap(),
        "[D{t(a): n(1), t(b): n(2)}]",
    );
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
    assert_eq!(
        tree("{a: {b: 1}}").unwrap(),
        "[D{t(a): D{t(b): n(1)}}]",
    );
}

#[test]
fn nested_list_in_dict() {
    assert_eq!(
        tree("{a: [1 2]}").unwrap(),
        "[D{t(a): L[n(1) n(2)]}]",
    );
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
    assert_eq!(
        tree("{(name): 1}").unwrap(),
        "[D{[t(name)]: n(1)}]",
    );
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
    // Second `:` inside the same value position is rejected.
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
    // `: ` outside a dict frame is a parse error under the type-sigil regime — the colon
    // must be glued to its operand (`:Number` for bare, `:(List ...)` for parameterized).
    // The string below is built from pieces so source-rewrite tooling can't migrate the
    // colon away — the point of the test is precisely the bad-glue form.
    let bad: String = format!("a{}{} Number", ':', "");
    assert!(tree(&bad).is_err());
}

#[test]
fn glued_colon_outside_dict_emits_type() {
    // Glued `:T` produces an `ExpressionPart::Type` directly, no `Keyword(":")` in between.
    assert_eq!(tree("a :Number").unwrap(), "[t(a) T(Number)]");
}

#[test]
fn comma_in_expression_is_whitespace() {
    // `,` inside an expression frame is a no-op — same parsed shape as whitespace.
    // Lets future named-argument parameter lists use commas as visual separators without
    // affecting the tree.
    assert_eq!(tree("a, b").unwrap(), tree("a b").unwrap());
    assert_eq!(tree("(a,, b)").unwrap(), tree("(a b)").unwrap());
    assert_eq!(tree("(a :Number, b :Str)").unwrap(), tree("(a :Number b :Str)").unwrap());
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
    // Multi-line dict goes through the full `parse` pipeline since `collapse_whitespace`
    // is the part that handles continuation. `tree` skips that step so we use `top`.
    assert_eq!(
        top("LET d = {\n  a: 1\n  b: 2\n}").unwrap(),
        vec!["[t(LET) t(d) t(=) D{t(a): n(1), t(b): n(2)}]"],
    );
}

// --- Parameterized type tests (Design-B `:(...)` sigil) ---
