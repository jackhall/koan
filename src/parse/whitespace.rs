//! Second parse pass: collapse indentation-based blocks into parenthesized form, so the
//! downstream tree builder only has to deal with explicit `(...)` grouping. Reads the
//! masked output of `quotes` and produces a paren-structured string consumed by
//! `expression_tree::build_tree`. Detailed semantics live on `collapse_whitespace` below.
//!
//! See [design/expressions-and-parsing.md](../../design/expressions-and-parsing.md).

/// Convert indentation-based block structure into parenthesized form, so the downstream
/// `build_tree` only has to deal with `(...)` grouping. Each non-blank line becomes a `(...)`
/// group, deeper indents nest inside their parent, and dedents close the matching groups.
/// Rejects tab indentation and odd-numbered space indentation (only even-space indents allowed).
///
/// **Collection-literal continuations.** When a `[` or `{` opens but its matching `]`/`}` is
/// on a later line, the lines in between are *not* line-wrapped — they're appended to the open
/// span as plain whitespace-separated content, and `build_tree` pairs the brackets itself.
/// Compound indexing like `foo[idx]` is balanced on its own line, so it never tips depth across
/// line boundaries; the rule only fires when an open bracket and its match are on different
/// lines. A single delta counter handles `[`/`]` and `{`/`}` together — `build_tree`'s frame
/// stack catches any cross-pairing (`[1 2}`). Strings are already masked at this point, so
/// brackets inside them don't reach this function.
///
/// **Trailing-comma continuations.** A line whose trimmed content ends in `,` declares that
/// the expression continues on the next non-blank line — the same suspend-indentation path
/// the bracket case uses. This lets `UNION Maybe = (some: Number,\n  none: Null)` and bare
/// multi-line calls like `add 1,\n  2,\n  3` parse as one expression instead of siblings.
/// Parens (`(`) are intentionally *not* tracked the same way — they're already used to wrap
/// sub-expressions inside indent-structured blocks, so making them suspend indentation would
/// change the meaning of existing programs. The trailing comma is opt-in: lines that don't
/// end in `,` keep their old sibling-boundary behavior. Blank lines preserve the
/// continuation flag (they're skipped before the suspend check fires), so a blank line
/// between two comma-joined fragments doesn't break the chain.
pub fn collapse_whitespace(input: &str) -> Result<String, String> {
    let mut out = String::new();
    let mut stack: Vec<usize> = Vec::new();
    let mut delim_depth: i32 = 0;
    let mut continuing: bool = false;

    for (lineno, raw) in input.lines().enumerate() {
        let stripped = raw.trim_start();
        let indent = raw.len() - stripped.len();
        let content = stripped.trim_end();

        if content.is_empty() {
            continue;
        }

        if delim_depth > 0 || continuing {
            // Inside an open list/dict span or a trailing-comma continuation: append the line
            // as continuation content. Skip the indent / paren-wrapping pass — the line is
            // logically part of the open expression, not a sibling block.
            out.push(' ');
            out.push_str(content);
            delim_depth += line_delim_delta(content);
            continuing = content.ends_with(',');
            continue;
        }

        if raw[..indent].contains('\t') {
            return Err(format!("tab indentation not allowed on line {}", lineno + 1));
        }
        if indent % 2 != 0 {
            return Err(format!(
                "odd-numbered space indentation on line {}",
                lineno + 1
            ));
        }

        while let Some(&top) = stack.last() {
            if top >= indent {
                stack.pop();
                out.push(')');
            } else {
                break;
            }
        }

        if !out.is_empty() {
            out.push(' ');
        }
        // Sigil-led lines (`#…`, `$…`) place the wrapping paren *after* the sigil, so a
        // continuation like `#3` collapses to `#(3)` not `(#3)`. The latter would put the
        // sigil immediately before a non-paren character inside the wrapping group, which
        // `expression_tree` rejects under its sigil-adjacency rule. The closing `)` is still
        // emitted on dedent/EOF (one per stack entry), so the operand-rest of the line
        // (plus any deeper-indented children) ends up correctly inside the sigil's group.
        let (head, rest) = match content.as_bytes().first() {
            Some(&b'#') | Some(&b'$') => content.split_at(1),
            _ => ("", content),
        };
        out.push_str(head);
        out.push('(');
        out.push_str(rest);
        stack.push(indent);
        delim_depth += line_delim_delta(content);
        continuing = content.ends_with(',');
    }

    while stack.pop().is_some() {
        out.push(')');
    }

    Ok(out)
}

/// Net `[`+`{` − `]`+`}` count on a single line (post-quote-masking, post-trim). Compound
/// tokens like `foo[idx]` and `bar[i][j]` balance to zero per line because tokens can't span
/// lines, so only an unmatched list/dict literal `[` or `{` shifts the running depth. A single
/// counter conflates the two bracket families intentionally — this function only decides
/// whether we're "inside an open span"; `build_tree`'s frame stack enforces that a `[` is
/// matched by `]` (not `}`) and vice versa.
fn line_delim_delta(s: &str) -> i32 {
    let opens = s.chars().filter(|&c| c == '[' || c == '{').count() as i32;
    let closes = s.chars().filter(|&c| c == ']' || c == '}').count() as i32;
    opens - closes
}

#[cfg(test)]
mod tests {
    use super::collapse_whitespace;

    #[test]
    fn empty_input() {
        assert_eq!(collapse_whitespace("").unwrap(), "");
    }

    #[test]
    fn only_whitespace() {
        assert_eq!(collapse_whitespace("   \n\t\n   \n").unwrap(), "");
    }

    #[test]
    fn single_line() {
        assert_eq!(collapse_whitespace("foo").unwrap(), "(foo)");
    }

    #[test]
    fn single_line_multiple_tokens() {
        assert_eq!(collapse_whitespace("foo bar baz").unwrap(), "(foo bar baz)");
    }

    #[test]
    fn sibling_lines() {
        assert_eq!(collapse_whitespace("foo\nbar").unwrap(), "(foo) (bar)");
    }

    #[test]
    fn parent_with_child() {
        assert_eq!(collapse_whitespace("foo\n    bar").unwrap(), "(foo (bar))");
    }

    #[test]
    fn parent_with_two_children() {
        assert_eq!(
            collapse_whitespace("foo\n    bar\n    baz").unwrap(),
            "(foo (bar) (baz))"
        );
    }

    #[test]
    fn nested_three_deep() {
        assert_eq!(
            collapse_whitespace("a\n  b\n    c").unwrap(),
            "(a (b (c)))"
        );
    }

    #[test]
    fn dedent_back_to_root() {
        assert_eq!(
            collapse_whitespace("foo\n    bar\nbaz").unwrap(),
            "(foo (bar)) (baz)"
        );
    }

    #[test]
    fn dedent_multiple_levels() {
        assert_eq!(
            collapse_whitespace("a\n  b\n    c\nd").unwrap(),
            "(a (b (c))) (d)"
        );
    }

    #[test]
    fn child_then_sibling_then_child() {
        assert_eq!(
            collapse_whitespace("foo\n    bar\n    baz\n        qux\n    quux\nanother").unwrap(),
            "(foo (bar) (baz (qux)) (quux)) (another)"
        );
    }

    #[test]
    fn blank_lines_skipped() {
        assert_eq!(
            collapse_whitespace("foo\n\n    bar\n\n\nbaz").unwrap(),
            "(foo (bar)) (baz)"
        );
    }

    #[test]
    fn tabs_rejected() {
        assert!(collapse_whitespace("foo\n\tbar").is_err());
        assert!(collapse_whitespace("foo\n  \tbar").is_err());
    }

    #[test]
    fn odd_spaces_rejected() {
        assert!(collapse_whitespace("foo\n bar").is_err());
        assert!(collapse_whitespace("foo\n   bar").is_err());
    }

    #[test]
    fn multi_token_lines_nested() {
        assert_eq!(
            collapse_whitespace("if x > 0\n    print pos\n    y = 1\nelse\n    print neg").unwrap(),
            "(if x > 0 (print pos) (y = 1)) (else (print neg))"
        );
    }

    #[test]
    fn output_has_no_tabs_or_newlines() {
        let out = collapse_whitespace("a\n  b\n    c\n  d\ne").unwrap();
        assert!(!out.contains('\n'));
        assert!(!out.contains('\t'));
    }

    #[test]
    fn list_literal_open_suspends_indentation_handling() {
        // The `[` on line 1 stays open across lines 2–4, so those lines append to the list
        // span instead of becoming nested paren groups. The closing `]` brings depth back to 0.
        assert_eq!(
            collapse_whitespace("LET xs = [\n  1\n  2\n  3\n]").unwrap(),
            "(LET xs = [ 1 2 3 ])",
        );
    }

    #[test]
    fn multiline_list_with_continuation_indent() {
        // The `[1` opens at the end of line 1; lines 2 and 3 sit under it as continuation,
        // not as deeper-indent children. Final `]` closes the span.
        assert_eq!(
            collapse_whitespace("LET xs = [1\n          2\n          3]").unwrap(),
            "(LET xs = [1 2 3])",
        );
    }

    #[test]
    fn nested_multiline_lists() {
        // Inner `]` brings depth from 2 to 1 mid-line; outer `]` closes back to 0 on the
        // last line.
        assert_eq!(
            collapse_whitespace("[[1\n  2]\n [3 4]]").unwrap(),
            "([[1 2] [3 4]])",
        );
    }

    #[test]
    fn balanced_inline_list_does_not_perturb_indentation() {
        // `[1 2 3]` balances within its line, so depth stays at 0 and the indentation pass
        // continues normally — the next line becomes a sibling group as it would without
        // brackets at all.
        assert_eq!(
            collapse_whitespace("LET xs = [1 2 3]\nbar").unwrap(),
            "(LET xs = [1 2 3]) (bar)",
        );
    }

    #[test]
    fn multiline_dict_literal_continues() {
        // Same continuation rule as lists: `{` opens, lines append, `}` closes.
        assert_eq!(
            collapse_whitespace("LET d = {\n  a = 1\n  b = 2\n}").unwrap(),
            "(LET d = { a = 1 b = 2 })",
        );
    }

    #[test]
    fn inline_dict_does_not_perturb_indentation() {
        assert_eq!(
            collapse_whitespace("LET d = {a: 1}\nbar").unwrap(),
            "(LET d = {a: 1}) (bar)",
        );
    }

    #[test]
    fn nested_multiline_dict_inside_list() {
        // List opens on line 1, dict opens inside on line 2; both close on the last line.
        assert_eq!(
            collapse_whitespace("[\n  {a: 1\n   b: 2}\n]").unwrap(),
            "([ {a: 1 b: 2} ])",
        );
    }

    // --- Trailing-comma line continuation ---

    #[test]
    fn trailing_comma_continues_expression() {
        // The `,` at end of line 1 suspends indentation handling; line 2 appends to the open
        // group instead of becoming a child block.
        assert_eq!(
            collapse_whitespace("add 1,\n    2").unwrap(),
            "(add 1, 2)",
        );
    }

    #[test]
    fn trailing_comma_chain_across_three_lines() {
        // Continuation persists as long as each line keeps ending in `,`.
        assert_eq!(
            collapse_whitespace("foo 1,\n    2,\n    3").unwrap(),
            "(foo 1, 2, 3)",
        );
    }

    #[test]
    fn trailing_comma_inside_paren_expression() {
        // The motivating UNION shape: open paren on line 1, comma signals continuation,
        // close paren on line 2.
        assert_eq!(
            collapse_whitespace("UNION Maybe = (some :Number,\n               none :Null)")
                .unwrap(),
            "(UNION Maybe = (some :Number, none :Null))",
        );
    }

    #[test]
    fn trailing_comma_continuation_through_blank_line() {
        // Blank lines are skipped before the continuation check, so they don't break a
        // comma chain — same shape Python uses inside bracket continuations.
        assert_eq!(
            collapse_whitespace("add 1,\n\n    2").unwrap(),
            "(add 1, 2)",
        );
    }

    #[test]
    fn dangling_trailing_comma_at_eof() {
        // No following line to consume the continuation; the `,` rides through unchanged.
        // `build_tree` drops it as a no-op once it sees an expression-frame `,`.
        assert_eq!(collapse_whitespace("foo,").unwrap(), "(foo,)");
    }

    #[test]
    fn no_trailing_comma_keeps_sibling_boundary() {
        // Guard: lines that don't end in `,` still produce sibling groups.
        assert_eq!(collapse_whitespace("foo\nbar").unwrap(), "(foo) (bar)");
    }

    // --- Sigil-led continuation lines ---

    #[test]
    fn quote_sigil_continuation_wraps_outside_paren() {
        // `#3` on a continuation line must collapse to `#(3)`, not `(#3)` — the latter
        // violates `expression_tree`'s sigil-adjacency rule (sigil glued to a non-paren).
        assert_eq!(
            collapse_whitespace("LET x =\n  #3").unwrap(),
            "(LET x = #(3))",
        );
    }

    #[test]
    fn eval_sigil_continuation_wraps_outside_paren() {
        // Symmetric case for `$`: `$q` collapses to `$(q)` so the parser sees the sigil
        // immediately followed by `(`.
        assert_eq!(
            collapse_whitespace("foo\n  $q").unwrap(),
            "(foo $(q))",
        );
    }

    #[test]
    fn quote_sigil_at_top_level_wraps_outside_paren() {
        // The same rule applies even when the sigil-led line is itself the root of the
        // collapse (no parent expression). `#3` collapses to `#(3)`.
        assert_eq!(collapse_whitespace("#3").unwrap(), "#(3)");
    }

    #[test]
    fn sigil_with_paren_operand_still_legal() {
        // `#(3)` written on a continuation line collapses to `#((3))`. The double wrapping
        // is harmless: `peel_redundant` in `build_tree` strips extra single-`Expression`
        // wrappers downstream.
        assert_eq!(
            collapse_whitespace("foo\n  #(3)").unwrap(),
            "(foo #((3)))",
        );
    }

    #[test]
    fn sigil_continuation_with_deeper_children() {
        // Deeper-indented children of a sigil-led line live inside the sigil's group, so
        // the sigil applies to the whole sub-block.
        assert_eq!(
            collapse_whitespace("foo\n  #bar\n    baz").unwrap(),
            "(foo #(bar (baz)))",
        );
    }

    // --- Sigils on comma- and bracket-continuation lines (no wrap-operand fix) ---
    //
    // The wrap-outside-paren rewrite only runs on the indent-driven path. Lines consumed by
    // the comma-continuation or open-bracket/dict continuation path are appended verbatim,
    // so a bare `#sym` on those lines stays bare and reaches `build_tree` to be rejected by
    // the sigil-adjacency rule. These tests lock that contract in: the user gets a clear
    // parse error and must spell out `#(sym)` explicitly when continuing into a list/dict
    // literal or trailing-comma chain.

    #[test]
    fn comma_continuation_with_bare_sigil_stays_bare() {
        assert_eq!(
            collapse_whitespace("add 1,\n  #2").unwrap(),
            "(add 1, #2)",
        );
    }

    #[test]
    fn comma_continuation_with_paren_sigil_passes_through() {
        assert_eq!(
            collapse_whitespace("add 1,\n  #(2)").unwrap(),
            "(add 1, #(2))",
        );
    }

    #[test]
    fn bracket_continuation_with_bare_sigil_stays_bare() {
        assert_eq!(
            collapse_whitespace("LET xs = [\n  #3\n]").unwrap(),
            "(LET xs = [ #3 ])",
        );
    }

    #[test]
    fn bracket_continuation_with_paren_sigils_passes_through() {
        assert_eq!(
            collapse_whitespace("LET xs = [\n  #(3)\n  #(4)\n]").unwrap(),
            "(LET xs = [ #(3) #(4) ])",
        );
    }

    #[test]
    fn dict_continuation_with_paren_sigils_passes_through() {
        // The motivating dict-as-struct shape from the roadmap: each value is a `#(...)`
        // QUOTE that the struct constructor will dispatch on later.
        assert_eq!(
            collapse_whitespace("LET d = {\n  x = #(foo)\n  y = #(bar)\n}").unwrap(),
            "(LET d = { x = #(foo) y = #(bar) })",
        );
    }
}
