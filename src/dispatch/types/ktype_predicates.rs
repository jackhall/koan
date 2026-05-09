//! Per-`ExpressionPart` admissibility and per-value type-tag checks for `KType`.
//! Specificity ordering for dispatch tie-breaking lives here too — these are the
//! predicates the dispatcher consults to decide whether a part fills a slot and
//! which of two viable candidates is more specific.

use super::ktype::KType;
use super::signature::{ExpressionSignature, SignatureElement};
use crate::dispatch::values::KObject;
use crate::parse::kexpression::{ExpressionPart, KLiteral};

impl KType {
    /// Specificity ordering for `specificity_vs`. Concrete types outrank `Any`; for parameterized
    /// containers, refinement of any inner slot makes the whole type more specific (covariant in
    /// element / key / value / arg / return positions). Strict — returns `false` for equal types.
    pub fn is_more_specific_than(&self, other: &KType) -> bool {
        use KType::*;
        if matches!(other, Any) && !matches!(self, Any) {
            return true;
        }
        match (self, other) {
            (List(a), List(b)) => a.is_more_specific_than(b),
            (Dict(ka, va), Dict(kb, vb)) => {
                let k_more = ka.is_more_specific_than(kb);
                let v_more = va.is_more_specific_than(vb);
                let k_eq = ka == kb;
                let v_eq = va == vb;
                (k_more && (v_more || v_eq)) || (k_eq && v_more)
            }
            (
                KFunction { args: aa, ret: ar },
                KFunction { args: ba, ret: br },
            ) if aa.len() == ba.len() => {
                let args_more = aa.iter().zip(ba.iter()).any(|(x, y)| x.is_more_specific_than(y));
                let args_eq = aa == ba;
                let ret_more = ar.is_more_specific_than(br);
                let ret_eq = ar == br;
                (args_more && (ret_more || ret_eq)) || (args_eq && ret_more)
            }
            _ => false,
        }
    }

    /// True iff a runtime `KObject` value satisfies this declared type. `Any` matches
    /// everything; container types recurse into element/key/value positions; function types
    /// require structural signature compatibility (a `KFuture` thunk is accepted because its
    /// result isn't known yet — full check deferred to runtime).
    pub fn matches_value(&self, obj: &KObject<'_>) -> bool {
        match self {
            KType::Any => true,
            KType::List(elem) => match obj {
                KObject::List(items) => items.iter().all(|x| elem.matches_value(x)),
                _ => false,
            },
            KType::Dict(k_ty, v_ty) => match obj {
                KObject::Dict(map) => map.iter().all(|(k_key, v_obj)| {
                    let k_t = k_key.ktype();
                    (matches!(k_ty.as_ref(), KType::Any) || **k_ty == k_t)
                        && v_ty.matches_value(v_obj)
                }),
                _ => false,
            },
            KType::KFunction { args, ret } => match obj {
                KObject::KFunction(f, _) => function_compat(&f.signature, args, ret),
                KObject::KFuture(_, _) => true,
                _ => false,
            },
            _ => *self == obj.ktype(),
        }
    }

    /// Per-`ExpressionPart` admissibility check: can a part of this shape fill an argument
    /// slot of this type? Container slots are shape-only at dispatch time — element-type
    /// validation for `List<Number>` etc. happens post-evaluation in `matches_value`, since
    /// lazy lists at dispatch time may carry unevaluated `Expression` parts. Function slots
    /// with a structural `KFunction { args, ret }` shape DO validate the bound function's
    /// signature here, since `KObject::KFunction` carries the full signature.
    ///
    /// The per-variant table is the dispatch-time admissibility check; `Argument::matches`
    /// is a thin delegate.
    pub fn accepts_part(&self, part: &ExpressionPart<'_>) -> bool {
        match self {
            KType::Any => true,
            KType::Number => matches!(
                part,
                ExpressionPart::Literal(KLiteral::Number(_))
                    | ExpressionPart::Future(KObject::Number(_))
            ),
            KType::Str => matches!(
                part,
                ExpressionPart::Literal(KLiteral::String(_))
                    | ExpressionPart::Future(KObject::KString(_))
            ),
            KType::Bool => matches!(
                part,
                ExpressionPart::Literal(KLiteral::Boolean(_))
                    | ExpressionPart::Future(KObject::Bool(_))
            ),
            KType::Null => matches!(
                part,
                ExpressionPart::Literal(KLiteral::Null) | ExpressionPart::Future(KObject::Null)
            ),
            KType::List(_) => matches!(
                part,
                ExpressionPart::ListLiteral(_) | ExpressionPart::Future(KObject::List(_))
            ),
            KType::Dict(_, _) => matches!(
                part,
                ExpressionPart::DictLiteral(_) | ExpressionPart::Future(KObject::Dict(_))
            ),
            KType::KFunction { args, ret } => match part {
                ExpressionPart::Future(KObject::KFunction(f, _)) => {
                    function_compat(&f.signature, args, ret)
                }
                ExpressionPart::Future(KObject::KFuture(_, _)) => true,
                _ => false,
            },
            KType::Identifier => matches!(part, ExpressionPart::Identifier(_)),
            KType::KExpression => matches!(part, ExpressionPart::Expression(_)),
            KType::TypeExprRef => matches!(
                part,
                ExpressionPart::Type(_) | ExpressionPart::Future(KObject::TypeExprValue(_))
            ),
            KType::Type => matches!(
                part,
                ExpressionPart::Future(KObject::TaggedUnionType(_))
                    | ExpressionPart::Future(KObject::StructType { .. })
            ),
            KType::Tagged => matches!(
                part,
                ExpressionPart::Future(KObject::Tagged { .. })
            ),
            KType::Struct => matches!(
                part,
                ExpressionPart::Future(KObject::Struct { .. })
            ),
            KType::ModuleType { .. } => match part {
                // A part filling a `ModuleType` slot must be a value whose runtime KType is
                // an exactly-equal `ModuleType` (same scope_id and name) — that's the
                // abstraction-barrier identity check. Today no value variant reports
                // `ModuleType`; this arm is reserved for stage-3 first-class module values
                // and falls through to false until then.
                ExpressionPart::Future(obj) => &obj.ktype() == self,
                _ => false,
            },
            KType::Module => matches!(part, ExpressionPart::Future(KObject::KModule(_))),
            KType::Signature => matches!(part, ExpressionPart::Future(KObject::KSignature(_))),
        }
    }
}

/// Structural function-type compatibility check. Returns true iff `sig`'s declared parameter
/// types and return type are equal (by KType structural equality) to the slot's expectations.
/// Strict equality, not subtyping — a function declared `(x: Number) -> Str` only fills a slot
/// typed `Function<(Number) -> Str>`, not `Function<(Any) -> Str>`. Subtype-aware function
/// matching (contravariant in args, covariant in ret) is a future refinement.
pub(super) fn function_compat(
    sig: &ExpressionSignature,
    args: &[KType],
    ret: &KType,
) -> bool {
    if sig.return_type != *ret {
        return false;
    }
    let mut i = 0;
    for el in &sig.elements {
        if let SignatureElement::Argument(a) = el {
            if i >= args.len() || a.ktype != args[i] {
                return false;
            }
            i += 1;
        }
    }
    i == args.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_more_specific_concrete_beats_any() {
        assert!(KType::Number.is_more_specific_than(&KType::Any));
        assert!(!KType::Any.is_more_specific_than(&KType::Number));
    }

    #[test]
    fn is_more_specific_list_number_beats_list_any() {
        let n = KType::List(Box::new(KType::Number));
        let a = KType::List(Box::new(KType::Any));
        assert!(n.is_more_specific_than(&a));
        assert!(!a.is_more_specific_than(&n));
    }

    #[test]
    fn is_more_specific_disjoint_lists_incomparable() {
        let n = KType::List(Box::new(KType::Number));
        let s = KType::List(Box::new(KType::Str));
        assert!(!n.is_more_specific_than(&s));
        assert!(!s.is_more_specific_than(&n));
    }

    #[test]
    fn is_more_specific_dict_refines_value() {
        let strict = KType::Dict(Box::new(KType::Str), Box::new(KType::Number));
        let loose = KType::Dict(Box::new(KType::Str), Box::new(KType::Any));
        assert!(strict.is_more_specific_than(&loose));
        assert!(!loose.is_more_specific_than(&strict));
    }

    #[test]
    fn is_more_specific_function_arity_mismatch_incomparable() {
        let unary = KType::KFunction {
            args: vec![KType::Number],
            ret: Box::new(KType::Number),
        };
        let nullary = KType::KFunction {
            args: vec![],
            ret: Box::new(KType::Number),
        };
        assert!(!unary.is_more_specific_than(&nullary));
        assert!(!nullary.is_more_specific_than(&unary));
    }
}
