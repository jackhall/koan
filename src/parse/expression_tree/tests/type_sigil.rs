//! `type_sigil` parse cases for `expression_tree::parse`.
//!
//! See [type-language-via-dispatch](../../../../design/typing/type-language-via-dispatch.md).
//! The parser does no shape-folding inside `:(...)`: every sigil emits
//! `ExpressionPart::SigiledTypeExpr(inner)` whose inner `KExpression`'s parts mirror
//! whatever appeared between the parens (leaf Types, keywords like `->`, nested
//! parens, etc.). Shape recognition (positional `:(List Number)` →
//! `TypeConstructorCall` arm; keyworded `:(LIST OF Number)` → `Keyworded` arm;
//! user-functor `:(MyFunctor (T = IntOrd))` → `FunctionValueCall` arm) is the
//! dispatcher's job. These tests only assert what the parser produces — they do
//! not run dispatch.

use super::tree;

#[test]
fn type_with_one_param() {
    // `:(List Number)` — head Type + one Type arg, wrapped in SigiledTypeExpr.
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
    // Whitespace-only separation is also legal — `,` is a no-op inside expression frames.
    assert_eq!(
        tree(":(Dict String Number)").unwrap(),
        tree(":(Dict String Number)").unwrap(),
    );
}

#[test]
fn type_nested_two_levels() {
    // `:(List :(Dict String Number))` — inner sigil is also a SigiledTypeExpr part.
    assert_eq!(
        tree(":(List :(Dict String Number))").unwrap(),
        "[:(T(List) :(T(Dict) T(String) T(Number)))]"
    );
}

#[test]
fn function_type_unary() {
    // The parser does no folding inside `:(...)` — the `Function` head, the args
    // expression, the `->` keyword, and the return Type are all sibling parts.
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
    // Inside the `(...)` arg group, commas are no-ops just like elsewhere.
    assert_eq!(
        tree(":(Function (Number Bool) -> Number)").unwrap(),
        tree(":(Function (Number Bool) -> Number)").unwrap(),
    );
}

#[test]
fn function_type_arg_nested_parameterized() {
    // Args themselves can be parameterized types via nested sigils — the inner
    // SigiledTypeExpr lives as a part inside the args expression.
    assert_eq!(
        tree(":(Function (:(List Number) Str) -> Bool)").unwrap(),
        "[:(T(Function) [:(T(List) T(Number)) T(Str)] t(->) T(Bool))]"
    );
}

#[test]
fn lt_after_non_type_with_whitespace_emits_keyword() {
    // `<` not preceded by a Type part — and properly whitespace-separated — emits a
    // standalone keyword, available for a future less-than builtin to dispatch on.
    assert_eq!(tree("a < b").unwrap(), "[t(a) t(<) t(b)]");
}

#[test]
fn lt_glued_to_non_type_no_longer_special() {
    // Under Design B the `<` / `>` characters carry no type-position meaning, so glued
    // forms like `a<b` lex into separate tokens `a`, `<`, `b` with no glue error.
    assert_eq!(tree("a<b").unwrap(), "[t(a) t(<) t(b)]");
}

#[test]
fn gt_lt_outside_type_emit_keywords() {
    // `<` and `>` always emit standalone keywords now — whitespace-glued or not. The
    // `prev=='-'` rule still keeps `->` contiguous so `a -> b` continues to tokenize as
    // one keyword.
    assert_eq!(tree("a > b").unwrap(), "[t(a) t(>) t(b)]");
    assert_eq!(tree("Number > 0").unwrap(), "[T(Number) t(>) n(0)]");
    assert_eq!(tree("a -> b").unwrap(), "[t(a) t(->) t(b)]");
    assert_eq!(tree("a>b").unwrap(), "[t(a) t(>) t(b)]");
}

#[test]
fn type_token_with_invalid_char_errors() {
    // The token classifier rejects non-alphanumeric chars inside a type name.
    assert!(tree("Foo$Bar").is_err());
    assert!(tree("Foo+Bar").is_err());
}

#[test]
fn unclosed_type_sigil_errors() {
    // `:(List Number` (no close paren) leaves the TypeExpr frame open at EOF.
    assert!(tree(":(List Number").is_err());
}

#[test]
fn comma_outside_type_sigil_unchanged_inside_paren() {
    // Sanity: signatures like `(xs :(List Number), ys :(List Str))` still parse — the
    // comma is a no-op inside the expression frame.
    assert_eq!(
        tree("(xs :(List Number), ys :(List Str))").unwrap(),
        "[[t(xs) :(T(List) T(Number)) t(ys) :(T(List) T(Str))]]",
    );
}

// --- Type-sigil basics (Design-B `:(...)` and `:T`) ---

#[test]
fn type_sigil_bare_emits_type_part() {
    // `:Number` consumes the `:` and emits the bare Type token as a single Type part.
    assert_eq!(tree("LET x :Number = 5").unwrap(), "[t(LET) t(x) T(Number) t(=) n(5)]");
}

#[test]
fn type_sigil_parameterized_list() {
    // `:(List Number)` opens a SigiledTypeExpr frame; close wraps the inner
    // [Type(List), Type(Number)] expression in `SigiledTypeExpr`.
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
    // No folding — the parser keeps the inner parens as an Expression part.
    assert_eq!(
        tree("LET d :(Dict Str (List Number))").unwrap(),
        "[t(LET) t(d) :(T(Dict) T(Str) [T(List) T(Number)])]",
    );
}

#[test]
fn type_sigil_let_type_binding_rhs() {
    // `LET t = :(List Number)` is unambiguously a type binding because the RHS is
    // sigil-prefixed.
    assert_eq!(
        tree("LET t = :(List Number)").unwrap(),
        "[t(LET) t(t) t(=) :(T(List) T(Number))]",
    );
}

#[test]
fn type_sigil_lone_colon_with_eof_errors() {
    // Trailing `:` at end of input.
    assert!(tree("LET x :").is_err());
}

#[test]
fn type_sigil_lone_colon_glued_to_lowercase_errors() {
    // `:` followed by a lowercase identifier is a parse error — type sigils require
    // an uppercase head OR `(`.
    assert!(tree("LET x :foo").is_err());
}

#[test]
fn type_sigil_empty_parens_parses() {
    // `:()` opens a SigiledTypeExpr frame with zero parts — the parser no longer
    // rejects this shape; the dispatcher will surface the empty-expression error
    // when it tries to run the inner KExpression.
    assert_eq!(tree("LET x :()").unwrap(), "[t(LET) t(x) :()]");
}

#[test]
fn type_sigil_bare_type_name_in_parens() {
    // `:(Number)` — single bare Type inside the sigil. The parser's redundant-Expression
    // peel does NOT apply inside `:(...)` — the sigil is the wrapper.
    assert_eq!(tree("LET x :(Number)").unwrap(), "[t(LET) t(x) :(T(Number))]");
}

#[test]
fn type_sigil_function_nested_arg_unparameterized() {
    // `:(Function ((List Number)) -> Bool)` — the inner `(List Number)` is a nested
    // Expression inside the args group. No folding.
    assert_eq!(
        tree("LET f :(Function ((List Number)) -> Bool)").unwrap(),
        "[t(LET) t(f) :(T(Function) [[T(List) T(Number)]] t(->) T(Bool))]",
    );
}

#[test]
fn type_sigil_function_return_parenthesized() {
    // `:(Function (Number) -> (List Str))` — the return slot is a parenthesized
    // sub-expression. No folding.
    assert_eq!(
        tree("LET f :(Function (Number) -> (List Str))").unwrap(),
        "[t(LET) t(f) :(T(Function) [T(Number)] t(->) [T(List) T(Str)])]",
    );
}

// --- Sigil tests (`#(...)` quote, `$(...)` eval) ---
