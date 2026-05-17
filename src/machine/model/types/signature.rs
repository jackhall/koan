//! Expression-signature machinery: the call shape a `KFunction` matches against — an ordered
//! mix of fixed `Keyword` tokens and typed `Argument` slots, plus a `return_type`.
//! `UntypedKey` groups overloads by shape; `Specificity` ranks candidates within a bucket.
//!
//! Not to be confused with the **module-signature** type (`SIG`-declared) at
//! [`crate::machine::model::values::module::Signature`].
//!
//! The `return_type` field is a [`ReturnType`] rather than a bare [`KType`] so functor
//! return types that reference a per-call parameter (`-> Er`, `-> (MODULE_TYPE_OF Er Type)`,
//! `-> (SIG_WITH Set ((Elt: Er)))`) survive FN-definition without sub-dispatching against
//! the outer scope. The `Resolved(KType)` variant covers every non-templated case —
//! builtins, user-defined FNs whose return type doesn't reference any parameter, every
//! historical site — and `Deferred(DeferredReturn)` carries the parser- or
//! expression-preserved form for per-call re-elaboration at the dispatch boundary.

use crate::machine::model::ast::{ExpressionPart, KExpression, TypeExpr};

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
/// design/type-system.md](../../../../design/type-system.md#token-classes--the-parser-level-foundation):
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
/// (see [`crate::builtins::fn_def`]) detected at least one leaf matching a
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
    use crate::machine::model::ast::TypeParams;
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
    /// [`crate::machine::execute::scheduler::execute::Scheduler::execute`]
    /// skips the per-slot check for `Deferred(_)` to avoid a redundant (and
    /// always-passing) `Any`-style accept here.
    pub fn matches_value(&self, obj: &crate::machine::model::values::KObject<'_>) -> bool {
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
    use crate::machine::model::ast::TypeParams;
    use std::cell::OnceCell;

    fn one_slot<'a>(kt: KType) -> ExpressionSignature<'a> {
        ExpressionSignature {
            return_type: ReturnType::Resolved(KType::Any),
            elements: vec![SignatureElement::Argument(Argument {
                name: "v".into(),
                ktype: kt,
            })],
        }
    }

    fn list_te(name: &str, items: Vec<TypeExpr>) -> TypeExpr {
        TypeExpr {
            name: name.into(),
            params: TypeParams::List(items),
            builtin_cache: OnceCell::new(),
        }
    }

    fn fn_te(args: Vec<TypeExpr>, ret: TypeExpr) -> TypeExpr {
        TypeExpr {
            name: "Function".into(),
            params: TypeParams::Function { args, ret: Box::new(ret) },
            builtin_cache: OnceCell::new(),
        }
    }

    fn expr_with_keyword<'a>(kw: &str) -> KExpression<'a> {
        KExpression { parts: vec![ExpressionPart::Keyword(kw.into())] }
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

    #[test]
    fn return_type_clone_round_trips_all_arms() {
        // Resolved arm
        let r = ReturnType::Resolved(KType::Number);
        assert_eq!(r, r.clone());
        // Deferred(TypeExpr) arm — also exercises DeferredReturn::clone TypeExpr arm
        let d = ReturnType::Deferred(DeferredReturn::TypeExpr(TypeExpr::leaf("Er".into())));
        assert_eq!(d, d.clone());
        // Deferred(Expression) arm — exercises DeferredReturn::clone Expression arm
        let e = ReturnType::Deferred(DeferredReturn::Expression(expr_with_keyword("FOO")));
        assert_eq!(e, e.clone());
    }

    #[test]
    fn return_type_eq_deferred_match_and_variant_mismatch() {
        let r = ReturnType::Resolved(KType::Number);
        let d = ReturnType::Deferred(DeferredReturn::TypeExpr(TypeExpr::leaf("Er".into())));
        // Variant-mismatch `_ => false` arm.
        assert_ne!(r, d);
        // Deferred==Deferred arm — same payload.
        let d2 = ReturnType::Deferred(DeferredReturn::TypeExpr(TypeExpr::leaf("Er".into())));
        assert_eq!(d, d2);
        // Deferred==Deferred arm — different payload.
        let d3 = ReturnType::Deferred(DeferredReturn::TypeExpr(TypeExpr::leaf("Other".into())));
        assert_ne!(d, d3);
    }

    #[test]
    fn deferred_return_eq_matches_per_carrier() {
        let t1 = DeferredReturn::TypeExpr(TypeExpr::leaf("Er".into()));
        let t2 = DeferredReturn::TypeExpr(TypeExpr::leaf("Er".into()));
        let t3 = DeferredReturn::TypeExpr(TypeExpr::leaf("Other".into()));
        assert_eq!(t1, t2);
        assert_ne!(t1, t3);

        let e1 = DeferredReturn::Expression(expr_with_keyword("FOO"));
        let e2 = DeferredReturn::Expression(expr_with_keyword("FOO"));
        let e3 = DeferredReturn::Expression(expr_with_keyword("BAR"));
        assert_eq!(e1, e2);
        assert_ne!(e1, e3);

        // Variant-mismatch `_ => false` arm.
        assert_ne!(t1, e1);
    }

    #[test]
    fn type_expr_eq_covers_all_param_arms() {
        // Leaf (None vs None) — name match and name mismatch.
        let leaf_a = TypeExpr::leaf("A".into());
        let leaf_a2 = TypeExpr::leaf("A".into());
        let leaf_b = TypeExpr::leaf("B".into());
        assert!(type_expr_eq(&leaf_a, &leaf_a2));
        assert!(!type_expr_eq(&leaf_a, &leaf_b));

        // List structural equality, element mismatch, and arity mismatch.
        let list_a = list_te("List", vec![TypeExpr::leaf("A".into())]);
        let list_a2 = list_te("List", vec![TypeExpr::leaf("A".into())]);
        let list_diff = list_te("List", vec![TypeExpr::leaf("X".into())]);
        let list_two = list_te(
            "List",
            vec![TypeExpr::leaf("A".into()), TypeExpr::leaf("B".into())],
        );
        assert!(type_expr_eq(&list_a, &list_a2));
        assert!(!type_expr_eq(&list_a, &list_diff));
        assert!(!type_expr_eq(&list_a, &list_two));

        // Function structural equality, arg mismatch, and return-type mismatch.
        let fn_a = fn_te(vec![TypeExpr::leaf("A".into())], TypeExpr::leaf("R".into()));
        let fn_a2 = fn_te(vec![TypeExpr::leaf("A".into())], TypeExpr::leaf("R".into()));
        let fn_arg_diff =
            fn_te(vec![TypeExpr::leaf("X".into())], TypeExpr::leaf("R".into()));
        let fn_ret_diff =
            fn_te(vec![TypeExpr::leaf("A".into())], TypeExpr::leaf("X".into()));
        let fn_arity = fn_te(
            vec![TypeExpr::leaf("A".into()), TypeExpr::leaf("B".into())],
            TypeExpr::leaf("R".into()),
        );
        assert!(type_expr_eq(&fn_a, &fn_a2));
        assert!(!type_expr_eq(&fn_a, &fn_arg_diff));
        assert!(!type_expr_eq(&fn_a, &fn_ret_diff));
        assert!(!type_expr_eq(&fn_a, &fn_arity));

        // Variant-mismatch `_ => false` arm. Same name across both sides so the
        // name short-circuit at the top of `type_expr_eq` doesn't pre-empt the
        // params-shape fallthrough.
        let same_name_leaf = TypeExpr::leaf("Shape".into());
        let same_name_list = list_te("Shape", vec![TypeExpr::leaf("A".into())]);
        let same_name_fn =
            TypeExpr {
                name: "Shape".into(),
                params: TypeParams::Function {
                    args: vec![TypeExpr::leaf("A".into())],
                    ret: Box::new(TypeExpr::leaf("R".into())),
                },
                builtin_cache: OnceCell::new(),
            };
        assert!(!type_expr_eq(&same_name_leaf, &same_name_list));
        assert!(!type_expr_eq(&same_name_list, &same_name_fn));
    }

    #[test]
    fn expression_signature_matches_rejects_length_and_keyword_part_mismatches() {
        // Length mismatch arm: sig has 1 element, expr has 0 parts.
        let sig = ExpressionSignature {
            return_type: ReturnType::Resolved(KType::Any),
            elements: vec![SignatureElement::Keyword("FOO".into())],
        };
        let empty: KExpression<'_> = KExpression { parts: vec![] };
        assert!(!sig.matches(&empty));

        // Keyword-slot vs non-Keyword part arm: sig expects keyword, expr supplies a literal.
        let mismatched = KExpression {
            parts: vec![ExpressionPart::Literal(
                crate::machine::model::ast::KLiteral::Number(1.0),
            )],
        };
        assert!(!sig.matches(&mismatched));

        // Sanity: matching keyword at the right position still accepts.
        let matching = KExpression {
            parts: vec![ExpressionPart::Keyword("FOO".into())],
        };
        assert!(sig.matches(&matching));
    }

    #[test]
    fn return_type_debug_renders_both_arms() {
        let r = ReturnType::Resolved(KType::Number);
        assert!(format!("{:?}", r).contains("Resolved"));
        let d = ReturnType::Deferred(DeferredReturn::TypeExpr(TypeExpr::leaf("Er".into())));
        assert!(format!("{:?}", d).contains("Deferred"));
    }

    #[test]
    fn deferred_return_debug_renders_both_arms() {
        let t = DeferredReturn::TypeExpr(TypeExpr::leaf("Er".into()));
        assert!(format!("{:?}", t).contains("TypeExpr"));
        let e = DeferredReturn::Expression(expr_with_keyword("FOO"));
        assert!(format!("{:?}", e).contains("Expression"));
    }

    #[test]
    fn return_type_name_covers_all_arms() {
        // Resolved delegates to KType::name.
        let r = ReturnType::Resolved(KType::Number);
        assert_eq!(r.name(), KType::Number.name());
        // Deferred(TypeExpr) renders the surface name via TypeExpr::render.
        let t = ReturnType::Deferred(DeferredReturn::TypeExpr(TypeExpr::leaf("Er".into())));
        assert_eq!(t.name(), "Er");
        // Deferred(Expression) renders via KExpression::summarize.
        let e = ReturnType::Deferred(DeferredReturn::Expression(expr_with_keyword("FOO")));
        assert_eq!(e.name(), "FOO");
    }

    #[test]
    fn return_type_matches_value_deferred_always_true_resolved_delegates() {
        use crate::machine::model::values::KObject;
        let obj = KObject::Number(42.0);
        // Deferred arm: always true — per-call check runs elsewhere.
        let d = ReturnType::Deferred(DeferredReturn::TypeExpr(TypeExpr::leaf("Er".into())));
        assert!(d.matches_value(&obj));
        assert!(!d.is_resolved());
        // Resolved arm: delegates to KType::matches_value.
        let r_num = ReturnType::Resolved(KType::Number);
        assert!(r_num.matches_value(&obj));
        assert!(r_num.is_resolved());
        let r_bool = ReturnType::Resolved(KType::Bool);
        assert!(!r_bool.matches_value(&obj));
    }
}
