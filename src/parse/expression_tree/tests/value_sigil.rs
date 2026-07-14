//! `value_sigil` parse cases for `expression_tree::parse`.

use super::{top, tree};

#[test]
fn quote_sigil_captures_body_as_quoted_part() {
    // `#(foo)` captures its body at parse time — no keyword head, no call.
    assert_eq!(tree("#(foo)").unwrap(), "[#[t(foo)]]");
}

#[test]
fn eval_sigil_wraps_body_in_eval_keyword() {
    assert_eq!(tree("$(x)").unwrap(), "[[t(EVAL) [t(x)]]]");
}

#[test]
fn quote_sigil_preserves_multi_part_inner() {
    assert_eq!(tree("#(a b c)").unwrap(), "[#[t(a) t(b) t(c)]]");
}

#[test]
fn nested_sigils_quote_around_eval() {
    assert_eq!(tree("$(#(x))").unwrap(), "[[t(EVAL) [#[t(x)]]]]");
}

#[test]
fn quote_sigil_inside_list_literal() {
    assert_eq!(tree("[a #(b) c]").unwrap(), "[L[t(a) #[t(b)] t(c)]]");
}

#[test]
fn quote_sigil_as_dict_value() {
    assert_eq!(tree("{x: #(y)}").unwrap(), "[D{t(x): #[t(y)]}]");
}

#[test]
fn eval_sigil_as_call_argument() {
    assert_eq!(tree("PRINT $(x)").unwrap(), "[t(PRINT) [t(EVAL) [t(x)]]]");
}

/// The redundant-wrapper peel reaches *into* the quote: the indent collapse wraps a sigil-led
/// line's body in its own group (`#(+)` on its own line becomes `#((+))`), and every surface
/// form must still leave the quote holding exactly the single keyword part the user wrote.
#[test]
fn quote_of_single_keyword_holds_one_part_in_every_surface_form() {
    assert_eq!(top("#(+)").unwrap(), vec!["[#[t(+)]]"]);
    assert_eq!(
        top("LET plus = #(+)").unwrap(),
        vec!["[t(LET) t(plus) t(=) #[t(+)]]"]
    );
    assert_eq!(
        top("LET plus =\n  #+").unwrap(),
        vec!["[t(LET) t(plus) t(=) #[t(+)]]"]
    );
}

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
fn comma_continuation_with_paren_sigil_parses() {
    assert_eq!(top("add 1,\n  #(2)").unwrap(), top("add 1, #(2)").unwrap());
    assert_eq!(
        top("add 1,\n  #(2)").unwrap(),
        vec!["[t(add) n(1) #[n(2)]]"]
    );
}

#[test]
fn bracket_continuation_with_bare_sigil_parse_errors() {
    let error = top("LET xs = [\n  #3\n]").unwrap_err();
    assert_eq!(error, "parse error: expected '(' after '#', found '3'");
}

#[test]
fn bracket_continuation_with_paren_sigils_parses_to_quote_list() {
    assert_eq!(
        top("LET xs = [\n  #(3)\n  #(4)\n]").unwrap(),
        vec!["[t(LET) t(xs) t(=) L[#[n(3)] #[n(4)]]]"],
    );
}

#[test]
fn dict_continuation_with_paren_sigils_parses_to_quote_values() {
    assert_eq!(
        top("LET d = {\n  x: #(foo)\n  y: #(bar)\n}").unwrap(),
        vec!["[t(LET) t(d) t(=) D{t(x): #[t(foo)], t(y): #[t(bar)]}]"],
    );
}
