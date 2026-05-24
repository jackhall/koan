//! Per-`ExpressionPart` admissibility, per-value type-tag checks, and specificity
//! ordering for dispatch tie-breaking on `KType`. See
//! [design/typing/ktype.md](../../../../design/typing/ktype.md).

use super::ktype::{KType, UserTypeKind};
use super::signature::{ExpressionSignature, SignatureElement};
use crate::machine::model::values::KObject;
use crate::machine::model::ast::{ExpressionPart, KLiteral};

impl KType {
    /// True iff a parameter declared with this `KType` carries a value whose nominal
    /// identity is meaningful as a *type* binding (not just a value binding), so the
    /// per-call binding must be dual-written into the types-side scope.
    pub fn is_type_denoting(&self) -> bool {
        matches!(
            self,
            KType::SatisfiesSignature { .. }
                | KType::MetaSignature
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
            (SatisfiesSignature { .. }, AnyUserType { kind: UserTypeKind::Module }) => true,
            // Same-sig: strict refinement iff `pa` covers every `(name, kt)` in `pb`
            // with equal `KType` AND carries at least one constraint `pb` lacks.
            // Disjoint or same-key-different-`KType` pin sets are incomparable.
            (
                SatisfiesSignature { sig_id: ia, pinned_slots: pa, .. },
                SatisfiesSignature { sig_id: ib, pinned_slots: pb, .. },
            ) if ia == ib => {
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
            (UserType { kind: a, .. }, AnyUserType { kind: b }) if a == b => true,
            (Mu { binder: ba, body: a }, Mu { binder: bb, body: b }) if ba == bb => {
                a.is_more_specific_than(b)
            }
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

    /// True iff a value carrying type `carried` satisfies a slot declared as `self` — exact
    /// match or covariant refinement (`carried` is the more specific). The element-position
    /// helper for dispatch admission of *evaluated* containers (see `accepts_part`): a
    /// `List<Number>` value fills a `:(List Any)` slot, but a `List<Any>` value (the join an
    /// empty or heterogeneous literal memoizes) does not fill `:(List Number)`.
    pub fn satisfied_by(&self, carried: &KType) -> bool {
        *self == *carried || carried.is_more_specific_than(self)
    }

    /// True iff a runtime `KObject` value satisfies this declared type. `Any` matches
    /// everything; container types recurse into element/key/value positions; function types
    /// require structural signature compatibility (a `KFuture` thunk is accepted because its
    /// result isn't known yet — full check deferred to runtime).
    pub fn matches_value(&self, obj: &KObject<'_>) -> bool {
        match self {
            KType::Any => true,
            KType::List(elem) => match obj {
                KObject::List(items, _) => items.iter().all(|x| elem.matches_value(x)),
                _ => false,
            },
            KType::Dict(k_ty, v_ty) => match obj {
                KObject::Dict(map, _, _) => map.iter().all(|(k_key, v_obj)| {
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
            KType::SatisfiesSignature { sig_id, pinned_slots, .. } => match obj {
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
            KType::AnyUserType { kind } => matches!(
                (kind, obj),
                (UserTypeKind::Struct, KObject::Struct { .. })
                    | (UserTypeKind::Tagged, KObject::Tagged { .. })
                    | (UserTypeKind::Module, KObject::KModule(_, _))
                    | (UserTypeKind::Newtype { .. }, KObject::Wrapped { .. })
            ),
            // One-unfold. No runtime value carries a `RecursiveRef`, so this can't
            // recurse onto one; cycle-gating waits on a real carrier.
            KType::Mu { body, .. } => body.matches_value(obj),
            KType::RecursiveRef(_) => true,
            // A `ConstructorApply` slot (`:(Result T E)`) admits a `Tagged` value whose
            // declaring schema is the same constructor, checking the *inhabited* tag's
            // payload against the type argument that field maps to (Result: `ok`→arg 0,
            // `error`→arg 1; see `result_field_param_index`). The non-inhabited parameter
            // is unconstrained at the value — a `Result` value occupies exactly one tag, so
            // only that side carries a payload to check. A populated `type_args` carrier
            // (stamped by ascription) takes precedence: when present, every arg is checked
            // structurally against the carried args.
            KType::ConstructorApply { ctor, args } => match obj {
                KObject::Tagged { tag, value, name, scope_id, type_args } => {
                    let ctor_matches = matches!(
                        ctor.as_ref(),
                        KType::UserType { name: cn, scope_id: cs, .. }
                            if cn == name && cs == scope_id
                    );
                    if !ctor_matches {
                        return false;
                    }
                    // Stamped carrier: structural per-arg check against the declared args.
                    if !type_args.is_empty() {
                        return type_args.len() == args.len()
                            && type_args.iter().zip(args.iter()).all(|(a, b)| {
                                matches!(b, KType::Any) || a == b
                            });
                    }
                    // Erased carrier: check the inhabited tag's payload against its arg.
                    match result_field_param_index(name, tag).and_then(|i| args.get(i)) {
                        Some(arg) => arg.matches_value(value),
                        // Unknown field linkage — fall back to the inhabited payload being
                        // unconstrained (ctor identity already matched).
                        None => true,
                    }
                }
                _ => false,
            },
            _ => *self == obj.ktype(),
        }
    }

    /// Per-`ExpressionPart` admissibility check: can a part of this shape fill an argument
    /// slot of this type? An *unevaluated* container literal (`ListLiteral` / `DictLiteral`)
    /// is shape-only — its element types aren't known until it evaluates, so it admits and
    /// the dispatch driver defers it (a strict tie over two container slots re-dispatches
    /// once the literal becomes a typed `Future`). An *evaluated* container
    /// (`Future(List/Dict)`) is element-aware: it admits only when its memoized carried type
    /// satisfies the slot's declared element/key/value type (`satisfied_by`) — pure
    /// type-level comparison, no element walk. A `List<Any>` value (empty or heterogeneous)
    /// thus admits `:(List Any)` but not `:(List Number)`, and a non-satisfying container
    /// falls through the scope walk rather than committing to a bind-time mismatch. Function
    /// slots with a structural `KFunction { args, ret }` shape validate the bound function's
    /// signature here, since `KObject::KFunction` carries the full signature.
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
            KType::List(elem) => match part {
                ExpressionPart::ListLiteral(_) => true,
                ExpressionPart::Future(KObject::List(_, carried)) => elem.satisfied_by(carried),
                _ => false,
            },
            KType::Dict(k_ty, v_ty) => match part {
                ExpressionPart::DictLiteral(_) => true,
                ExpressionPart::Future(KObject::Dict(_, carried_k, carried_v)) => {
                    k_ty.satisfied_by(carried_k) && v_ty.satisfied_by(carried_v)
                }
                _ => false,
            },
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
            // Strict equality is the abstraction-barrier check for opaquely-ascribed
            // module abstract types (`Foo.Type`).
            KType::UserType { .. } => {
                matches!(part, ExpressionPart::Future(obj) if &obj.ktype() == self)
            }
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
            // A `Future(KModule)` fills a sig-typed slot iff its ascription-populated
            // `compatible_sigs` set carries `sig_id`. Unascribed source modules never
            // match (their compat set is empty) — pass them through `:|` / `:!` first.
            KType::SatisfiesSignature { sig_id, pinned_slots, .. } => match part {
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
            KType::MetaSignature => matches!(part, ExpressionPart::Future(KObject::KSignature(_))),
            KType::Mu { body, .. } => body.accepts_part(part),
            KType::RecursiveRef(_) => true,
            // Meta-type path: no runtime carrier synthesizes a `ConstructorApply`
            // `ktype()`, so admit only `Future(KTypeValue(_))` with structurally-equal
            // inner `KType`.
            KType::ConstructorApply { .. } => match part {
                ExpressionPart::Future(KObject::KTypeValue(kt)) => kt == self,
                _ => false,
            },
        }
    }
}

/// Field→type-parameter linkage for the builtin `Result` parameterized union: which
/// type-argument position a given variant's payload is checked against. `ok`→0 (`T`),
/// `error`→1 (`E`), mirroring the `param_names: ["T", "E"]` ordering registered in
/// [`crate::builtins::result`]. Returns `None` for any other carrier name — user UNIONs
/// don't yet carry runtime type arguments, so their `ConstructorApply` admission falls
/// back to a ctor-identity-only check.
///
/// Lives in the type layer (rather than `builtins/result.rs`) because `matches_value`
/// consumes it and `model::types` sits below `builtins` in the dependency stack; the
/// builtin registration is the source of the *ordering*, this is the read side.
pub fn result_field_param_index(carrier_name: &str, tag: &str) -> Option<usize> {
    match (carrier_name, tag) {
        ("Result", "ok") => Some(0),
        ("Result", "error") => Some(1),
        _ => None,
    }
}

/// Structural function-type compatibility. True iff `sig`'s declared parameter types
/// and return type are equal (by KType structural equality) to the slot's expectations.
/// Strict equality, not subtyping — a function declared `(x: Number) -> Str` only fills
/// a slot typed `Function<(Number) -> Str>`, not `Function<(Any) -> Str>`.
///
/// A `Deferred(_)` return collapses to `KType::Any` for this check (the structural
/// comparison can't see the per-call resolution). See
/// [roadmap/kfunction-deferred-ret-precision.md](../../../../roadmap/predicate_typing/kfunction-deferred-ret-precision.md).
pub(super) fn function_compat(
    sig: &ExpressionSignature<'_>,
    args: &[KType],
    ret: &KType,
) -> bool {
    use crate::machine::model::types::ReturnType;
    let sig_ret_kt: &KType = match &sig.return_type {
        ReturnType::Resolved(kt) => kt,
        ReturnType::Deferred(_) => {
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
