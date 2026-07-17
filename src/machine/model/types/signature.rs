//! Expression-signature machinery: the call shape a `KFunction` matches against — an ordered
//! mix of fixed `Keyword` tokens and typed `Argument` slots, plus a `return_type`.
//! `UntypedKey` groups overloads by shape; `Specificity` ranks candidates within a bucket.
//!
//! Not to be confused with the **module-signature** type (`SIG`-declared) at
//! [`crate::machine::model::values::module::ModuleSignature`].
//!
//! `return_type` is a [`ReturnType`] rather than a bare [`KType`] so return types that
//! reference a per-call parameter (`-> er`, `-> er.Carrier`) survive FN-definition without
//! sub-dispatching against the outer scope.

use crate::machine::model::ast::{ExpressionPart, KExpression, TypeIdentifier};

use super::ktraits::Parseable;
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

/// True iff `s` classifies as a keyword (fixed token). See
/// [tokens.md](../../../../design/typing/tokens.md): pure-symbol tokens (no ASCII letters)
/// are always keywords; alphabetic tokens are keywords iff they have ≥2 ASCII-uppercase
/// letters and no ASCII-lowercase letters.
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

pub struct ExpressionSignature<'a> {
    pub return_type: ReturnType<'a>,
    pub elements: Vec<SignatureElement<'a>>,
}

/// Carrier for an FN's declared return type. The surface admits parameter-name references
/// in return-type position (`FN (LIFT er: Ordered) -> er = ...`); `Deferred` holds the
/// captured surface form for per-call re-elaboration against the per-call scope where the
/// parameter's type-language identity is registered. See
/// [functors.md](../../../../design/typing/functors.md).
pub enum ReturnType<'a> {
    Resolved(KType<'a>),
    Deferred(DeferredReturn<'a>),
}

/// Surface form preserved for per-call re-elaboration. Two carriers mirror the two FN
/// return-type slot kinds:
///
/// - `Type` — parser-preserved structured form (`er`, `List<er>`). Re-elaborated per
///   call via `elaborate_type_identifier`. Owns its strings, so no region lifetime.
/// - `Expression` — captured `:(…)` / dotted return expression (`er.Carrier`,
///   `Set WITH {…}`). Re-runs as a sub-Dispatch under the per-call scope; the resulting
///   `Carried::Type`'s inner `KType` is the per-call return type.
pub enum DeferredReturn<'a> {
    Type(TypeIdentifier),
    Expression(KExpression<'a>),
}

/// Hashable type-language shadow of a [`DeferredReturn`], stored inside
/// `KType::DeferredReturn`. The `Expression` carrier holds the canonical `summarize()`
/// render — NOT the live `KExpression`, which impls neither `Eq` nor `Hash`. Identity is
/// syntactic, matching `DeferredReturn`'s own `PartialEq` (`Type` by name, `Expression`
/// by canonical render), so a synthesized `KType::DeferredReturn` ret slot compares,
/// hashes, and ranks by the same surface form `ExpressionSignature::exact_equal` uses.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum DeferredReturnSurface {
    Type(TypeIdentifier),
    Expression(String),
}

impl DeferredReturnSurface {
    pub fn from_deferred(d: &DeferredReturn<'_>) -> Self {
        match d {
            DeferredReturn::Type(t) => Self::Type(t.clone()),
            DeferredReturn::Expression(e) => Self::Expression(e.summarize()),
        }
    }

    pub fn render(&self) -> String {
        match self {
            Self::Type(t) => t.render(),
            Self::Expression(s) => s.clone(),
        }
    }
}

impl<'a> Clone for ReturnType<'a> {
    fn clone(&self) -> Self {
        match self {
            ReturnType::Resolved(kt) => ReturnType::Resolved(kt.clone()),
            ReturnType::Deferred(d) => ReturnType::Deferred(d.clone()),
        }
    }
}

impl<'a> Clone for DeferredReturn<'a> {
    fn clone(&self) -> Self {
        match self {
            DeferredReturn::Type(t) => DeferredReturn::Type(t.clone()),
            DeferredReturn::Expression(e) => DeferredReturn::Expression(e.clone()),
        }
    }
}

impl<'a> PartialEq for ReturnType<'a> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (ReturnType::Resolved(a), ReturnType::Resolved(b)) => a == b,
            (ReturnType::Deferred(a), ReturnType::Deferred(b)) => a == b,
            _ => false,
        }
    }
}

impl<'a> Eq for ReturnType<'a> {}

impl<'a> PartialEq for DeferredReturn<'a> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (DeferredReturn::Type(a), DeferredReturn::Type(b)) => a == b,
            (DeferredReturn::Expression(a), DeferredReturn::Expression(b)) => {
                // Structural syntax equality over the two captured expressions — the same walk
                // `==` runs on a quoted value. A banned-shape splice inside a deferred return
                // conservatively counts as a distinct overload (`Err` → not equal).
                crate::machine::model::values::expression_equal(a, b).unwrap_or(false)
            }
            _ => false,
        }
    }
}

impl<'a> std::fmt::Debug for ReturnType<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReturnType::Resolved(kt) => f.debug_tuple("Resolved").field(kt).finish(),
            ReturnType::Deferred(d) => f.debug_tuple("Deferred").field(d).finish(),
        }
    }
}

impl<'a> std::fmt::Debug for DeferredReturn<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeferredReturn::Type(t) => f.debug_tuple("Type").field(t).finish(),
            DeferredReturn::Expression(e) => {
                f.debug_tuple("Expression").field(&e.summarize()).finish()
            }
        }
    }
}

impl<'a> ReturnType<'a> {
    /// Surface name for diagnostics.
    pub fn name(&self) -> String {
        match self {
            ReturnType::Resolved(kt) => kt.name(),
            ReturnType::Deferred(DeferredReturn::Type(t)) => t.render(),
            ReturnType::Deferred(DeferredReturn::Expression(e)) => e.summarize(),
        }
    }

    /// Lift-time return-type check. `Deferred` returns `true` — the real slot check
    /// runs in the per-call elaboration's dep-finish, where the resolved `KType`
    /// is available.
    pub fn matches_value(&self, obj: &crate::machine::model::values::KObject<'a>) -> bool {
        match self {
            ReturnType::Resolved(kt) => kt.matches_value(obj),
            ReturnType::Deferred(_) => true,
        }
    }

    pub fn is_resolved(&self) -> bool {
        matches!(self, ReturnType::Resolved(_))
    }
}

impl<'a> ExpressionSignature<'a> {
    pub fn matches<'e>(&self, expr: &KExpression<'e>) -> bool {
        if self.elements.len() != expr.parts.len() {
            return false;
        }
        self.elements
            .iter()
            .zip(&expr.parts)
            .all(|(el, part)| match (el, &part.value) {
                (SignatureElement::Keyword(s), ExpressionPart::Keyword(t)) => s == t,
                (SignatureElement::Keyword(_), _) => false,
                (SignatureElement::Argument(arg), part_value) => arg.matches(part_value),
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

    /// Uppercases lowercase fixed tokens so the bucket key matches what dispatch computes
    /// from incoming expressions. TODO(monadic-effects): emit a warning instead of silently
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

    /// Assumes `self` and `other` share an `UntypedKey` — only argument slots contribute,
    /// since fixed-token positions are equal by construction.
    pub fn specificity_vs(&self, other: &ExpressionSignature<'a>) -> Specificity {
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

    /// Pairwise specificity tournament across co-bucket signatures. Returns `Some(i)` iff
    /// `candidates[i]` is `StrictlyMore` than every peer — `Equal` against any peer means a
    /// same-arg-type duplicate, which must surface as ambiguity rather than silently win.
    pub fn most_specific(candidates: &[&ExpressionSignature<'a>]) -> Option<usize> {
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

    /// Structural equality on shape + per-`Argument` `KType` + return type. Independent of
    /// `Argument::name` — two overloads with matching shape and types collide for dispatch
    /// regardless of parameter naming.
    pub fn exact_equal(&self, other: &ExpressionSignature<'a>) -> bool {
        if self.return_type != other.return_type {
            return false;
        }
        if self.elements.len() != other.elements.len() {
            return false;
        }
        self.elements
            .iter()
            .zip(other.elements.iter())
            .all(|(x, y)| match (x, y) {
                (SignatureElement::Keyword(s), SignatureElement::Keyword(t)) => s == t,
                (SignatureElement::Argument(ax), SignatureElement::Argument(ay)) => {
                    ax.ktype == ay.ktype
                }
                _ => false,
            })
    }
}

pub enum SignatureElement<'a> {
    Keyword(String),
    Argument(Argument<'a>),
}

/// `name` keys the slot in the bound argument record; `ktype` gates what `ExpressionPart`s it
/// accepts. `'a` because the declared `KType` may reference region-pinned `Module` /
/// `ModuleSignature` carriers.
pub struct Argument<'a> {
    pub name: String,
    pub ktype: KType<'a>,
}

impl<'a> Argument<'a> {
    pub fn matches<'e>(&self, part: &ExpressionPart<'e>) -> bool {
        self.ktype.accepts_part(part)
    }
}

#[cfg(test)]
mod tests;
