//! The parameter-name positions of a signature parts run.
//!
//! A `FN` signature interleaves keyword parts with `<name> :<Type>` pairs, and reading it means
//! knowing that stride: which parts are binders, which are the annotations those binders own, and
//! which are neither. [`SignatureScan`] is the one statement of it. Every reader of a signature's
//! binder positions drives off this iterator, so the stride is defined once no matter how many
//! questions are asked of it.

use crate::machine::model::ExpressionPart;
use crate::machine::model::labels::{BinderSymbol, KeywordSymbol};
use crate::source::Spanned;

/// One position in a signature parts run, as [`SignatureScan`] reads it.
pub(crate) enum SignaturePosition {
    /// A keyword part — fixed syntax, no binder.
    Keyword(KeywordSymbol),
    /// A `<name> :<Type>` pair: the binder the name part declares, plus the index of the
    /// annotation part it owns. The annotation's own shape (a bare `Type` leaf, an expression to
    /// dispatch, a sigiled type expression, a record type) is left for the caller to read off
    /// `parts[annotation]`.
    Annotated {
        name: BinderSymbol,
        annotation: usize,
    },
    /// A name part with no annotation following it — a binder position the signature left
    /// unannotated.
    Bare(BinderSymbol),
    /// A part that is neither a keyword nor a name: the index it sits at.
    Foreign(usize),
}

/// Walks a signature parts run one position at a time.
///
/// Each part's class *is* its binder's channel: the lexer tags an `Identifier` only for a token
/// classifying as neither keyword nor Type, and a `Type` part only for one classifying as a Type
/// token, so a name position hands over the symbol its own token minted. A bare-leaf `Type` in
/// name position (`er` in `FN (LIFT er :Ordered) -> …`) therefore declares a type binder rather
/// than referencing a type.
pub(crate) struct SignatureScan<'p, 'a> {
    parts: &'p [Spanned<ExpressionPart<'a>>],
    index: usize,
}

impl<'p, 'a> SignatureScan<'p, 'a> {
    pub(crate) fn new(parts: &'p [Spanned<ExpressionPart<'a>>]) -> Self {
        Self { parts, index: 0 }
    }
}

/// Whether the part at `index` can serve as a binder's type annotation.
fn is_annotation(parts: &[Spanned<ExpressionPart<'_>>], index: usize) -> bool {
    matches!(
        parts.get(index).map(|p| p.value),
        Some(
            ExpressionPart::Type(_)
                | ExpressionPart::Expression(_)
                | ExpressionPart::SigiledTypeExpr(_)
                | ExpressionPart::RecordType(_)
        )
    )
}

impl Iterator for SignatureScan<'_, '_> {
    type Item = SignaturePosition;

    fn next(&mut self) -> Option<SignaturePosition> {
        let part = self.parts.get(self.index)?.value;
        let at = self.index;
        let name = match part {
            ExpressionPart::Keyword(keyword) => {
                self.index += 1;
                return Some(SignaturePosition::Keyword(keyword));
            }
            ExpressionPart::Identifier(value) => BinderSymbol::Value(value),
            ExpressionPart::Type(kind) => BinderSymbol::Type(kind),
            _ => {
                self.index += 1;
                return Some(SignaturePosition::Foreign(at));
            }
        };
        if is_annotation(self.parts, at + 1) {
            self.index += 2;
            Some(SignaturePosition::Annotated {
                name,
                annotation: at + 1,
            })
        } else {
            self.index += 1;
            Some(SignaturePosition::Bare(name))
        }
    }
}
