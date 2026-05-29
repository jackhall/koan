//! Dict-literal sub-state-machine for `build_tree`. The surrounding character handlers
//! delegate to `accept_colon`, `accept_comma`, and `finish`; multi-part keys/values
//! collapse into a sub-expression via `single_or_wrapped`.

use crate::machine::model::ast::ExpressionPart;
use crate::machine::KError;

pub(super) struct DictFrame<'a> {
    pairs: Vec<(ExpressionPart<'a>, ExpressionPart<'a>)>,
    state: DictPairState<'a>,
}

enum DictPairState<'a> {
    Empty,
    Key(Vec<ExpressionPart<'a>>),
    Value {
        key: ExpressionPart<'a>,
        value: Vec<ExpressionPart<'a>>,
    },
}

/// Single part stays as-is; multiple parts wrap as a sub-expression so the scheduler
/// dispatches them.
fn single_or_wrapped<'a>(parts: Vec<ExpressionPart<'a>>) -> ExpressionPart<'a> {
    match <[ExpressionPart<'a>; 1]>::try_from(parts) {
        Ok([single]) => single,
        Err(parts)   => ExpressionPart::expression(parts),
    }
}

/// Auto-commit trigger on the value side: any part that could be a fresh key on its own.
/// Lets commas be optional — `{a: 1 b: 2}` parses identically to `{a: 1, b: 2}`.
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

    /// When the value side already has content and the new part could start a fresh
    /// key, auto-commit the in-progress pair before opening a new key.
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

    /// Errors if no key was buffered or if a `:` arrives while a value is already
    /// being built — one `:` per pair.
    pub(super) fn accept_colon(&mut self) -> Result<(), KError> {
        match std::mem::replace(&mut self.state, DictPairState::Empty) {
            DictPairState::Empty => {
                Err(KError::parse("missing key before ':' in dict literal", None))
            }
            DictPairState::Key(parts) if parts.is_empty() => {
                Err(KError::parse("missing key before ':' in dict literal", None))
            }
            DictPairState::Key(parts) => {
                self.state = DictPairState::Value {
                    key: single_or_wrapped(parts),
                    value: Vec::new(),
                };
                Ok(())
            }
            DictPairState::Value { key, value } => {
                self.state = DictPairState::Value { key, value };
                Err(KError::parse("unexpected ':' inside dict value", None))
            }
        }
    }

    /// Trailing or repeated commas no-op (`{a: 1,}` and `{a: 1,, b: 2}` both legal);
    /// a comma after a key without `:`, or after `:` with no value, errors.
    pub(super) fn accept_comma(&mut self) -> Result<(), KError> {
        match std::mem::replace(&mut self.state, DictPairState::Empty) {
            DictPairState::Empty => Ok(()),
            DictPairState::Key(parts) if parts.is_empty() => Ok(()),
            DictPairState::Key(parts) => {
                self.state = DictPairState::Key(parts);
                Err(KError::parse("key without value in dict literal", None))
            }
            DictPairState::Value { value, .. } if value.is_empty() => {
                Err(KError::parse("missing value after ':' in dict literal", None))
            }
            DictPairState::Value { key, value } => {
                self.pairs.push((key, single_or_wrapped(value)));
                Ok(())
            }
        }
    }

    /// Commit any in-progress pair and yield the completed pair list. Errors for a
    /// key without `:` or a `:` without a value.
    pub(super) fn finish(
        mut self,
    ) -> Result<Vec<(ExpressionPart<'a>, ExpressionPart<'a>)>, KError> {
        match self.state {
            DictPairState::Empty => {}
            DictPairState::Key(parts) if parts.is_empty() => {}
            DictPairState::Key(_) => {
                return Err(KError::parse(
                    "unterminated key in dict literal (missing ':')",
                    None,
                ));
            }
            DictPairState::Value { value, .. } if value.is_empty() => {
                return Err(KError::parse(
                    "missing value after ':' in dict literal",
                    None,
                ));
            }
            DictPairState::Value { key, value } => {
                self.pairs.push((key, single_or_wrapped(value)));
            }
        }
        Ok(self.pairs)
    }
}
