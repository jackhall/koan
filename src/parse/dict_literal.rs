use crate::parse::kexpression::ExpressionPart;

/// In-progress dict literal: completed pairs plus the state of the current pair. Owns its
/// own state machine so character handlers in `build_tree` delegate to `accept_colon`,
/// `accept_comma`, and `finish` instead of pattern-matching the state inline.
pub(super) struct DictFrame<'a> {
    pairs: Vec<(ExpressionPart<'a>, ExpressionPart<'a>)>,
    state: DictPairState<'a>,
}

/// State of the in-progress key/value pair inside a `DictFrame`. `Empty` is "ready for a
/// fresh key", `Key(parts)` is "accumulating key parts before we see ':'", `Value { key,
/// value }` is "saw ':', now collecting value parts until `,`, `}`, or auto-commit fires".
/// A multi-part key/value collapses via `single_or_wrapped`: one part stays as-is, multiple
/// parts wrap into a sub-expression.
enum DictPairState<'a> {
    Empty,
    Key(Vec<ExpressionPart<'a>>),
    Value {
        key: ExpressionPart<'a>,
        value: Vec<ExpressionPart<'a>>,
    },
}

/// Collapse the buffered parts of one half of a dict pair: a single part is the half
/// directly; multiple parts wrap as a sub-expression so the scheduler dispatches them.
fn single_or_wrapped<'a>(mut parts: Vec<ExpressionPart<'a>>) -> ExpressionPart<'a> {
    if parts.len() == 1 {
        parts.pop().unwrap()
    } else {
        ExpressionPart::expression(parts)
    }
}

/// Auto-commit threshold for `DictFrame`'s value side: any incoming part that could
/// plausibly start a fresh key (i.e. would be a complete dict half on its own). Used to
/// make commas optional — `{a: 1 b: 2}` parses identically to `{a: 1, b: 2}` because the
/// `b` token, arriving while `value = [1]`, triggers the previous pair's commit.
fn is_dict_key_start_part(part: &ExpressionPart<'_>) -> bool {
    matches!(
        part,
        ExpressionPart::Identifier(_)
            | ExpressionPart::Type(_)
            | ExpressionPart::Literal(_)
            | ExpressionPart::Expression(_)
            | ExpressionPart::ListLiteral(_)
            | ExpressionPart::DictLiteral(_)
    )
}

impl<'a> DictFrame<'a> {
    pub(super) fn new() -> Self {
        Self { pairs: Vec::new(), state: DictPairState::Empty }
    }

    /// Accept an incoming part. Either appends to the current key/value being accumulated,
    /// or — when a value-side already has content and the new part could plausibly start a
    /// fresh key — auto-commits the in-progress pair before opening a new key.
    pub(super) fn push(&mut self, part: ExpressionPart<'a>) {
        match &mut self.state {
            DictPairState::Empty => {
                self.state = DictPairState::Key(vec![part]);
            }
            DictPairState::Key(parts) => parts.push(part),
            DictPairState::Value { value, .. } => {
                if !value.is_empty() && is_dict_key_start_part(&part) {
                    let prev = std::mem::replace(&mut self.state, DictPairState::Empty);
                    if let DictPairState::Value { key, value } = prev {
                        self.pairs.push((key, single_or_wrapped(value)));
                    }
                    self.state = DictPairState::Key(vec![part]);
                } else {
                    value.push(part);
                }
            }
        }
    }

    /// Handle a `:` — promote the buffered key parts into a finalized key and switch to
    /// accumulating the value side. Errors if no key was buffered or if a `:` arrives
    /// while a value is already being built (one `:` per pair).
    pub(super) fn accept_colon(&mut self) -> Result<(), String> {
        match std::mem::replace(&mut self.state, DictPairState::Empty) {
            DictPairState::Empty => {
                Err("missing key before ':' in dict literal".to_string())
            }
            DictPairState::Key(parts) if parts.is_empty() => {
                Err("missing key before ':' in dict literal".to_string())
            }
            DictPairState::Key(parts) => {
                self.state = DictPairState::Value {
                    key: single_or_wrapped(parts),
                    value: Vec::new(),
                };
                Ok(())
            }
            DictPairState::Value { key, value } => {
                // Restore for diagnostic context, then error.
                self.state = DictPairState::Value { key, value };
                Err("unexpected ':' inside dict value".to_string())
            }
        }
    }

    /// Handle a `,` — commit the in-progress pair if a value has been collected. Trailing
    /// or repeated commas no-op (`{a: 1,}` and `{a: 1,, b: 2}` both legal); a comma after
    /// a key without a value, or after `:` with no value, errors.
    pub(super) fn accept_comma(&mut self) -> Result<(), String> {
        match std::mem::replace(&mut self.state, DictPairState::Empty) {
            DictPairState::Empty => Ok(()),
            DictPairState::Key(parts) if parts.is_empty() => Ok(()),
            DictPairState::Key(parts) => {
                self.state = DictPairState::Key(parts);
                Err("key without value in dict literal".to_string())
            }
            DictPairState::Value { value, .. } if value.is_empty() => {
                Err("missing value after ':' in dict literal".to_string())
            }
            DictPairState::Value { key, value } => {
                self.pairs.push((key, single_or_wrapped(value)));
                Ok(())
            }
        }
    }

    /// Handle `}` — commit any in-progress pair and yield the completed pair list. Errors
    /// for a key without `:` or a `:` without a value.
    pub(super) fn finish(mut self) -> Result<Vec<(ExpressionPart<'a>, ExpressionPart<'a>)>, String> {
        match self.state {
            DictPairState::Empty => {}
            DictPairState::Key(parts) if parts.is_empty() => {}
            DictPairState::Key(_) => {
                return Err("unterminated key in dict literal (missing ':')".to_string());
            }
            DictPairState::Value { value, .. } if value.is_empty() => {
                return Err("missing value after ':' in dict literal".to_string());
            }
            DictPairState::Value { key, value } => {
                self.pairs.push((key, single_or_wrapped(value)));
            }
        }
        Ok(self.pairs)
    }
}
