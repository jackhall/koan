//! What a `:` refuses, and the one place a bare `(…)` reads as a type expression anyway.
//!
//! The parser does no shape-folding inside `:(...)`: every sigil emits
//! `ExpressionPart::SigiledTypeExpr(inner)` whose inner mirrors the parens contents, and its
//! idempotence is [`properties`](super::properties)' ninth law.
//!
//! The flip below is the exception the sigil rules make for a *binder form*: in that form's type
//! slots a plain `(…)` is rewritten to `SigiledTypeExpr`, so the two spellings are one parts run.
//! It is keyed on the whole bucket key, so no other run takes it.

use super::tree;

/// An operator is a word like any other, so whitespace is what delimits it: `a<b` is one atom,
/// and no identifier may hold a `<`.
#[test]
fn an_operator_glued_to_its_operands_is_one_invalid_atom() {
    assert!(tree("a<b").is_err());
    assert!(tree("a>b").is_err());
    assert!(tree("a<=b").is_err());
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
fn unclosed_record_type_sigil_errors() {
    assert!(tree(":{x :Number").is_err());
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

// --- The bare parenthesized type spelling, in a binder form's type slots ---

/// The bare parenthesized type spelling: in a binder form's **type slots** the parser rewrites a
/// plain `(…)` to `SigiledTypeExpr`, so `(LIST OF Str)` ≡ `:(LIST OF Str)` there and everything
/// downstream sees the one part kind. Every other slot of the same statement — the signature at
/// index 1, the body at index 5 — keeps its `(…)`.
#[test]
fn a_binder_forms_type_slot_admits_the_bare_parenthesized_spelling() {
    use super::top;
    assert_eq!(
        top("EXPR (WRAP s :Str) -> (LIST OF Str) = (s)").unwrap(),
        ["[t(EXPR) [t(WRAP) t(s) T(Str)] t(->) :(t(LIST) t(OF) T(Str)) t(=) [t(s)]]"],
    );
    assert_eq!(
        top("EXPR (WRAP s :Str) -> (LIST OF Str) = (s)").unwrap(),
        top("EXPR (WRAP s :Str) -> :(LIST OF Str) = (s)").unwrap(),
        "the two spellings are the same parts run",
    );
}

/// The flip is keyed on the whole binder key, not on the paren shape: a run spelling the same
/// arity and the same `-> … = …` keywords under a different head is an ordinary call, and every
/// one of its `(…)` slots stays code.
#[test]
fn a_non_binder_run_with_the_same_shape_does_not_flip() {
    use super::top;
    assert_eq!(
        top("MYFORM (a) -> (LIST OF Str) = (b)").unwrap(),
        ["[t(MYFORM) [t(a)] t(->) [t(LIST) t(OF) T(Str)] t(=) [t(b)]]"],
    );
}

/// Parse normalization applies uniformly inside a `#(…)` quote and a `$(…)` body, as suffix
/// folding already does — so a quoted definition evaluated later reads identically to its sigiled
/// spelling.
#[test]
fn the_flip_reaches_quote_and_eval_bodies() {
    use super::top;
    assert_eq!(
        top("#(EXPR (WRAP s :Str) -> (LIST OF Str) = (s))").unwrap(),
        top("#(EXPR (WRAP s :Str) -> :(LIST OF Str) = (s))").unwrap(),
    );
    assert!(
        top("#(EXPR (WRAP s :Str) -> (LIST OF Str) = (s))").unwrap()[0]
            .contains(":(t(LIST) t(OF) T(Str))"),
        "a quote body's type slot takes the flip",
    );
    assert_eq!(
        top("$(EXPR (WRAP s :Str) -> (LIST OF Str) = (s))").unwrap(),
        top("$(EXPR (WRAP s :Str) -> :(LIST OF Str) = (s))").unwrap(),
    );
}
