//! Second parse pass: collapse indentation-based blocks into parenthesized form. Reads the
//! masked **byte** output of `quotes` (which is valid UTF-8 — see `quotes` for the marker
//! encoding) and produces a paren-structured **byte** stream consumed by
//! `expression_tree::build_tree`.
//!
//! Phase 2: the input/output types switch to `&[u8]` / `Vec<u8>`, but the line-collapse
//! logic still operates over `&str` internally. The in-band LITERAL/JUMP markers from
//! `mask_quotes` are non-whitespace ASCII C0 bytes (0x1D–0x1F), so they pass through the
//! whitespace-aware logic untouched. Phase 3 will rewrite this to emit its own JUMP
//! markers at line/synthetic-char boundaries.
//!
//! See [design/expressions-and-parsing.md](../../design/expressions-and-parsing.md).

/// Each non-blank line becomes a `(...)` group; deeper indents nest, dedents close. Tabs and
/// odd-numbered space indentation are rejected.
///
/// **Collection-literal continuations.** When `[`/`{` opens but its match is on a later line,
/// intermediate lines are appended to the open span instead of being wrapped — `build_tree`
/// pairs the brackets. A single delta counter conflates `[`/`{`; `build_tree`'s frame stack
/// catches cross-pairing like `[1 2}`. Strings are already masked, so brackets inside them
/// don't reach this function.
///
/// **Trailing-comma continuations.** A line ending in `,` suspends indentation through the
/// next non-blank line, so `UNION Maybe = (some: Number,\n  none: Null)` parses as one
/// expression. Parens (`(`) are intentionally *not* tracked the same way — they already wrap
/// sub-expressions inside indent-structured blocks, so suspending on them would change the
/// meaning of existing programs. Blank lines preserve the continuation flag.
pub fn collapse_whitespace(input: &[u8]) -> Result<Vec<u8>, String> {
    let s = std::str::from_utf8(input)
        .map_err(|_| "collapse_whitespace expected UTF-8 input".to_string())?;
    collapse_str(s).map(String::into_bytes)
}

fn collapse_str(input: &str) -> Result<String, String> {
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
        // Sigil-led lines place the wrapping paren *after* the sigil (`#3` → `#(3)`, not
        // `(#3)`); `expression_tree` rejects a sigil adjacent to a non-paren inside a group.
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

/// Net `[`+`{` − `]`+`}` on a single (post-mask, post-trim) line. The two bracket families
/// are conflated intentionally — this only decides whether we're inside an open span;
/// `build_tree`'s frame stack enforces `[`/`]` and `{`/`}` pairing.
fn line_delim_delta(s: &str) -> i32 {
    let opens = s.chars().filter(|&c| c == '[' || c == '{').count() as i32;
    let closes = s.chars().filter(|&c| c == ']' || c == '}').count() as i32;
    opens - closes
}

#[cfg(test)]
mod tests;
