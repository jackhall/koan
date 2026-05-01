/// Convert indentation-based block structure into parenthesized form, so the downstream
/// `build_tree` only has to deal with `(...)` grouping. Each non-blank line becomes a `(...)`
/// group, deeper indents nest inside their parent, and dedents close the matching groups.
/// Rejects tab indentation and odd-numbered space indentation (only even-space indents allowed).
///
/// **List-literal continuations.** When a `[` opens but its matching `]` is on a later line,
/// the lines in between are *not* line-wrapped — they're appended to the open list span as
/// plain whitespace-separated content, and `build_tree` pairs the brackets itself. Compound
/// indexing like `foo[idx]` is balanced on its own line, so it never tips depth across line
/// boundaries; the rule only fires when `[` and `]` are on different lines. Strings are
/// already masked at this point, so brackets inside them don't reach this function.
pub fn collapse_whitespace(input: &str) -> Result<String, String> {
    let mut out = String::new();
    let mut stack: Vec<usize> = Vec::new();
    let mut bracket_depth: i32 = 0;

    for (lineno, raw) in input.lines().enumerate() {
        let stripped = raw.trim_start();
        let indent = raw.len() - stripped.len();
        let content = stripped.trim_end();

        if content.is_empty() {
            continue;
        }

        if bracket_depth > 0 {
            // Inside an open list span: append the line as continuation content. Skip the
            // indent / paren-wrapping pass — the line is logically part of the bracket
            // expression, not a sibling block.
            out.push(' ');
            out.push_str(content);
            bracket_depth += line_bracket_delta(content);
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
        out.push('(');
        out.push_str(content);
        stack.push(indent);
        bracket_depth += line_bracket_delta(content);
    }

    while stack.pop().is_some() {
        out.push(')');
    }

    Ok(out)
}

/// Net `[` − `]` count on a single line (post-quote-masking, post-trim). Compound tokens like
/// `foo[idx]` and `bar[i][j]` balance to zero per line because tokens can't span lines, so
/// only an unmatched list-literal `[` shifts the running depth.
fn line_bracket_delta(s: &str) -> i32 {
    let opens = s.chars().filter(|&c| c == '[').count() as i32;
    let closes = s.chars().filter(|&c| c == ']').count() as i32;
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
}
