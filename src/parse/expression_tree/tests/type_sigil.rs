//! `type_sigil` parse cases for `expression_tree::parse`.


use super::tree;

#[test]
fn type_with_one_param() {
    // `List<Number>` parses as one Type part with one nested param. The describe helper
    // renders TypeExpr via `render()`, so the structural distinction is visible.
    assert_eq!(tree(":(List Number)").unwrap(), "[T(:(List Number))]");
}

#[test]
fn type_with_two_params() {
    assert_eq!(
        tree(":(Dict String Number)").unwrap(),
        "[T(:(Dict String Number))]"
    );
}

#[test]
fn type_with_two_params_no_comma() {
    // Whitespace-only separation is also legal — `,` is a no-op inside expression frames,
    // and the same precedent applies inside TypeFrames.
    assert_eq!(
        tree(":(Dict String Number)").unwrap(),
        tree(":(Dict String Number)").unwrap(),
    );
}

#[test]
fn type_nested_two_levels() {
    assert_eq!(
        tree(":(List :(Dict String Number))").unwrap(),
        "[T(:(List :(Dict String Number)))]"
    );
}

#[test]
fn function_type_unary() {
    // Function args are always parenthesized — `Function<(arg) -> ret>` for one arg,
    // `Function<() -> ret>` for nullary.
    assert_eq!(
        tree(":(Function (Number) -> Str)").unwrap(),
        "[T(:(Function (Number) -> Str))]"
    );
}

#[test]
fn function_type_nullary() {
    assert_eq!(
        tree(":(Function () -> Number)").unwrap(),
        "[T(:(Function () -> Number))]"
    );
}

#[test]
fn function_type_multi_arg() {
    assert_eq!(
        tree(":(Function (Number Bool) -> Number)").unwrap(),
        "[T(:(Function (Number Bool) -> Number))]"
    );
}

#[test]
fn function_type_multi_arg_no_comma() {
    // Inside the `(...)` arg group, commas are no-ops just like elsewhere in expression
    // frames — whitespace alone separates args.
    assert_eq!(
        tree(":(Function (Number Bool) -> Number)").unwrap(),
        tree(":(Function (Number Bool) -> Number)").unwrap(),
    );
}

#[test]
fn function_type_bare_arrow_no_parens_errors() {
    // `:(Function -> R)` (no args at all) is rejected — the user must use the explicit
    // `()` for nullary.
    assert!(tree(":(Function -> Number)").is_err());
}

#[test]
fn function_type_unparenthesized_args_errors() {
    // `:(Function A -> R)` (no parens around the args) is rejected so the syntax stays
    // uniform: args are ALWAYS parenthesized, even for the single-arg case.
    assert!(tree(":(Function Number -> Str)").is_err());
    assert!(tree(":(Function Number Bool -> Str)").is_err());
}

#[test]
fn function_type_arg_nested_parameterized() {
    // New sigil form: args themselves can be parameterized types. The inner sigil
    // closes before the outer Function frame's args expression. Render uses Design-B
    // sigil syntax, so the parsed-and-rendered shape round-trips.
    assert_eq!(
        tree(":(Function (:(List Number) Str) -> Bool)").unwrap(),
        "[T(:(Function (:(List Number) Str) -> Bool))]"
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
    // forms like `a<b` lex into separate tokens `a`, `<`, `b` with no glue error. The
    // freed-up syntax is reserved for future numeric-comparison operators.
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
fn function_arrow_in_non_function_type_errors() {
    // `->` is exclusive to the `Function` head — other parameterized types must reject.
    assert!(tree(":(List Number -> Str)").is_err());
}

#[test]
fn double_arrow_in_function_type_errors() {
    // Two `->`s inside one Function sigil — `find_arrow` rejects on the second arrow
    // before the function-builder ever runs. Use multi-letter type names so the tokens
    // classify as Types (single uppercase letters fail token classification first).
    let err = tree(":(Function Num -> Str -> Bool)").unwrap_err();
    assert!(
        err.contains("more than one `->` arrow"),
        "expected double-arrow diagnostic, got: {err}",
    );
}

#[test]
fn list_with_whitespace_then_lt_is_three_tokens() {
    // Under Design B `<` carries no type meaning, so `List <Number>` parses as three
    // tokens: Type, `<` keyword, Type — not the legacy single TypeFrame.
    assert_eq!(
        tree("List <Number>").unwrap(),
        "[T(List) t(<) T(Number) t(>)]",
    );
}

#[test]
fn comma_outside_type_sigil_unchanged_inside_paren() {
    // Sanity: signatures like `(xs :(List Number), ys :(List Str))` still parse — the
    // comma is a no-op inside the expression frame, and `)` followed by `,` then `ys`
    // continues normally.
    assert_eq!(
        tree("(xs :(List Number), ys :(List Str))").unwrap(),
        "[[t(xs) T(:(List Number)) t(ys) T(:(List Str))]]",
    );
}

// --- Type-sigil validation (Design-B `:(...)` and `:T`) ---

#[test]
fn type_sigil_bare_emits_type_part() {
    // `:Number` consumes the `:` and emits the bare Type token as a single Type part.
    assert_eq!(tree("LET x :Number = 5").unwrap(), "[t(LET) t(x) T(Number) t(=) n(5)]");
}

#[test]
fn type_sigil_parameterized_list() {
    // `:(List Number)` opens a TypeExpr frame; close folds into a parameterized Type.
    assert_eq!(tree("LET ns :(List Number)").unwrap(), "[t(LET) t(ns) T(:(List Number))]");
}

#[test]
fn type_sigil_function_nullary() {
    assert_eq!(
        tree("LET f :(Function () -> Str)").unwrap(),
        "[t(LET) t(f) T(:(Function () -> Str))]",
    );
}

#[test]
fn type_sigil_function_unary() {
    assert_eq!(
        tree("LET f :(Function (Number) -> Str)").unwrap(),
        "[t(LET) t(f) T(:(Function (Number) -> Str))]",
    );
}

#[test]
fn type_sigil_function_multi_arg() {
    assert_eq!(
        tree("LET f :(Function (Number Str) -> Bool)").unwrap(),
        "[t(LET) t(f) T(:(Function (Number Str) -> Bool))]",
    );
}

#[test]
fn type_sigil_nested_dict_of_list() {
    assert_eq!(
        tree("LET d :(Dict Str (List Number))").unwrap(),
        "[t(LET) t(d) T(:(Dict Str :(List Number)))]",
    );
}

#[test]
fn type_sigil_let_type_binding_rhs() {
    // `LET t = :(List Number)` is unambiguously a type binding because the RHS is
    // sigil-prefixed.
    assert_eq!(
        tree("LET t = :(List Number)").unwrap(),
        "[t(LET) t(t) t(=) T(:(List Number))]",
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
    // an uppercase head.
    assert!(tree("LET x :foo").is_err());
}

#[test]
fn type_sigil_empty_parens_errors() {
    // `:()` opens a TypeExpr frame with zero parts — `build` rejects with the
    // empty-type-expression diagnostic.
    let err = tree("LET x :()").unwrap_err();
    assert!(
        err.contains("empty `:(...)` type expression"),
        "expected empty-type-expression diagnostic, got: {err}",
    );
}

#[test]
fn type_sigil_parameterized_head_errors() {
    // Head must be a bare type name. `:(:(List Number) Foo)` puts a fully-parsed
    // parameterized type in the head slot — `build` rejects via the
    // `Type(t) if matches!(t.params, TypeParams::None)` guard.
    let err = tree("LET x :(:(List Number) Foo)").unwrap_err();
    assert!(
        err.contains("type-expression head must be a bare type name"),
        "expected bare-type-name diagnostic, got: {err}",
    );
}

#[test]
fn type_sigil_non_type_head_errors() {
    // `:(Number)` is fine — a bare uppercase token classifies as a Type. To exercise
    // the non-Type head arm we need a non-Type first part; the literal `5` is a
    // Literal part.
    let err = tree("LET x :(5)").unwrap_err();
    assert!(
        err.contains("type-expression head must be a type name"),
        "expected type-name head diagnostic, got: {err}",
    );
}

#[test]
fn type_sigil_bare_type_name_in_parens() {
    // `:(Number)` — single bare Type, no params. Exercises the `rest.is_empty()`
    // early-return in `build_list_params` (TypeParams::None).
    assert_eq!(tree("LET x :(Number)").unwrap(), "[t(LET) t(x) T(Number)]");
}

#[test]
fn type_sigil_function_without_arrow_errors() {
    // `:(Function (Number))` — no `->` arrow. `build` matches `(None, true)` and
    // rejects with the arrow-required diagnostic.
    let err = tree("LET f :(Function (Number))").unwrap_err();
    assert!(
        err.contains("requires `->`"),
        "expected arrow-required diagnostic, got: {err}",
    );
}

#[test]
fn type_sigil_non_type_param_errors() {
    // `:(List 5)` — a Literal param flows into `find_arrow`'s walk and trips the
    // catch-all rejection (not a Type, not an Expression, not the `->` arrow).
    let err = tree("LET x :(List 5)").unwrap_err();
    assert!(
        err.contains("parameter must be a type name"),
        "expected non-type-param diagnostic, got: {err}",
    );
}

#[test]
fn type_sigil_function_arg_non_type_errors() {
    // Inside the `(...)` arg group, args are walked by `extract_function_args`.
    // A literal arg like `5` triggers the function-arg non-type diagnostic
    // (distinct from the find_arrow rejection, which can't see inside the inner paren).
    let err = tree("LET f :(Function (5) -> Bool)").unwrap_err();
    assert!(
        err.contains("arg must be a type name"),
        "expected function-arg non-type diagnostic, got: {err}",
    );
}

#[test]
fn type_sigil_function_nested_arg_unparameterized() {
    // `:(Function ((List Number)) -> Bool)` — the inner `(List Number)` is itself a
    // parenthesized type expression without the sigil. `extract_function_args`
    // recurses through a fresh `TypeExprFrame::build` for the Expression arm.
    assert_eq!(
        tree("LET f :(Function ((List Number)) -> Bool)").unwrap(),
        "[t(LET) t(f) T(:(Function (:(List Number)) -> Bool))]",
    );
}

#[test]
fn type_sigil_function_return_wrong_arity_errors() {
    // `:(Function () -> A B)` — two parts after the arrow. The `[only] = try_from`
    // pattern in `extract_function_return` rejects with the arity diagnostic.
    let err = tree("LET f :(Function () -> Number Bool)").unwrap_err();
    assert!(
        err.contains("exactly one return type"),
        "expected return-arity diagnostic, got: {err}",
    );
}

#[test]
fn type_sigil_function_return_parenthesized() {
    // `:(Function (Number) -> (List Str))` — the return slot is a parenthesized type
    // expression. `extract_function_return` recurses through `TypeExprFrame::build`
    // for the Expression arm.
    assert_eq!(
        tree("LET f :(Function (Number) -> (List Str))").unwrap(),
        "[t(LET) t(f) T(:(Function (Number) -> :(List Str)))]",
    );
}

// --- Sigil tests (`#(...)` quote, `$(...)` eval) ---
