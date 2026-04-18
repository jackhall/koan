pub fn collapse_whitespace(input: &str) -> String {
    let mut out = String::new();
    let mut stack: Vec<usize> = Vec::new();

    for raw in input.lines() {
        let stripped = raw.trim_start();
        let indent = raw.len() - stripped.len();
        let content = stripped.trim_end();

        if content.is_empty() {
            continue;
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
    }

    while stack.pop().is_some() {
        out.push(')');
    }

    out
}

#[cfg(test)]
mod tests {
    use super::collapse_whitespace;

    #[test]
    fn empty_input() {
        assert_eq!(collapse_whitespace(""), "");
    }

    #[test]
    fn only_whitespace() {
        assert_eq!(collapse_whitespace("   \n\t\n   \n"), "");
    }

    #[test]
    fn single_line() {
        assert_eq!(collapse_whitespace("foo"), "(foo)");
    }

    #[test]
    fn single_line_multiple_tokens() {
        assert_eq!(collapse_whitespace("foo bar baz"), "(foo bar baz)");
    }

    #[test]
    fn sibling_lines() {
        assert_eq!(collapse_whitespace("foo\nbar"), "(foo) (bar)");
    }

    #[test]
    fn parent_with_child() {
        assert_eq!(collapse_whitespace("foo\n    bar"), "(foo (bar))");
    }

    #[test]
    fn parent_with_two_children() {
        assert_eq!(
            collapse_whitespace("foo\n    bar\n    baz"),
            "(foo (bar) (baz))"
        );
    }

    #[test]
    fn nested_three_deep() {
        assert_eq!(
            collapse_whitespace("a\n  b\n    c"),
            "(a (b (c)))"
        );
    }

    #[test]
    fn dedent_back_to_root() {
        assert_eq!(
            collapse_whitespace("foo\n    bar\nbaz"),
            "(foo (bar)) (baz)"
        );
    }

    #[test]
    fn dedent_multiple_levels() {
        assert_eq!(
            collapse_whitespace("a\n  b\n    c\nd"),
            "(a (b (c))) (d)"
        );
    }

    #[test]
    fn child_then_sibling_then_child() {
        assert_eq!(
            collapse_whitespace("foo\n    bar\n    baz\n        qux\n    quux\nanother"),
            "(foo (bar) (baz (qux)) (quux)) (another)"
        );
    }

    #[test]
    fn blank_lines_skipped() {
        assert_eq!(
            collapse_whitespace("foo\n\n    bar\n\n\nbaz"),
            "(foo (bar)) (baz)"
        );
    }

    #[test]
    fn tabs_as_indent() {
        assert_eq!(
            collapse_whitespace("foo\n\tbar\n\tbaz"),
            "(foo (bar) (baz))"
        );
    }

    #[test]
    fn multi_token_lines_nested() {
        assert_eq!(
            collapse_whitespace("if x > 0\n    print pos\n    y = 1\nelse\n    print neg"),
            "(if x > 0 (print pos) (y = 1)) (else (print neg))"
        );
    }

    #[test]
    fn output_has_no_tabs_or_newlines() {
        let out = collapse_whitespace("a\n\tb\n\t\tc\n\td\ne");
        assert!(!out.contains('\n'));
        assert!(!out.contains('\t'));
    }
}
