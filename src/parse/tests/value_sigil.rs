//! `value_sigil` parse cases for `parse`.

use super::{top, tree};

#[test]
fn quote_sigil_without_paren_errors() {
    // Sigil surface is paren-only.
    assert!(tree("#foo").is_err());
}

#[test]
fn eval_sigil_without_paren_errors() {
    assert!(tree("$x").is_err());
}

#[test]
fn quote_sigil_with_whitespace_before_paren_errors() {
    // Whitespace breaks the contiguity rule.
    assert!(tree("# (foo)").is_err());
}

#[test]
fn quote_sigil_followed_by_number_errors() {
    assert!(tree("#42").is_err());
}

#[test]
fn quote_sigil_followed_by_close_brace_errors() {
    assert!(tree("#}").is_err());
}

#[test]
fn double_sigil_errors() {
    assert!(tree("#$x").is_err());
    assert!(tree("#$(x)").is_err());
}

#[test]
fn trailing_sigil_at_end_of_input_errors() {
    assert!(tree("#").is_err());
    assert!(tree("$").is_err());
}

/// A bare `#2` only parses where the indent collapse rewrites it to `#(2)` — a sigil-led line.
/// On a continuation line the rewrite does not run, so the paren is mandatory.
#[test]
fn bare_sigil_parses_only_as_a_sigil_led_line() {
    assert_eq!(
        top("LET q =\n  #2").unwrap(),
        vec!["[t(LET) t(q) t(=) #[n(2)]]"]
    );
    let error = top("add 1,\n  #2").unwrap_err();
    assert_eq!(error, "parse error: expected '(' after '#', found '2'");
}

#[test]
fn bracket_continuation_with_bare_sigil_parse_errors() {
    let error = top("LET xs = [\n  #3\n]").unwrap_err();
    assert_eq!(error, "parse error: expected '(' after '#', found '3'");
}
