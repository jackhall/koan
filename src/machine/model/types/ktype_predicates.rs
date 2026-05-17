//! Per-`ExpressionPart` admissibility and per-value type-tag checks for `KType`.
//! Specificity ordering for dispatch tie-breaking lives here too — these are the
//! predicates the dispatcher consults to decide whether a part fills a slot and
//! which of two viable candidates is more specific.

use super::ktype::{KType, UserTypeKind};
use super::signature::{ExpressionSignature, SignatureElement};
use crate::machine::model::values::KObject;
use crate::machine::model::ast::{ExpressionPart, KLiteral};

impl KType {
    /// True iff this declared parameter `KType` denotes the type language — i.e. a
    /// FN parameter declared with this `KType` carries (at call time) a value whose
    /// nominal type identity is meaningful as a *type* binding, not just as a value
    /// binding. Used by [`crate::machine::core::kfunction::KFunction::invoke`]
    /// to decide whether to dual-write the per-call binding into
    /// [`crate::machine::core::Bindings::types`] alongside the usual
    /// value-side `bind_value`.
    ///
    /// Variants returning `true`:
    /// - [`KType::SignatureBound`]: parameter is a module ascribed to a signature;
    ///   the bound `KObject::KModule` carries a nominal `UserType { kind: Module, .. }`.
    /// - [`KType::Signature`]: parameter is a first-class signature value; its
    ///   nominal identity is `SignatureBound { sig_id, sig_path, pinned_slots: [] }`.
    /// - [`KType::Type`]: parameter is a `KObject::KTypeValue(kt)` schema; the
    ///   identity is `kt` itself.
    /// - [`KType::TypeExprRef`]: parameter carries a type expression
    ///   (`KObject::KTypeValue` / `KObject::TypeNameRef`); identity is the
    ///   elaborated `KType`.
    /// - [`KType::AnyUserType`] with `kind: Module`: parameter is an unascribed
    ///   module; identity is the module's nominal `UserType { kind: Module, .. }`.
    ///
    /// Everything else (`Number`, `Str`, `List<_>`, `KExpression`, `Identifier`,
    /// concrete `UserType`, etc.) returns `false` — those parameters carry no
    /// type-language identity.
    pub fn is_type_denoting(&self) -> bool {
        matches!(
            self,
            KType::SignatureBound { .. }
                | KType::Signature
                | KType::Type
                | KType::TypeExprRef
                | KType::AnyUserType { kind: UserTypeKind::Module }
        )
    }

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
            // SignatureBound strictly refines the "any module" wildcard: a sig-typed slot
            // is a refinement of "any module." Two SignatureBounds with different sig_ids
            // are incomparable — they're disjoint slot types — so this predicate stays
            // `false` for that case by falling through to the wildcard.
            (SignatureBound { .. }, AnyUserType { kind: UserTypeKind::Module }) => true,
            // Same-sig SignatureBound specificity by `pinned_slots`. A pin-extended form
            // strictly refines a pin-reduced form when every slot in the reduced side's
            // pin vec also appears (with equal `KType`) on the extended side. Disjoint
            // constraint sets are incomparable; same-key-different-`KType` is a hard
            // mismatch and likewise incomparable. Different `sig_id`s fall through to
            // the wildcard `false`.
            (
                SignatureBound { sig_id: ia, pinned_slots: pa, .. },
                SignatureBound { sig_id: ib, pinned_slots: pb, .. },
            ) if ia == ib => {
                // Strict refinement: `pa` must cover every `(name, kt)` in `pb`
                // (equal `KType`) AND carry at least one constraint `pb` lacks.
                if pa.len() <= pb.len() {
                    return false;
                }
                for (name, expected) in pb.iter() {
                    match pa.iter().find(|(n, _)| n == name) {
                        Some((_, actual)) if actual == expected => {}
                        _ => return false,
                    }
                }
                true
            }
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
            // Two `ConstructorApply`s with the same `ctor` rank by arg specificity —
            // mirror of the `List(a)` vs `List(b)` arm. Different `ctor`s are
            // incomparable (sibling per-call applications). Arity mismatch is
            // incomparable too — we hold the elaborator's arity check load-bearing.
            (
                ConstructorApply { ctor: ca, args: aa },
                ConstructorApply { ctor: cb, args: ab },
            ) if ca == cb && aa.len() == ab.len() => {
                let any_more = aa.iter().zip(ab.iter()).any(|(x, y)| x.is_more_specific_than(y));
                let all_eq_or_more = aa
                    .iter()
                    .zip(ab.iter())
                    .all(|(x, y)| x == y || x.is_more_specific_than(y));
                any_more && all_eq_or_more
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
            // Mirror of `accepts_part`'s SignatureBound arm. With non-empty `pinned_slots`,
            // also require that each pinned slot exists in the module's `type_members` with
            // the structurally-equal `KType` — sharing constraints reject mismatched
            // ascriptions.
            KType::SignatureBound { sig_id, pinned_slots, .. } => match obj {
                KObject::KModule(m, _) => {
                    if !m.compatible_sigs.borrow().contains(sig_id) {
                        return false;
                    }
                    if pinned_slots.is_empty() {
                        return true;
                    }
                    let tm = m.type_members.borrow();
                    pinned_slots.iter().all(|(name, expected)| {
                        tm.get(name).map(|actual| actual == expected).unwrap_or(false)
                    })
                }
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
                    | (UserTypeKind::Newtype { .. }, KObject::Wrapped { .. })
            ),
            // Phase 1: one-unfold check. Cycle-gating (a threaded "currently unfolding" set)
            // is a phase-3 concern; today no runtime value carries a `RecursiveRef` so the
            // unfold can't actually recurse onto one.
            KType::Mu { body, .. } => body.matches_value(obj),
            // Phase 1: cycle gate. Inside a `Mu` body the recursive back-edge accepts
            // anything; phase 3 will tighten this by carrying the enclosing `Mu`'s body
            // through the predicate's call frame.
            KType::RecursiveRef(_) => true,
            // Higher-kinded application has no runtime carrier in stage 2 — no
            // `KObject` synthesizes a `ConstructorApply` `ktype()`. Reject all values;
            // the meta-type admissibility path goes through `accepts_part` against a
            // `Future(KTypeValue(_))`.
            KType::ConstructorApply { .. } => false,
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
            // Per-declaration identity: a slot typed with a concrete `UserType { kind,
            // scope_id, name }` accepts only a `Future(KObject)` value whose `ktype()`
            // reports the same `UserType`. Same equality is the abstraction-barrier check
            // for opaquely-ascribed module abstract types (`Foo.Type`).
            KType::UserType { .. } => {
                matches!(part, ExpressionPart::Future(obj) if &obj.ktype() == self)
            }
            // Wildcard "any user-declared X" slot: the `kind` discriminator selects which
            // family of carriers we admit (`Struct`/`Tagged`/`Module`). Surface names
            // `Struct`/`Tagged`/`Module` from `from_name` resolve here, so existing
            // dispatch tests using `(PICK x: Struct)` accept any struct carrier
            // regardless of declaring schema.
            KType::AnyUserType { kind } => match part {
                ExpressionPart::Future(obj) => matches!(
                    (kind, obj),
                    (UserTypeKind::Struct, KObject::Struct { .. })
                        | (UserTypeKind::Tagged, KObject::Tagged { .. })
                        | (UserTypeKind::Module, KObject::KModule(_, _))
                        | (UserTypeKind::Newtype { .. }, KObject::Wrapped { .. })
                ),
                _ => false,
            },
            // O(1) per-sig admissibility: a `Future(KModule)` fills a sig-typed slot iff
            // its ascription-populated `compatible_sigs` set carries the slot's `sig_id`.
            // Unascribed source modules never match (their compat set is empty); pass them
            // through `:|` / `:!` first. Bare-name arguments are routed through value
            // lookup (LET-bound to a lowercase identifier) so they enter as Identifier
            // tokens which the auto-wrap pass converts to sub-Dispatches that resolve
            // to the module value before re-entering this slot. When `pinned_slots` is
            // non-empty, each pin must additionally match the module's `type_members`
            // entry — mirror of the `matches_value` arm above.
            KType::SignatureBound { sig_id, pinned_slots, .. } => match part {
                ExpressionPart::Future(KObject::KModule(m, _)) => {
                    if !m.compatible_sigs.borrow().contains(sig_id) {
                        return false;
                    }
                    if pinned_slots.is_empty() {
                        return true;
                    }
                    let tm = m.type_members.borrow();
                    pinned_slots.iter().all(|(name, expected)| {
                        tm.get(name).map(|actual| actual == expected).unwrap_or(false)
                    })
                }
                _ => false,
            },
            KType::Signature => matches!(part, ExpressionPart::Future(KObject::KSignature(_))),
            // Phase 1: same one-unfold rule as `matches_value`.
            KType::Mu { body, .. } => body.accepts_part(part),
            // Phase 1: cycle gate — accept anything until phase 3 introduces a threaded
            // unfold set.
            KType::RecursiveRef(_) => true,
            // Higher-kinded application: structural identity by `(ctor, args)`. Stage 2
            // has no runtime carrier whose `ktype()` would synthesize a `ConstructorApply`
            // (`KObject::KTypeValue` reports `TypeExprRef`, not the inner application), so
            // a slot typed `ConstructorApply` admits only a `Future(KTypeValue(_))` whose
            // inner `KType` is structurally equal. This is the meta-type path — actual
            // value-level admissibility for opaque applied types is a stage-3 concern.
            KType::ConstructorApply { .. } => match part {
                ExpressionPart::Future(KObject::KTypeValue(kt)) => kt == self,
                _ => false,
            },
        }
    }
}

/// Structural function-type compatibility check. Returns true iff `sig`'s declared parameter
/// types and return type are equal (by KType structural equality) to the slot's expectations.
/// Strict equality, not subtyping — a function declared `(x: Number) -> Str` only fills a slot
/// typed `Function<(Number) -> Str>`, not `Function<(Any) -> Str>`. Subtype-aware function
/// matching (contravariant in args, covariant in ret) is a future refinement.
///
/// `Deferred(_)` return-type carrier: the structural-type comparison can't see the per-call
/// resolution, so a `Deferred` return collapses to `KType::Any` for this check (admit
/// anything). Documented coarsening — two FNs differing only in their deferred carriers
/// look structurally identical at this comparison site. There is no current consumer of
/// the difference (module-system functor-params Stage B never lifts a deferred-return FN
/// into a structural `KFunction` slot in a way that would exercise refinement), but flag
/// in case modular-implicit search or similar future work needs precision here.
pub(super) fn function_compat(
    sig: &ExpressionSignature<'_>,
    args: &[KType],
    ret: &KType,
) -> bool {
    use crate::machine::model::types::ReturnType;
    let sig_ret_kt: &KType = match &sig.return_type {
        ReturnType::Resolved(kt) => kt,
        ReturnType::Deferred(_) => {
            // Tripwire for the documented coarsening: a `Deferred(_)`-return candidate
            // being checked against a slot whose `ret` is more specific than `Any` is a
            // scenario we haven't decided. Today the `==` below safely refuses
            // (`Any != SpecificT`), but the refusal is silent — a future consumer adding
            // precise FN-typed slots (`LET cb: Function<(Er) -> Er>` against a deferred-
            // return candidate) would see "no matching function" with no signal that the
            // refusal is due to coarsening rather than a real shape mismatch. When this
            // assertion fires, refine either the synthesis at
            // `kobject.rs::function_value_ktype` (mint a precision-aware variant) or this
            // admission site (route through a value-aware admission helper that can see
            // the underlying `KFunction::signature.return_type`).
            debug_assert!(
                matches!(ret, KType::Any),
                "Deferred-return FN candidate against non-Any slot ret ({:?}) — \
                 see ktype_predicates.rs::function_compat for the unresolved case",
                ret,
            );
            &KType::Any
        }
    };
    if sig_ret_kt != ret {
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
mod tests;
