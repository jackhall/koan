//! Unit tests for the parse module. Each test parses a source snippet through
//! `expression_tree::parse` and compares the result against an expected shape string
//! produced by the local `describe` helper, which renders an `ExpressionPart` tree as a
//! compact `t(...)` / `T(...)` / `e(...)` notation — terser to read and diff than the
//! full `KExpression` debug output.

use super::expression_tree::{build_tree, parse};
use crate::parse::kexpression::{ExpressionPart, KExpression, KLiteral};
use crate::parse::quotes::mask_quotes;

fn describe(e: &KExpression<'_>) -> String {
    fn describe_part(p: &ExpressionPart<'_>) -> String {
        match p {
            ExpressionPart::Keyword(s) => format!("t({})", s),
            ExpressionPart::Identifier(s) => format!("t({})", s),
            ExpressionPart::Type(s) => format!("T({})", s),
            ExpressionPart::Expression(e) => describe(e),
            ExpressionPart::ListLiteral(items) => {
                let inner: Vec<String> = items.iter().map(describe_part).collect();
                format!("L[{}]", inner.join(" "))
            }
            ExpressionPart::DictLiteral(pairs) => {
                let inner: Vec<String> = pairs
                    .iter()
                    .map(|(k, v)| format!("{}: {}", describe_part(k), describe_part(v)))
                    .collect();
                format!("D{{{}}}", inner.join(", "))
            }
            ExpressionPart::Literal(KLiteral::String(s)) => format!("s({})", s),
            ExpressionPart::Literal(KLiteral::Number(n)) => format!("n({})", n),
            ExpressionPart::Literal(KLiteral::Boolean(b)) => format!("b({})", b),
            ExpressionPart::Literal(KLiteral::Null) => "null".to_string(),
            ExpressionPart::Future(_) => "future".to_string(),
        }
    }
    let parts: Vec<String> = e.parts.iter().map(describe_part).collect();
    format!("[{}]", parts.join(" "))
}

fn tree(input: &str) -> Result<String, String> {
    let (masked, dict) = mask_quotes(input);
    build_tree(&masked, &dict).map(|e| describe(&e))
}

fn top(input: &str) -> Result<Vec<String>, String> {
    parse(input).map(|exprs| exprs.iter().map(describe).collect())
}

#[test]
fn parse_single_line_has_no_top_level_wrapper() {
    assert_eq!(top("foo bar").unwrap(), vec!["[t(foo) t(bar)]"]);
}

#[test]
fn parse_multiple_lines_are_siblings() {
    assert_eq!(top("foo\nbar").unwrap(), vec!["[t(foo)]", "[t(bar)]"]);
}

#[test]
fn parse_peels_top_level_redundant_parens() {
    assert_eq!(top("(foo bar)").unwrap(), top("foo bar").unwrap());
}

#[test]
fn parse_peels_multiple_redundant_layers() {
    assert_eq!(top("(((foo bar)))").unwrap(), vec!["[t(foo) t(bar)]"]);
}

#[test]
fn parse_peels_redundant_wrappers_inside_subexpressions() {
    // The inner `((bar baz))` collapses to `(bar baz)` — a sub-expression with one
    // wrapping layer, not two — so peel doesn't change argument arity.
    assert_eq!(
        top("foo ((bar baz))").unwrap(),
        top("foo (bar baz)").unwrap(),
    );
}

#[test]
fn parse_keeps_meaningful_subexpression_parens() {
    // A single set of parens around an argument is meaningful structure, not redundancy.
    assert_eq!(
        top("foo (bar baz)").unwrap(),
        vec!["[t(foo) [t(bar) t(baz)]]"],
    );
}

#[test]
fn empty_input() {
    assert_eq!(tree("").unwrap(), "[]");
}

#[test]
fn single_token() {
    assert_eq!(tree("foo").unwrap(), "[t(foo)]");
}

#[test]
fn split_on_whitespace() {
    assert_eq!(tree("hi there").unwrap(), "[t(hi) t(there)]");
}

#[test]
fn runs_of_whitespace_collapse() {
    assert_eq!(tree("  hi   there  ").unwrap(), "[t(hi) t(there)]");
}

#[test]
fn empty_parens() {
    assert_eq!(tree("()").unwrap(), "[[]]");
}

#[test]
fn flat_parens() {
    assert_eq!(tree("(hi there)").unwrap(), "[[t(hi) t(there)]]");
}

#[test]
fn siblings_and_groups() {
    assert_eq!(
        tree("hey (whoever you are) look at").unwrap(),
        "[t(hey) [t(whoever) t(you) t(are)] t(look) t(at)]"
    );
}

#[test]
fn two_paren_groups() {
    assert_eq!(
        tree("hey (whoever you are) look at (that over there)").unwrap(),
        "[t(hey) [t(whoever) t(you) t(are)] t(look) t(at) [t(that) t(over) t(there)]]"
    );
}

#[test]
fn nested_parens() {
    assert_eq!(
        tree("hey (whoever you are) look at (whatever (that over there) is)").unwrap(),
        "[t(hey) [t(whoever) t(you) t(are)] t(look) t(at) [t(whatever) [t(that) t(over) t(there)] t(is)]]"
    );
}

#[test]
fn adjacent_paren_groups() {
    assert_eq!(
        tree("hey (whoever you are)(hello in this language)").unwrap(),
        "[t(hey) [t(whoever) t(you) t(are)] [t(hello) t(in) t(this) t(language)]]"
    );
}

#[test]
fn deeply_nested() {
    assert_eq!(
        tree("hey (whoever (I think) you are (when I remember) now) look at").unwrap(),
        "[t(hey) [t(whoever) [t(I) t(think)] t(you) t(are) [t(when) t(I) t(remember)] t(now)] t(look) t(at)]"
    );
}

#[test]
fn string_literal() {
    assert_eq!(tree(r#"say "hello""#).unwrap(), "[t(say) s(hello)]");
}

#[test]
fn empty_string_literal() {
    assert_eq!(tree(r#""""#).unwrap(), "[s()]");
}

#[test]
fn literal_inside_parens() {
    assert_eq!(
        tree(r#"print ("hello" to 'world')"#).unwrap(),
        "[t(print) [s(hello) t(to) s(world)]]"
    );
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
    assert_eq!(
        tree("true false null").unwrap(),
        "[b(true) b(false) null]"
    );
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
fn close_without_open_errors() {
    assert!(tree(")(").is_err());
    assert!(tree("has closed) paren only").is_err());
    assert!(tree("two (closed one) open)").is_err());
}

#[test]
fn open_without_close_errors() {
    assert!(tree("has (open paren only").is_err());
    assert!(tree("(two (open one closed)").is_err());
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
    // responsible for resolving them at runtime via the Aggregate node path.
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
fn single_pair_dict() {
    assert_eq!(tree("{a: 1}").unwrap(), "[D{t(a): n(1)}]");
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
fn multi_part_value_in_parens() {
    assert_eq!(
        tree("{a: (foo bar)}").unwrap(),
        "[D{t(a): [t(foo) t(bar)]}]",
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
fn colon_outside_dict_emits_keyword() {
    // `:` outside a dict frame is the type-annotation separator and parses as
    // a standalone `Keyword(":")`. UNION schemas (and, eventually, function
    // signatures) consume the resulting `[Identifier, Keyword(":"), Type]` triples.
    assert_eq!(tree("a: Number").unwrap(), "[t(a) t(:) T(Number)]");
}

#[test]
fn comma_in_expression_is_whitespace() {
    // `,` inside an expression frame is a no-op — same parsed shape as whitespace.
    // Lets type-annotation triples and future function-signature parameter lists
    // use commas as visual separators without affecting the tree.
    assert_eq!(tree("a, b").unwrap(), tree("a b").unwrap());
    assert_eq!(tree("(a,, b)").unwrap(), tree("(a b)").unwrap());
    assert_eq!(tree("(a: Number, b: Str)").unwrap(), tree("(a: Number b: Str)").unwrap());
}

#[test]
fn unclosed_dict_errors() {
    assert!(tree("{a: 1").is_err());
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
fn multi_part_value_without_parens_errors() {
    // `{a: foo bar}` parses key=`a`, value=`foo`, auto-commits — then `bar` starts a new
    // key and `}` closes with that key unterminated. The constraint is intentional:
    // dict values are single-token unless parenthesized, mirroring list elements.
    assert!(tree("{a: foo bar}").is_err());
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
