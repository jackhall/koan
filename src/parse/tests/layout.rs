//! The layout rules a rendered tree cannot state: where a continuation regime refuses a break,
//! and where one regime overrides another. That a layout choice never changes the tree is
//! [`properties`](super::properties)' first law; what is left here is the surface saying no.

use super::top;

// --- Paren continuation across line breaks ---

#[test]
fn open_paren_same_indent_break_is_error() {
    let error = top("PRINT (\n3.14\n)").unwrap_err();
    assert!(error.contains("unmatched '('"), "got: {error}");
}

#[test]
fn close_paren_below_opener_indent_is_error() {
    let error = top("foo\n  PRINT (\n    3.14\n)").unwrap_err();
    assert!(error.contains("less indented"), "got: {error}");
}

#[test]
fn comma_continuation_overrides_paren_guard() {
    assert_eq!(
        top("PRINT (,\n3.14,\n)").unwrap(),
        vec!["[t(PRINT) [n(3.14)]]"]
    );
}

// --- Sigil-led lines ---

#[test]
fn comma_continuation_with_bare_sigil() {
    let error = top("add 1,\n  #2").unwrap_err();
    assert!(error.contains("expected '(' after '#'"), "got: {error}");
}

#[test]
fn bracket_continuation_with_bare_sigil() {
    let error = top("LET xs = [\n  #3\n]").unwrap_err();
    assert!(error.contains("expected '(' after '#'"), "got: {error}");
}
