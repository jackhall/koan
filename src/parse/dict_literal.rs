//! Brace-literal sub-state-machine for `build_tree`. One `{…}` frame serves both
//! containers: a **dict** (`{k: v}`, `:` pairs) and a **record** (`{x = 1}`, `=` pairs).
//! The first pairing operator selects the mode (`accept_colon` / `accept_equals`); mixing
//! the two is an error, and an empty `{}` stays a dict. The surrounding character handlers
//! delegate to `accept_colon`, `accept_equals`, `accept_comma`, and `finish`; multi-part
//! keys/values collapse into a sub-expression via `single_or_wrapped`.

use crate::machine::model::ast::ExpressionPart;
use crate::machine::KError;

pub(super) struct DictFrame<'a> {
    pairs: Vec<(ExpressionPart<'a>, ExpressionPart<'a>)>,
    state: DictPairState<'a>,
    mode: BraceMode,
}

/// Which pairing operator the brace frame committed to. `Unknown` until the first
/// separator — an empty `{}` finishes as an empty dict.
#[derive(PartialEq, Clone, Copy)]
enum BraceMode {
    Unknown,
    Dict,
    Record,
}

/// What a finished brace frame yields: a dict's `(key, value)` pairs or a record's
/// `(field-name, value)` pairs (record keys are bare identifiers, validated at `finish`).
pub(super) enum BraceContents<'a> {
    Dict(Vec<(ExpressionPart<'a>, ExpressionPart<'a>)>),
    Record(Vec<(String, ExpressionPart<'a>)>),
}

const MIXED_DELIMITERS: &str =
    "mixed `:` and `=` in a brace literal: use `=` for every field (record) \
     or `:` for every entry (dict)";

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
        Err(parts) => ExpressionPart::expression(parts),
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
        Self {
            pairs: Vec::new(),
            state: DictPairState::Empty,
            mode: BraceMode::Unknown,
        }
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
    /// being built — one `:` per pair. Selects (or confirms) dict mode.
    pub(super) fn accept_colon(&mut self) -> Result<(), KError> {
        if self.mode == BraceMode::Record {
            return Err(KError::parse(MIXED_DELIMITERS, None));
        }
        self.mode = BraceMode::Dict;
        match std::mem::replace(&mut self.state, DictPairState::Empty) {
            DictPairState::Empty => Err(KError::parse(
                "missing key before ':' in dict literal",
                None,
            )),
            DictPairState::Key(parts) if parts.is_empty() => Err(KError::parse(
                "missing key before ':' in dict literal",
                None,
            )),
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

    /// Record counterpart of [`accept_colon`](Self::accept_colon): a `=` separates a
    /// field name from its value. Selects (or confirms) record mode; one `=` per field.
    pub(super) fn accept_equals(&mut self) -> Result<(), KError> {
        if self.mode == BraceMode::Dict {
            return Err(KError::parse(MIXED_DELIMITERS, None));
        }
        self.mode = BraceMode::Record;
        match std::mem::replace(&mut self.state, DictPairState::Empty) {
            DictPairState::Empty => Err(KError::parse(
                "missing field name before '=' in record literal",
                None,
            )),
            DictPairState::Key(parts) if parts.is_empty() => Err(KError::parse(
                "missing field name before '=' in record literal",
                None,
            )),
            DictPairState::Key(parts) => {
                self.state = DictPairState::Value {
                    key: single_or_wrapped(parts),
                    value: Vec::new(),
                };
                Ok(())
            }
            DictPairState::Value { key, value } => {
                self.state = DictPairState::Value { key, value };
                Err(KError::parse("unexpected '=' inside record value", None))
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
            DictPairState::Value { value, .. } if value.is_empty() => Err(KError::parse(
                "missing value after ':' in dict literal",
                None,
            )),
            DictPairState::Value { key, value } => {
                self.pairs.push((key, single_or_wrapped(value)));
                Ok(())
            }
        }
    }

    /// Commit any in-progress pair and yield the completed contents — a dict's pairs or
    /// a record's `(field, value)` list. Errors for a key/field without its separator, a
    /// separator without a value, or (record mode) a non-identifier field name.
    pub(super) fn finish(mut self) -> Result<BraceContents<'a>, KError> {
        let is_record = self.mode == BraceMode::Record;
        match self.state {
            DictPairState::Empty => {}
            DictPairState::Key(parts) if parts.is_empty() => {}
            DictPairState::Key(_) => {
                return Err(KError::parse(
                    if is_record {
                        "unterminated field in record literal (missing '=')"
                    } else {
                        "unterminated key in dict literal (missing ':')"
                    },
                    None,
                ));
            }
            DictPairState::Value { value, .. } if value.is_empty() => {
                return Err(KError::parse(
                    if is_record {
                        "missing value after '=' in record literal"
                    } else {
                        "missing value after ':' in dict literal"
                    },
                    None,
                ));
            }
            DictPairState::Value { key, value } => {
                self.pairs.push((key, single_or_wrapped(value)));
            }
        }
        if is_record {
            let mut fields = Vec::with_capacity(self.pairs.len());
            for (key, value) in self.pairs {
                let name = match key {
                    ExpressionPart::Identifier(s) => s,
                    other => {
                        return Err(KError::parse(
                            format!(
                                "record field name must be a bare identifier, got `{}`",
                                other.summarize()
                            ),
                            None,
                        ));
                    }
                };
                fields.push((name, value));
            }
            Ok(BraceContents::Record(fields))
        } else {
            Ok(BraceContents::Dict(self.pairs))
        }
    }
}
