//! Function signatures and their building blocks. An `ExpressionSignature` is the call shape
//! a `KFunction` matches against — an ordered mix of fixed `Keyword` tokens and typed
//! `Argument` slots, plus a `return_type`. `Scope::dispatch` walks each registered function's
//! signature looking for one whose `matches` returns true for an incoming `KExpression`;
//! `specificity_vs` then breaks ties between overloads sharing the same untyped shape.
//!
//! `UntypedKey` is the bucket key used to group overloads by shape only; `Specificity` ranks
//! candidates within a bucket. `is_keyword_token` is the parser-side classifier that decides
//! whether a source token is a `Keyword` or `Identifier`; both ends of dispatch (signature
//! registration and source-expression matching) rely on it agreeing with itself.

use crate::parse::kexpression::{ExpressionPart, KExpression};

use super::ktype::KType;

/// One position in a function's structural shape: a `Keyword` (fixed token) or a typeless
/// `Slot`. A sequence of these is the dispatch bucket key; overloads sharing a shape compete
/// on `KType` specificity within the bucket.
#[derive(Hash, Eq, PartialEq, Clone, Debug)]
pub enum UntypedElement {
    Keyword(String),
    Slot,
}

/// Bucket key produced by `ExpressionSignature::untyped_key` and `KExpression::untyped_key`.
/// They MUST agree on the same key for any signature/expression that should match. The parser
/// classifies source tokens into `ExpressionPart::Keyword` vs `ExpressionPart::Identifier` up
/// front using `is_keyword_token`; signatures map every `SignatureElement::Token` to
/// `Keyword`. `ExpressionSignature::normalize` uppercases lowercase registered tokens so the
/// two sides agree on the spelling.
pub type UntypedKey = Vec<UntypedElement>;

/// True iff `s` is a keyword (fixed token) rather than an identifier when classifying a source
/// token: no lowercase ASCII letters. `LET`, `=`, `THEN` qualify; `x`, `foo`, `Foo` don't.
/// Used by the parser's `classify_atom` and by `ExpressionSignature::normalize` to keep the
/// two ends of the dispatch contract aligned.
pub fn is_keyword_token(s: &str) -> bool {
    !s.chars().any(|c| c.is_ascii_lowercase())
}

/// Result of comparing two signatures' specificity. Returned by
/// `ExpressionSignature::specificity_vs`. `Equal` means "identical slot types"; `Incomparable`
/// means "neither dominates" — e.g. `<Number> <Any>` vs `<Any> <Number>` for an input that
/// matches both.
#[derive(Debug, Eq, PartialEq, Clone, Copy)]
pub enum Specificity {
    StrictlyMore,
    StrictlyLess,
    Equal,
    Incomparable,
}

/// The shape a function expects: an ordered mix of fixed `Token`s and typed `Argument` slots.
/// `Scope::dispatch` walks each registered function's signature looking for one whose
/// `matches` returns true for an incoming `KExpression`.
pub struct ExpressionSignature {
    pub return_type: KType,
    pub elements: Vec<SignatureElement>,
}

impl ExpressionSignature {
    pub fn matches(&self, expr: &KExpression<'_>) -> bool {
        if self.elements.len() != expr.parts.len() {
            return false;
        }
        self.elements.iter().zip(&expr.parts).all(|(el, part)| match (el, part) {
            (SignatureElement::Keyword(s), ExpressionPart::Keyword(t)) => s == t,
            (SignatureElement::Keyword(_), _) => false,
            (SignatureElement::Argument(arg), part) => arg.matches(part),
        })
    }

    /// Bucket key for this signature: keyword tokens become `Keyword(s)`, argument slots become
    /// `Slot`. Slot types are erased — same shape with different types lives in the same bucket
    /// and competes on specificity at dispatch time.
    pub fn untyped_key(&self) -> UntypedKey {
        self.elements
            .iter()
            .map(|el| match el {
                SignatureElement::Keyword(s) => UntypedElement::Keyword(s.clone()),
                SignatureElement::Argument(_) => UntypedElement::Slot,
            })
            .collect()
    }

    /// Registration-time fixup: uppercase any lowercase fixed `Token` so its bucket key matches
    /// what dispatch will compute from incoming expressions. TODO(monadic-effects): once
    /// effects exist, emit a warning here instead of silently rewriting — rejecting would lose
    /// the "drop in a builtin without thinking about caps" affordance.
    pub fn normalize(&mut self) {
        for el in &mut self.elements {
            if let SignatureElement::Keyword(s) = el {
                if s.chars().any(|c| c.is_ascii_lowercase()) {
                    *s = s.to_ascii_uppercase();
                }
            }
        }
    }

    /// Partial-order specificity comparison for overload tiebreaking. Assumes `self` and
    /// `other` share an `UntypedKey` (caller's responsibility) — only argument slots
    /// contribute, since fixed-token positions are equal by construction.
    pub fn specificity_vs(&self, other: &ExpressionSignature) -> Specificity {
        let mut any_more = false;
        let mut any_less = false;
        for (a, b) in self.elements.iter().zip(other.elements.iter()) {
            if let (SignatureElement::Argument(aa), SignatureElement::Argument(bb)) = (a, b) {
                if aa.ktype.is_more_specific_than(&bb.ktype) {
                    any_more = true;
                } else if bb.ktype.is_more_specific_than(&aa.ktype) {
                    any_less = true;
                }
            }
        }
        match (any_more, any_less) {
            (true, false) => Specificity::StrictlyMore,
            (false, true) => Specificity::StrictlyLess,
            (false, false) => Specificity::Equal,
            (true, true) => Specificity::Incomparable,
        }
    }
}

/// One slot in an `ExpressionSignature`: a literal `Token` that must match by string equality,
/// or a typed `Argument` whose value is captured into the `ArgumentBundle`.
pub enum SignatureElement {
    Keyword(String),
    Argument(Argument),
}

/// A typed parameter slot in a signature. `name` keys it in the `ArgumentBundle`; `ktype` gates
/// what `ExpressionPart`s it accepts.
pub struct Argument {
    pub name: String,
    pub ktype: KType,
}

impl Argument {
    /// Per-part type check. Thin delegate to `KType::accepts_part` — the per-variant table
    /// lives there so it stays next to the `KType` enum.
    pub fn matches(&self, part: &ExpressionPart<'_>) -> bool {
        self.ktype.accepts_part(part)
    }
}
