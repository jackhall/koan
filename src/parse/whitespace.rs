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
mod tests;
