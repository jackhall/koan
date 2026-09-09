//! Brace-literal sub-state-machine. One `{…}` frame serves both containers: a **dict**
//! (`{k: v}`, `:` pairs) and a **record** (`{x = 1}`, `=` pairs). The first pairing operator
//! selects the mode (`accept_colon` / `accept_equals`); mixing the two is an error, and an empty
//! `{}` is the empty record. The lowering delegates to `accept_colon`, `accept_equals`,
//! `accept_comma`, and `finish`; multi-part keys/values collapse into a sub-expression via
//! `single_or_wrapped`.

use crate::machine::KError;
use crate::machine::model::ast::ExpressionPart;
use crate::machine::model::labels::{BinderSymbol, LabelInterner};
use crate::memory::ProgramBrand;
use crate::source::Spanned;

pub(super) struct DictFrame<'a> {
    brand: ProgramBrand<'a>,
    pairs: Vec<(ExpressionPart<'a>, ExpressionPart<'a>)>,
    state: DictPairState<'a>,
    mode: BraceMode,
}

/// Which pairing operator the brace frame committed to. `Unknown` until the first
/// separator — an empty `{}` finishes as an empty record (the top of the record lattice).
#[derive(PartialEq, Clone, Copy)]
enum BraceMode {
    Unknown,
    Dict,
    Record,
}

/// What a finished brace frame yields: a dict's `(key, value)` pairs or a record's
/// `(field-name, value)` pairs (a record key is the Identifier or Type token's own symbol,
/// validated at `finish`).
pub(super) enum BraceContents<'a> {
    Dict(Vec<(ExpressionPart<'a>, ExpressionPart<'a>)>),
    Record(Vec<(BinderSymbol, ExpressionPart<'a>)>),
}

const MIXED_DELIMITERS: &str = "mixed `:` and `=` in a brace literal: use `=` for every field (record) \
     or `:` for every entry (dict)";

/// An entry that reached the end of the brace, or a comma, without its pairing operator. Which
/// operator is missing follows from the mode the frame committed to; before either has been seen
/// neither is more expected than the other, so the message names both.
///
/// A key run holding a type is the case worth a second sentence. A `:` glued to what follows it
/// is a type sigil wherever it is written, so `{k :Number}` and `{'k':(f x)}` are a key beside an
/// annotation with nothing pairing them — and without the hint the writer sees only that a
/// separator is missing, not that the `:` they wrote was read as the other thing.
fn unterminated_entry(mode: BraceMode, key: &[ExpressionPart<'_>]) -> KError {
    let mut message = match mode {
        BraceMode::Record => "unterminated field in record literal (missing '=')".to_string(),
        BraceMode::Dict => "unterminated key in dict literal (missing ':')".to_string(),
        BraceMode::Unknown => "unterminated entry in a brace literal: write `key: value` for a \
             dict entry, or `name = value` for a record field"
            .to_string(),
    };
    if mode == BraceMode::Unknown
        && key.iter().any(|part| {
            matches!(
                part,
                ExpressionPart::Type(_) | ExpressionPart::SigiledTypeExpr(_)
            )
        })
    {
        message.push_str(
            ". The `:` here is glued to what follows it, so it reads as a type sigil rather \
             than a separator — and a record *type* is written `:{name :Type}`",
        );
    }
    KError::parse(message, None)
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
/// dispatches them. The wrapper is spanless: a brace frame stores bare parts, so no span
/// survives to stamp on it.
fn single_or_wrapped<'a>(
    brand: ProgramBrand<'a>,
    parts: Vec<ExpressionPart<'a>>,
) -> ExpressionPart<'a> {
    match <[ExpressionPart<'a>; 1]>::try_from(parts) {
        Ok([single]) => single,
        Err(parts) => {
            ExpressionPart::expression_from_iter(brand, parts.into_iter().map(Spanned::bare))
        }
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
    pub(super) fn new(brand: ProgramBrand<'a>) -> Self {
        Self {
            brand,
            pairs: Vec::new(),
            state: DictPairState::Empty,
            mode: BraceMode::Unknown,
        }
    }

    /// When the value side already has content and the new part could start a fresh
    /// key, auto-commit the in-progress pair before opening a new key.
    pub(super) fn push(&mut self, part: ExpressionPart<'a>) {
        let brand = self.brand;
        match &mut self.state {
            DictPairState::Empty => {
                self.state = DictPairState::Key(vec![part]);
            }
            DictPairState::Key(parts) => parts.push(part),
            DictPairState::Value { value, .. } => {
                if !value.is_empty() && is_dict_key_start_part(&part) {
                    let prev = std::mem::replace(&mut self.state, DictPairState::Empty);
                    if let DictPairState::Value { key, value } = prev {
                        self.pairs.push((key, single_or_wrapped(brand, value)));
                    }
                    self.state = DictPairState::Key(vec![part]);
                } else {
                    value.push(part);
                }
            }
        }
    }

    /// Shared separator step for `:` (dict) and `=` (record): commit the buffered
    /// key/field and open a value slot, or error. A separator in the opposite mode
    /// is a mixed-delimiter error; an empty buffer or a second separator inside a
    /// value uses the mode-specific `missing`/`inside_value` message.
    fn accept_separator(
        &mut self,
        target: BraceMode,
        conflict: BraceMode,
        missing: &str,
        inside_value: &str,
    ) -> Result<(), KError> {
        if self.mode == conflict {
            return Err(KError::parse(MIXED_DELIMITERS, None));
        }
        self.mode = target;
        match std::mem::replace(&mut self.state, DictPairState::Empty) {
            DictPairState::Empty => Err(KError::parse(missing, None)),
            DictPairState::Key(parts) if parts.is_empty() => Err(KError::parse(missing, None)),
            DictPairState::Key(parts) => {
                self.state = DictPairState::Value {
                    key: single_or_wrapped(self.brand, parts),
                    value: Vec::new(),
                };
                Ok(())
            }
            DictPairState::Value { key, value } => {
                self.state = DictPairState::Value { key, value };
                Err(KError::parse(inside_value, None))
            }
        }
    }

    /// Errors if no key was buffered or if a `:` arrives while a value is already
    /// being built — one `:` per pair. Selects (or confirms) dict mode.
    pub(super) fn accept_colon(&mut self) -> Result<(), KError> {
        self.accept_separator(
            BraceMode::Dict,
            BraceMode::Record,
            "missing key before ':' in dict literal",
            "unexpected ':' inside dict value",
        )
    }

    /// Record counterpart of [`accept_colon`](Self::accept_colon): a `=` separates a
    /// field name from its value. Selects (or confirms) record mode; one `=` per field.
    pub(super) fn accept_equals(&mut self) -> Result<(), KError> {
        self.accept_separator(
            BraceMode::Record,
            BraceMode::Dict,
            "missing field name before '=' in record literal",
            "unexpected '=' inside record value",
        )
    }

    /// Trailing or repeated commas no-op (`{a: 1,}` and `{a: 1,, b: 2}` both legal);
    /// a comma after a key without `:`, or after `:` with no value, errors.
    pub(super) fn accept_comma(&mut self) -> Result<(), KError> {
        match std::mem::replace(&mut self.state, DictPairState::Empty) {
            DictPairState::Empty => Ok(()),
            DictPairState::Key(parts) if parts.is_empty() => Ok(()),
            DictPairState::Key(parts) => {
                let error = unterminated_entry(self.mode, &parts);
                self.state = DictPairState::Key(parts);
                Err(error)
            }
            DictPairState::Value { value, .. } if value.is_empty() => Err(KError::parse(
                "missing value after ':' in dict literal",
                None,
            )),
            DictPairState::Value { key, value } => {
                self.pairs.push((key, single_or_wrapped(self.brand, value)));
                Ok(())
            }
        }
    }

    /// Commit any in-progress pair and yield the completed contents — a dict's pairs or
    /// a record's `(field, value)` list. Errors for a key/field without its separator, a
    /// separator without a value, or (record mode) a non-identifier field name.
    pub(super) fn finish(mut self, labels: &LabelInterner) -> Result<BraceContents<'a>, KError> {
        // Only an explicit `:` commits the frame to a dict; `Record` and the
        // separator-less `Unknown` (empty `{}`) both finish as a record.
        let is_record = self.mode != BraceMode::Dict;
        match self.state {
            DictPairState::Empty => {}
            DictPairState::Key(parts) if parts.is_empty() => {}
            DictPairState::Key(parts) => {
                return Err(unterminated_entry(self.mode, &parts));
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
                self.pairs.push((key, single_or_wrapped(self.brand, value)));
            }
        }
        if is_record {
            let mut fields = Vec::with_capacity(self.pairs.len());
            for (key, value) in self.pairs {
                let name = match key {
                    ExpressionPart::Identifier(v) => BinderSymbol::Value(v),
                    // A capitalized Type token is a valid literal field name (kept verbatim,
                    // never name-resolved) — e.g. abstract type-slot names in `WITH {Elt = T}`.
                    ExpressionPart::Type(t) => BinderSymbol::Type(t),
                    other => {
                        return Err(KError::parse(
                            format!(
                                "record field name must be a bare identifier or Type token, got `{}`",
                                other.summary(labels)
                            ),
                            None,
                        ));
                    }
                };
                // A record's field list is a static shape, so a repeated name is a
                // mistake rather than an override. Dict keys stay unchecked: they are
                // arbitrary value expressions keyed at runtime, not a shape.
                if fields.iter().any(|(seen, _)| *seen == name) {
                    return Err(KError::parse(
                        format!(
                            "duplicate field `{}` in record literal",
                            labels.render(name.symbol())
                        ),
                        None,
                    ));
                }
                fields.push((name, value));
            }
            Ok(BraceContents::Record(fields))
        } else {
            Ok(BraceContents::Dict(self.pairs))
        }
    }
}
