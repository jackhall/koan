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
    assert_eq!(
        error,
        "parse error: expected '(', '[' or '{' after '#', found '2'"
    );
}

#[test]
fn bracket_continuation_with_bare_sigil_parse_errors() {
    let error = top("LET xs = [\n  #3\n]").unwrap_err();
    assert_eq!(
        error,
        "parse error: expected '(', '[' or '{' after '#', found '3'"
    );
}

/// `#[…]` is the list, bare, with each element quoted: a paren group as that group, anything else
/// as a one-part quote.
#[test]
fn a_quoted_list_quotes_each_element() {
    assert_eq!(top("#[x y]").unwrap(), vec!["[L[#[t(x)] #[t(y)]]]"]);
    assert_eq!(
        top("#[(Elt UNDER Number) b]").unwrap(),
        vec!["[L[#[T(Elt) t(UNDER) T(Number)] #[t(b)]]]"]
    );
    // A nested literal is quoted whole.
    assert_eq!(top("#[[1 2]]").unwrap(), vec!["[L[#[L[n(1) n(2)]]]]"]);
    assert_eq!(
        top("#[\n  (VAL x :Str)\n  (TYPE Carrier)\n]").unwrap(),
        vec!["[L[#[t(VAL) t(x) T(Str)] #[t(TYPE) T(Carrier)]]]"]
    );
}

/// `#{…}` quotes each key and value of a dict, leaving a `_` key bare, and each value of a record,
/// leaving its field names bare.
#[test]
fn a_quoted_brace_quotes_each_entry() {
    assert_eq!(
        top("#{Some: (x), _: (y)}").unwrap(),
        vec!["[D{#[T(Some)]: #[t(x)], t(_): #[t(y)]}]"]
    );
    assert_eq!(top("#{x = 1}").unwrap(), vec!["[R{x = #[n(1)]}]"]);
    assert_eq!(
        top("#{\n  Some: (it)\n  None: (0)\n}").unwrap(),
        vec!["[D{#[T(Some)]: #[t(it)], #[T(None)]: #[n(0)]}]"]
    );
}

#[test]
fn eval_sigil_takes_only_a_paren() {
    assert!(top("${a: 1}").is_err());
    assert!(top("$[1]").is_err());
}
