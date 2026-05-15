//! Expression-signature machinery: the call shape a `KFunction` matches against — an ordered
//! mix of fixed `Keyword` tokens and typed `Argument` slots, plus a `return_type`.
//! `UntypedKey` groups overloads by shape; `Specificity` ranks candidates within a bucket.
//!
//! Not to be confused with the **module-signature** type (`SIG`-declared) at
//! [`crate::runtime::machine::model::values::module::Signature`].
//!
//! The `return_type` field is a [`ReturnType`] rather than a bare [`KType`] so functor
//! return types that reference a per-call parameter (`-> Er`, `-> (MODULE_TYPE_OF Er Type)`,
//! `-> (SIG_WITH Set ((Elt: Er)))`) survive FN-definition without sub-dispatching against
//! the outer scope. The `Resolved(KType)` variant covers every non-templated case —
//! builtins, user-defined FNs whose return type doesn't reference any parameter, every
//! historical site — and `Deferred(DeferredReturn)` carries the parser- or
//! expression-preserved form for per-call re-elaboration at the dispatch boundary.

use crate::runtime::machine::model::ast::{ExpressionPart, KExpression, TypeExpr};

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

pub struct ExpressionSignature<'a> {
    pub return_type: ReturnType<'a>,
    pub elements: Vec<SignatureElement>,
}

/// Carrier for an FN's declared return type. The shipped surface admits parameter-name
/// references in return-type position (`FN (LIFT Er: OrderedSig) -> Er = ...`); per-call
/// elaboration runs as a sibling Combine of the body and joins for the lift-time slot
/// check. See [module-system functors][1].
///
/// `Resolved(KType)` is the static case — the return type is fully elaborated at
/// FN-definition (or builtin-registration) time. Every builtin and every user-defined FN
/// whose return type doesn't reference a parameter rides this arm.
///
/// `Deferred(DeferredReturn)` is the per-call case. The FN body's parameter-name scan
/// (see [`crate::runtime::builtins::fn_def`]) detected at least one leaf matching a
/// parameter; the captured surface form is held verbatim so the dispatch boundary can
/// re-elaborate against the per-call scope where Stage A's dual-write has installed the
/// parameter's type-language identity.
///
/// [1]: ../../../design/module-system.md#functors
pub enum ReturnType<'a> {
    Resolved(KType),
    Deferred(DeferredReturn<'a>),
}

/// Surface form preserved for per-call re-elaboration. Two carriers, mirroring the two
/// FN overloads (one whose return-type slot is `TypeExprRef`, one whose slot is
/// `KExpression`):
///
/// - `TypeExpr(TypeExpr)` — parser-preserved structured form. Bare leaves (`Er`) or
///   parameterized leaves (`List<Er>`, `Wrap<Er>`). Re-elaborated per call via
///   `elaborate_type_expr` against the per-call scope. The carrier is `'static` because
///   `TypeExpr` itself owns its strings — no arena lifetime to thread.
/// - `Expression(KExpression<'a>)` — captured parens-form expression
///   (`(MODULE_TYPE_OF Er Type)`, `(SIG_WITH Set ((Elt: Er)))`). Re-runs as a
///   sub-Dispatch under the per-call scope; the resulting `KTypeValue`'s inner `KType`
///   is the per-call return type. Lifetime `'a` matches `KFunction::body` — same
///   arena-anchored carrier discipline.
pub enum DeferredReturn<'a> {
    TypeExpr(TypeExpr),
    Expression(KExpression<'a>),
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
            DeferredReturn::TypeExpr(t) => DeferredReturn::TypeExpr(t.clone()),
            DeferredReturn::Expression(e) => DeferredReturn::Expression(e.clone()),
        }
    }
}

impl<'a> PartialEq for ReturnType<'a> {
    /// Variant + payload equality. Two `Resolved` are equal iff their `KType`s match;
    /// two `Deferred` are equal iff their carrier variants and payloads match
    /// structurally. Used by `signatures_exact_equal` to flag overload duplicates — two
    /// FN-defs whose return-type carriers are byte-identical are interchangeable for
    /// dispatch, so the `DuplicateOverload` semantic still applies.
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
            (DeferredReturn::TypeExpr(a), DeferredReturn::TypeExpr(b)) => {
                type_expr_eq(a, b)
            }
            (DeferredReturn::Expression(a), DeferredReturn::Expression(b)) => {
                // Render-based structural equality. `KExpression` doesn't impl `Eq` and
                // a deep walk over `ExpressionPart` (which carries `&'a KObject` futures)
                // would have to handle pointer-equal Future entries. `summarize()` is the
                // existing canonical-rendering helper and is sufficient for duplicate
                // overload detection — the rendered form encodes structural identity.
                a.summarize() == b.summarize()
            }
            _ => false,
        }
    }
}

fn type_expr_eq(a: &TypeExpr, b: &TypeExpr) -> bool {
    if a.name != b.name {
        return false;
    }
    use crate::runtime::machine::model::ast::TypeParams;
    match (&a.params, &b.params) {
        (TypeParams::None, TypeParams::None) => true,
        (TypeParams::List(xs), TypeParams::List(ys)) => {
            xs.len() == ys.len() && xs.iter().zip(ys.iter()).all(|(x, y)| type_expr_eq(x, y))
        }
        (
            TypeParams::Function { args: ax, ret: ar },
            TypeParams::Function { args: bx, ret: br },
        ) => {
            ax.len() == bx.len()
                && ax.iter().zip(bx.iter()).all(|(x, y)| type_expr_eq(x, y))
                && type_expr_eq(ar, br)
        }
        _ => false,
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
            DeferredReturn::TypeExpr(t) => f.debug_tuple("TypeExpr").field(t).finish(),
            DeferredReturn::Expression(e) => {
                f.debug_tuple("Expression").field(&e.summarize()).finish()
            }
        }
    }
}

impl<'a> ReturnType<'a> {
    /// Surface name for diagnostics. `Resolved` delegates to `KType::name`; `Deferred`
    /// renders the carrier's surface form (the `TypeExpr` or `KExpression` summary).
    pub fn name(&self) -> String {
        match self {
            ReturnType::Resolved(kt) => kt.name(),
            ReturnType::Deferred(DeferredReturn::TypeExpr(t)) => t.render(),
            ReturnType::Deferred(DeferredReturn::Expression(e)) => e.summarize(),
        }
    }

    /// Lift-time return-type check. `Resolved` delegates to `KType::matches_value` —
    /// the existing static check applies unchanged. `Deferred` returns `true` here:
    /// the actual slot check moves into the per-call elaboration's Combine finish,
    /// where the resolved `KType` is available. The lift-time site at
    /// [`crate::runtime::machine::execute::scheduler::execute::Scheduler::execute`]
    /// skips the per-slot check for `Deferred(_)` to avoid a redundant (and
    /// always-passing) `Any`-style accept here.
    pub fn matches_value(&self, obj: &crate::runtime::machine::model::values::KObject<'_>) -> bool {
        match self {
            ReturnType::Resolved(kt) => kt.matches_value(obj),
            ReturnType::Deferred(_) => true,
        }
    }

    /// Convenience: `true` iff this is `Resolved(_)`. Used by the lift-time check at
    /// `execute.rs` to decide whether to run the slot check inline or skip in favour of
    /// the Combine finish's per-call check.
    pub fn is_resolved(&self) -> bool {
        matches!(self, ReturnType::Resolved(_))
    }
}

impl<'a> ExpressionSignature<'a> {
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
    pub fn specificity_vs(&self, other: &ExpressionSignature<'_>) -> Specificity {
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
    pub fn most_specific(candidates: &[&ExpressionSignature<'_>]) -> Option<usize> {
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

    fn one_slot<'a>(kt: KType) -> ExpressionSignature<'a> {
        ExpressionSignature {
            return_type: ReturnType::Resolved(KType::Any),
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
        let cands: Vec<&ExpressionSignature<'_>> = vec![&any, &num];
        assert_eq!(ExpressionSignature::most_specific(&cands), Some(1));
    }

    #[test]
    fn most_specific_returns_none_for_empty() {
        let cands: Vec<&ExpressionSignature<'_>> = Vec::new();
        assert_eq!(ExpressionSignature::most_specific(&cands), None);
    }

    #[test]
    fn most_specific_returns_none_when_tied() {
        // Two `Number` overloads tie under `Equal` — ambiguity must surface, not a winner.
        let a = one_slot(KType::Number);
        let b = one_slot(KType::Number);
        let cands: Vec<&ExpressionSignature<'_>> = vec![&a, &b];
        assert_eq!(ExpressionSignature::most_specific(&cands), None);
    }
}
