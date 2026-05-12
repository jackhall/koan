//! Expression-signature machinery: the call shape a `KFunction` matches against — an ordered
//! mix of fixed `Keyword` tokens and typed `Argument` slots, plus a `return_type`.
//! `UntypedKey` groups overloads by shape; `Specificity` ranks candidates within a bucket.
//!
//! Not to be confused with the **module-signature** type (`SIG`-declared) at
//! [`crate::runtime::model::values::module::Signature`].

use crate::ast::{ExpressionPart, KExpression};

use super::ktype::KType;

#[derive(Hash, Eq, PartialEq, Clone, Debug)]
pub enum UntypedElement {
    Keyword(String),
    Slot,
}

/// Bucket key produced by both `ExpressionSignature::untyped_key` and
/// `KExpression::untyped_key`; they MUST agree for any pair that should match. The parser
/// classifies source tokens via `is_keyword_token`; `ExpressionSignature::normalize`
/// uppercases lowercase registered tokens so the two sides agree on spelling.
pub type UntypedKey = Vec<UntypedElement>;

/// True iff `s` classifies as a keyword (fixed token). See [token classes in
/// design/type-system.md](../../../design/type-system.md#token-classes--the-parser-level-foundation):
/// pure-symbol tokens (no ASCII letters) are always keywords; alphabetic tokens are keywords
/// iff they have ≥2 ASCII-uppercase letters and no ASCII-lowercase letters.
pub fn is_keyword_token(s: &str) -> bool {
    let has_letter = s.chars().any(|c| c.is_ascii_alphabetic());
    if !has_letter {
        return true;
    }
    let upper_count = s.chars().filter(|c| c.is_ascii_uppercase()).count();
    let has_lower = s.chars().any(|c| c.is_ascii_lowercase());
    upper_count >= 2 && !has_lower
}

/// `Incomparable` means neither dominates — e.g. `<Number> <Any>` vs `<Any> <Number>` against
/// an input that matches both.
#[derive(Debug, Eq, PartialEq, Clone, Copy)]
pub enum Specificity {
    StrictlyMore,
    StrictlyLess,
    Equal,
    Incomparable,
}

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

    /// Slot types are erased — same shape with different types lives in the same bucket and
    /// competes on specificity at dispatch time.
    pub fn untyped_key(&self) -> UntypedKey {
        self.elements
            .iter()
            .map(|el| match el {
                SignatureElement::Keyword(s) => UntypedElement::Keyword(s.clone()),
                SignatureElement::Argument(_) => UntypedElement::Slot,
            })
            .collect()
    }

    /// Uppercases lowercase fixed tokens so the bucket key matches what dispatch computes from
    /// incoming expressions. TODO(monadic-effects): emit a warning instead of silently
    /// rewriting once effects exist — rejecting would lose the "drop in a builtin without
    /// thinking about caps" affordance.
    pub fn normalize(&mut self) {
        for el in &mut self.elements {
            if let SignatureElement::Keyword(s) = el {
                if s.chars().any(|c| c.is_ascii_lowercase()) {
                    *s = s.to_ascii_uppercase();
                }
            }
        }
    }

    /// Assumes `self` and `other` share an `UntypedKey` (caller's responsibility) — only
    /// argument slots contribute, since fixed-token positions are equal by construction.
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

    /// Pairwise specificity tournament across a slice of co-bucket signatures. Returns
    /// `Some(i)` iff `candidates[i]` is strictly more specific than every other candidate
    /// (`StrictlyMore` against all peers, not `StrictlyMore | Equal` — `Equal` against any
    /// peer means there's a same-arg-type duplicate, which must surface as ambiguity rather
    /// than silently win). `None` for an empty slice or any no-clear-winner case; callers
    /// distinguish via `candidates.is_empty()`.
    pub fn most_specific(candidates: &[&ExpressionSignature]) -> Option<usize> {
        candidates
            .iter()
            .enumerate()
            .find(|(i, a)| {
                candidates.iter().enumerate().all(|(j, b)| {
                    *i == j || matches!(a.specificity_vs(b), Specificity::StrictlyMore)
                })
            })
            .map(|(i, _)| i)
    }
}

pub enum SignatureElement {
    Keyword(String),
    Argument(Argument),
}

/// `name` keys the slot in the `ArgumentBundle`; `ktype` gates what `ExpressionPart`s it
/// accepts.
pub struct Argument {
    pub name: String,
    pub ktype: KType,
}

impl Argument {
    pub fn matches(&self, part: &ExpressionPart<'_>) -> bool {
        self.ktype.accepts_part(part)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one_slot(kt: KType) -> ExpressionSignature {
        ExpressionSignature {
            return_type: KType::Any,
            elements: vec![SignatureElement::Argument(Argument {
                name: "v".into(),
                ktype: kt,
            })],
        }
    }

    #[test]
    fn most_specific_picks_number_over_any() {
        let any = one_slot(KType::Any);
        let num = one_slot(KType::Number);
        let cands: Vec<&ExpressionSignature> = vec![&any, &num];
        assert_eq!(ExpressionSignature::most_specific(&cands), Some(1));
    }

    #[test]
    fn most_specific_returns_none_for_empty() {
        let cands: Vec<&ExpressionSignature> = Vec::new();
        assert_eq!(ExpressionSignature::most_specific(&cands), None);
    }

    #[test]
    fn most_specific_returns_none_when_tied() {
        // Two `Number` overloads tie under `Equal` — ambiguity must surface, not a winner.
        let a = one_slot(KType::Number);
        let b = one_slot(KType::Number);
        let cands: Vec<&ExpressionSignature> = vec![&a, &b];
        assert_eq!(ExpressionSignature::most_specific(&cands), None);
    }
}
