//! Per-`ExpressionPart` admissibility and per-value type-tag checks for `KType`.
//! Specificity ordering for dispatch tie-breaking lives here too — these are the
//! predicates the dispatcher consults to decide whether a part fills a slot and
//! which of two viable candidates is more specific.

use super::ktype::{KType, UserTypeKind};
use super::signature::{ExpressionSignature, SignatureElement};
use crate::runtime::model::values::KObject;
use crate::ast::{ExpressionPart, KLiteral};

impl KType {
    /// Specificity ordering for `specificity_vs`. Concrete types outrank `Any`; for parameterized
    /// containers, refinement of any inner slot makes the whole type more specific (covariant in
    /// element / key / value / arg / return positions). Strict — returns `false` for equal types.
    pub fn is_more_specific_than(&self, other: &KType) -> bool {
        use KType::*;
        // `AnyUserType` vs `Any` is already covered by this prefix — `AnyUserType` is
        // non-`Any`, so it lands here and returns `true` without needing a dedicated arm.
        // The dedicated `(UserType, AnyUserType)` arm below covers the strict refinement
        // direction inside the user-declared family.
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
            // SignatureBound strictly refines Module: a sig-typed slot is a refinement of
            // "any module." Two SignatureBounds with different sig_ids are incomparable —
            // they're disjoint slot types — so this predicate stays `false` for that case
            // by falling through to the wildcard.
            (SignatureBound { .. }, Module) => true,
            // A per-declaration `UserType { kind, .. }` strictly refines the wildcard
            // `AnyUserType { kind }` of the same kind. Different-kind pairs fall through
            // to the wildcard `false`, leaving them correctly incomparable. Two
            // `UserType`s of the same kind but different `(scope_id, name)` are
            // incomparable — same-kind siblings are sibling refinements of `AnyUserType`.
            (UserType { kind: a, .. }, AnyUserType { kind: b }) if a == b => true,
            // Phase 1: `Mu`-vs-`Mu` with the same binder name recurses on bodies. Different
            // binders are incomparable. Real cycle-aware structural ordering is a phase-3+
            // concern; phase 1 only needs the trivial reflexive shape so the variants don't
            // poison existing specificity decisions.
            (Mu { binder: ba, body: a }, Mu { binder: bb, body: b }) if ba == bb => {
                a.is_more_specific_than(b)
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
            // FN-return-type check: a FN declared `-> OrderedSig` whose body produces a
            // module that hasn't been ascribed to OrderedSig errors at the slot's Done arm.
            // Mirror of `accepts_part`'s SignatureBound arm.
            KType::SignatureBound { sig_id, .. } => match obj {
                KObject::KModule(m, _) => m.compatible_sigs.borrow().contains(sig_id),
                _ => false,
            },
            // Wildcard kind check for user-declared types. Mirrors the `accepts_part`
            // arm — admit any carrier of the matching family regardless of declaring
            // schema. Inert in 3.0 (no slot is typed `AnyUserType` until `from_name` is
            // rewired in 3.0b) but pinned now so 3.0b's `from_name` flip is a one-line
            // change with the predicate already correct.
            KType::AnyUserType { kind } => matches!(
                (kind, obj),
                (UserTypeKind::Struct, KObject::Struct { .. })
                    | (UserTypeKind::Tagged, KObject::Tagged { .. })
                    | (UserTypeKind::Module, KObject::KModule(_, _))
            ),
            // Phase 1: one-unfold check. Cycle-gating (a threaded "currently unfolding" set)
            // is a phase-3 concern; today no runtime value carries a `RecursiveRef` so the
            // unfold can't actually recurse onto one.
            KType::Mu { body, .. } => body.matches_value(obj),
            // Phase 1: cycle gate. Inside a `Mu` body the recursive back-edge accepts
            // anything; phase 3 will tighten this by carrying the enclosing `Mu`'s body
            // through the predicate's call frame.
            KType::RecursiveRef(_) => true,
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
                ExpressionPart::Type(_) | ExpressionPart::Future(KObject::KTypeValue(_))
            ),
            KType::Type => matches!(
                part,
                ExpressionPart::Future(KObject::TaggedUnionType { .. })
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
            // Per-declaration identity: a slot typed with a concrete `UserType { kind,
            // scope_id, name }` accepts only a `Future(KObject)` value whose `ktype()`
            // reports the same `UserType`. Inert in stage 3.0 — no carrier reports a
            // `UserType` from `ktype()` yet — but the arm lands correctly so the 3.1
            // variant collapse is a pure rewire of the value carriers' `ktype()` arms.
            KType::UserType { .. } => {
                matches!(part, ExpressionPart::Future(obj) if &obj.ktype() == self)
            }
            // Wildcard "any user-declared X" slot: the `kind` discriminator selects which
            // family of carriers we admit (`Struct`/`Tagged`/`Module`). Lands the 3.0b
            // surface-name rewire — `Struct`/`Tagged`/`Module` in `from_name` now map
            // here — so existing dispatch tests using `(PICK x: Struct)` continue to
            // accept any struct carrier regardless of declaring schema.
            KType::AnyUserType { kind } => match part {
                ExpressionPart::Future(obj) => matches!(
                    (kind, obj),
                    (UserTypeKind::Struct, KObject::Struct { .. })
                        | (UserTypeKind::Tagged, KObject::Tagged { .. })
                        | (UserTypeKind::Module, KObject::KModule(_, _))
                ),
                _ => false,
            },
            KType::ModuleType { .. } => match part {
                // A part filling a `ModuleType` slot must be a value whose runtime KType is
                // an exactly-equal `ModuleType` (same scope_id and name) — that's the
                // abstraction-barrier identity check. Today no value variant reports
                // `ModuleType`; this arm is reserved for stage-3 first-class module values
                // and falls through to false until then.
                ExpressionPart::Future(obj) => &obj.ktype() == self,
                _ => false,
            },
            KType::Module => matches!(part, ExpressionPart::Future(KObject::KModule(_, _))),
            // O(1) per-sig admissibility: a `Future(KModule)` fills a sig-typed slot iff
            // its ascription-populated `compatible_sigs` set carries the slot's `sig_id`.
            // Unascribed source modules never match (their compat set is empty); pass them
            // through `:|` / `:!` first. Bare-name arguments are routed through value
            // lookup (LET-bound to a lowercase identifier) so they enter as Identifier
            // tokens which the auto-wrap pass converts to sub-Dispatches that resolve
            // to the module value before re-entering this slot.
            KType::SignatureBound { sig_id, .. } => match part {
                ExpressionPart::Future(KObject::KModule(m, _)) => {
                    m.compatible_sigs.borrow().contains(sig_id)
                }
                _ => false,
            },
            KType::Signature => matches!(part, ExpressionPart::Future(KObject::KSignature(_))),
            // Phase 1: same one-unfold rule as `matches_value`.
            KType::Mu { body, .. } => body.accepts_part(part),
            // Phase 1: cycle gate — accept anything until phase 3 introduces a threaded
            // unfold set.
            KType::RecursiveRef(_) => true,
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

    #[test]
    fn mu_matches_value_via_one_unfold() {
        // Phase 1 cycle-gate: `Mu` matches via one unfold of its body. A `RecursiveRef`
        // inside that body accepts anything for now (phase 3 tightens).
        let t = KType::Mu {
            binder: "Tree".into(),
            body: Box::new(KType::List(Box::new(KType::RecursiveRef("Tree".into())))),
        };
        // Empty list — element type is unconstrained anyway.
        let v = KObject::List(vec![].into());
        assert!(t.matches_value(&v));
        // Non-list shouldn't pass through.
        assert!(!t.matches_value(&KObject::Number(1.0)));
    }

    #[test]
    fn recursive_ref_accepts_anything_phase_one() {
        // Phase 1: `RecursiveRef` is a cycle gate that accepts every value. Phase 3
        // tightens this by threading the enclosing `Mu`'s body through the predicate.
        let t = KType::RecursiveRef("Tree".into());
        assert!(t.matches_value(&KObject::Number(1.0)));
        assert!(t.matches_value(&KObject::List(vec![].into())));
    }

    /// `AnyUserType { kind: Struct }` accepts `Future(KObject::Struct{..})` and rejects
    /// carriers of other kinds (`Tagged`) or wholly different families (`Number`).
    /// Anchors the wildcard predicate's family-filtering behavior — stage 3.0b will
    /// flip `from_name("Struct")` to produce this variant, and dispatch tests using
    /// `(PICK x: Struct)` must continue to accept any struct carrier.
    #[test]
    fn any_user_type_struct_accepts_struct_future_only() {
        use crate::runtime::machine::core::RuntimeArena;
        use indexmap::IndexMap;
        use std::rc::Rc;
        // Arena-allocate the carriers: `KObject` is invariant in its `'a` lifetime, so
        // stack locals trip dropck. Arena allocation hands out `&'a KObject<'a>` whose
        // lifetime is tied to the arena's, dodging the false-positive.
        let arena = RuntimeArena::new();
        let t = KType::AnyUserType { kind: UserTypeKind::Struct };
        let s: &KObject<'_> = arena.alloc_object(KObject::Struct {
            name: "Point".into(),
            scope_id: 0,
            fields: Rc::new(IndexMap::new()),
        });
        let tagged: &KObject<'_> = arena.alloc_object(KObject::Tagged {
            tag: "some".into(),
            value: Rc::new(KObject::Number(1.0)),
            scope_id: 0,
            name: "Maybe".into(),
        });
        let n: &KObject<'_> = arena.alloc_object(KObject::Number(1.0));
        assert!(t.accepts_part(&ExpressionPart::Future(s)));
        assert!(!t.accepts_part(&ExpressionPart::Future(tagged)));
        assert!(!t.accepts_part(&ExpressionPart::Future(n)));
    }

    /// Specificity ordering for the new `UserType` / `AnyUserType` variants:
    /// - `AnyUserType` is strictly under `Any` (handled by the top-level `Any` short-circuit).
    /// - `UserType { kind: K, .. }` is strictly under `AnyUserType { kind: K }` (same kind).
    /// - `UserType` of one kind and `AnyUserType` of a different kind are incomparable
    ///   (sibling families).
    #[test]
    fn user_type_specificity_lattice() {
        let any_struct = KType::AnyUserType { kind: UserTypeKind::Struct };
        let any_tagged = KType::AnyUserType { kind: UserTypeKind::Tagged };
        let point = KType::UserType {
            kind: UserTypeKind::Struct,
            scope_id: 0xAA,
            name: "Point".into(),
        };
        // `AnyUserType` strictly under `Any`.
        assert!(any_struct.is_more_specific_than(&KType::Any));
        assert!(!KType::Any.is_more_specific_than(&any_struct));
        // `UserType { kind: Struct, .. }` strictly under `AnyUserType { kind: Struct }`.
        assert!(point.is_more_specific_than(&any_struct));
        assert!(!any_struct.is_more_specific_than(&point));
        // Different-kind pairs incomparable.
        assert!(!point.is_more_specific_than(&any_tagged));
        assert!(!any_tagged.is_more_specific_than(&point));
    }
}
