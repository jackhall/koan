//! `value_sigil` parse cases for `parse`.

use super::{top, tree};

#[test]
fn quote_sigil_without_paren_errors() {
    // Sigil surface is paren-only.
    assert!(tree("#foo").is_err());
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
    assert!(tree("\\").is_err());
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
fn a_group_mark_takes_only_a_paren() {
    assert!(top("${a: 1}").is_err());
    assert!(top("$[1]").is_err());
    assert!(top("\\[1]").is_err());
}

/// A mark leading a word marks the value or type name the word starts with.
#[test]
fn a_mark_leading_a_word_marks_its_name() {
    assert_eq!(tree("$x").unwrap(), "[$t(x)]");
    assert_eq!(tree("\\x").unwrap(), "[\\t(x)]");
    assert_eq!(tree("$Point").unwrap(), "[$T(Point)]");
    assert_eq!(tree("\\Point").unwrap(), "[\\T(Point)]");
    // On a compound word the mark lands on the leading name.
    assert_eq!(tree("$a.b").unwrap(), "[[t(ATTR) $t(a) t(b)]]");
}

#[test]
fn a_mark_leading_no_name_is_refused() {
    for source in ["$3", "$..xs", "\\true", "$LET", "$$x"] {
        let error = tree(source).unwrap_err();
        assert!(
            error.contains(&format!("`{source}`")),
            "{source}: the refusal names the word, got {error}"
        );
    }
}

/// `$(…)` and `\(…)` mark exactly one keyworded use, open buckets such as `PRINT`'s or `==`'s
/// included.
#[test]
fn a_group_mark_wraps_a_keyworded_use() {
    assert_eq!(tree("$(PRINT x)").unwrap(), "[$[t(PRINT) t(x)]]");
    assert_eq!(tree("\\(a == b)").unwrap(), "[\\[t(a) t(==) t(b)]]");
    assert_eq!(tree("$(a + b)").unwrap(), "[$[t(a) t(+) t(b)]]");
}

/// A group holding no keyworded use, or a closed builtin expression shape, is refused.
#[test]
fn a_group_mark_around_no_open_keyworded_use_is_refused() {
    for source in [
        "$(f x)",
        "$(y)",
        "\\(LET x = 1)",
        "$(FN :{} -> Number = #(1))",
    ] {
        let error = tree(source).unwrap_err();
        assert!(
            error.contains("wraps exactly one keyworded use"),
            "{source}: got {error}"
        );
    }
}

/// No line is led by a mark: a line starting with `$x` is a statement whose first part is the
/// marked name, inside a quote as at the top level.
#[test]
fn no_line_is_led_by_a_mark() {
    assert_eq!(top("$x MINUS 1").unwrap(), vec!["[$t(x) t(MINUS) n(1)]"]);
    assert_eq!(
        top("LET q = #(\n  $x\n  \\y\n  $(PRINT z)\n)").unwrap(),
        vec!["[t(LET) t(q) t(=) #[[$t(x)] [\\t(y)] [$[t(PRINT) t(z)]]]]"]
    );
}
