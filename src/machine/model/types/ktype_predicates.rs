//! Per-`ExpressionPart` admissibility, per-value type-tag checks, and specificity
//! ordering for dispatch tie-breaking on `KType`. See
//! [design/typing/ktype.md](../../../../design/typing/ktype.md).

use super::ktype::{KType, UserTypeKind};
use super::signature::{ExpressionSignature, SignatureElement};
use crate::machine::model::ast::{ExpressionPart, KLiteral};
use crate::machine::model::values::KObject;

impl<'a> KType<'a> {
    /// True iff a parameter declared with this `KType` carries a value whose nominal
    /// identity is meaningful as a *type* binding (not just a value binding), so the
    /// per-call binding must be dual-written into the types-side scope.
    pub fn is_type_denoting(&self) -> bool {
        matches!(
            self,
            KType::Signature { .. }
                | KType::AnySignature
                | KType::Type
                | KType::TypeExprRef
                | KType::AnyModule
        )
    }

    /// Admissibility predicate for the FUNCTOR return-type slot. See
    /// [design/typing/functors.md](../../../../design/typing/functors.md).
    /// `KType::Type` is intentionally excluded — bare `Type` denotes "any type
    /// value" rather than "a module or signature value", and the design pins
    /// the seam to the narrower set.
    pub fn is_admissible_functor_return(&self) -> bool {
        match self {
            KType::AnySignature
            | KType::Signature { .. }
            | KType::AnyModule
            | KType::Module { .. } => true,
            KType::KFunctor { ret, .. } => ret.is_admissible_functor_return(),
            _ => false,
        }
    }

    /// Strict specificity ordering. Concrete types outrank `Any` and the
    /// unconstrained-name slot types (`Identifier`, `TypeExprRef`), so an overload
    /// like `ATTR <s:AnyUserType{Struct}>` beats its `ATTR <s:Identifier>` sibling
    /// when both admit. Parameterized containers are covariant in their inner slots.
    /// Returns `false` for equal types.
    pub fn is_more_specific_than(&self, other: &KType<'a>) -> bool {
        use KType::*;
        if matches!(other, Any) && !matches!(self, Any) {
            return true;
        }
        if matches!(other, Identifier | TypeExprRef)
            && !matches!(self, Identifier | TypeExprRef | Any)
        {
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
            (KFunction { args: aa, ret: ar }, KFunction { args: ba, ret: br })
                if aa.len() == ba.len() =>
            {
                let args_more = aa
                    .iter()
                    .zip(ba.iter())
                    .any(|(x, y)| x.is_more_specific_than(y));
                let args_eq = aa == ba;
                let ret_more = ar.is_more_specific_than(br);
                let ret_eq = ar == br;
                (args_more && (ret_more || ret_eq)) || (args_eq && ret_more)
            }
            (
                KFunctor {
                    params: pa,
                    ret: ra,
                },
                KFunctor {
                    params: pb,
                    ret: rb,
                },
            ) if pa.len() == pb.len() => {
                let params_more = pa
                    .iter()
                    .zip(pb.iter())
                    .any(|(x, y)| x.is_more_specific_than(y));
                let params_eq = pa == pb;
                let ret_more = ra.is_more_specific_than(rb);
                let ret_eq = ra == rb;
                (params_more && (ret_more || ret_eq)) || (params_eq && ret_more)
            }
            // Constraint role: `:S` (a module satisfying `S`) is more specific than the
            // `:Module` wildcard.
            (Signature { .. }, AnyModule) => true,
            (Module { .. }, AnyModule) => true,
            // Value role: a concrete signature type is more specific than the
            // `:Signature` wildcard.
            (Signature { .. }, AnySignature) => true,
            // Same-sig: strict refinement iff `pa` covers every `(name, kt)` in `pb`
            // with equal `KType` AND carries at least one constraint `pb` lacks.
            // Disjoint or same-key-different-`KType` pin sets are incomparable.
            (
                Signature {
                    sig: sa,
                    pinned_slots: pa,
                },
                Signature {
                    sig: sb,
                    pinned_slots: pb,
                },
            ) if sa.sig_id() == sb.sig_id() => {
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
            (
                Mu {
                    binder: ba,
                    body: a,
                },
                Mu {
                    binder: bb,
                    body: b,
                },
            ) if ba == bb => a.is_more_specific_than(b),
            (ConstructorApply { ctor: ca, args: aa }, ConstructorApply { ctor: cb, args: ab })
                if ca == cb && aa.len() == ab.len() =>
            {
                let any_more = aa
                    .iter()
                    .zip(ab.iter())
                    .any(|(x, y)| x.is_more_specific_than(y));
                let all_eq_or_more = aa
                    .iter()
                    .zip(ab.iter())
                    .all(|(x, y)| x == y || x.is_more_specific_than(y));
                any_more && all_eq_or_more
            }
            _ => false,
        }
    }

    /// True iff `carried` satisfies a slot declared as `self` — exact match or covariant
    /// refinement. A `List<Any>` value (the join an empty or heterogeneous literal
    /// memoizes) does not satisfy `:(LIST OF Number)`.
    pub fn satisfied_by(&self, carried: &KType<'a>) -> bool {
        *self == *carried || carried.is_more_specific_than(self)
    }

    /// True iff a runtime `KObject` value satisfies this declared type. A `KFuture`
    /// thunk is accepted because its result isn't known yet — the full check defers to
    /// runtime.
    pub fn matches_value(&self, obj: &KObject<'a>) -> bool {
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
                KObject::KFunction(f, _) => {
                    if f.is_functor {
                        return false;
                    }
                    function_compat(&f.signature, args, ret, false)
                }
                KObject::KFuture(_, _) => true,
                _ => false,
            },
            KType::KFunctor { params, ret } => match obj {
                KObject::KFunction(f, _) => {
                    if !f.is_functor {
                        return false;
                    }
                    function_compat(&f.signature, params, ret, true)
                }
                KObject::KFuture(_, _) => true,
                _ => false,
            },
            // Constraint role: a `Signature { .. }` slot matches a *module* whose
            // `compatible_sigs` contains `sig.sig_id()` (+ pinned-slot check). A signature
            // *value* is matched by `AnySignature` below, never here.
            KType::Signature { sig, pinned_slots } => match obj {
                KObject::KTypeValue(KType::Module { module: m, .. }) => {
                    if !m.compatible_sigs.borrow().contains(&sig.sig_id()) {
                        return false;
                    }
                    if pinned_slots.is_empty() {
                        return true;
                    }
                    let tm = m.type_members.borrow();
                    pinned_slots.iter().all(|(name, expected)| {
                        tm.get(name)
                            .map(|actual| actual == expected)
                            .unwrap_or(false)
                    })
                }
                _ => false,
            },
            KType::AnyModule => matches!(obj, KObject::KTypeValue(KType::Module { .. })),
            KType::AnySignature => matches!(obj, KObject::KTypeValue(KType::Signature { .. })),
            KType::AnyUserType { kind } => matches!(
                (kind, obj),
                (UserTypeKind::Struct { .. }, KObject::Struct { .. })
                    | (UserTypeKind::Tagged { .. }, KObject::Tagged { .. })
                    | (UserTypeKind::Newtype { .. }, KObject::Wrapped { .. })
            ),
            // One-unfold. No runtime value carries a `RecursiveRef`, so this can't
            // recurse onto one; cycle-gating waits on a real carrier.
            KType::Mu { body, .. } => body.matches_value(obj),
            KType::RecursiveRef(_) => true,
            // A stamped `type_args` carrier (from ascription) takes precedence and is
            // checked structurally per-arg; an erased carrier falls back to checking the
            // inhabited tag's payload against the arg that field maps to (see
            // `result_field_param_index`).
            KType::ConstructorApply { ctor, args } => match obj {
                KObject::Tagged {
                    tag,
                    value,
                    name,
                    scope_id,
                    type_args,
                } => {
                    let ctor_matches = matches!(
                        ctor.as_ref(),
                        KType::UserType { name: cn, scope_id: cs, .. }
                            if cn == name && cs == scope_id
                    );
                    if !ctor_matches {
                        return false;
                    }
                    if !type_args.is_empty() {
                        return type_args.len() == args.len()
                            && type_args
                                .iter()
                                .zip(args.iter())
                                .all(|(a, b)| matches!(b, KType::Any) || a == b);
                    }
                    match result_field_param_index(name, tag).and_then(|i| args.get(i)) {
                        Some(arg) => arg.matches_value(value),
                        None => true,
                    }
                }
                _ => false,
            },
            _ => *self == obj.ktype(),
        }
    }

    /// Per-`ExpressionPart` admissibility for argument slots. Unevaluated container
    /// literals admit shape-only (element types unknown until evaluation); evaluated
    /// containers compare their memoized carried type against the slot via
    /// `satisfied_by` — pure type-level, no element walk. Non-satisfying containers
    /// fall through the scope walk rather than failing the bind.
    pub fn accepts_part(&self, part: &ExpressionPart<'a>) -> bool {
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
                    if f.is_functor {
                        return false;
                    }
                    function_compat(&f.signature, args, ret, false)
                }
                ExpressionPart::Future(KObject::KFuture(_, _)) => true,
                _ => false,
            },
            KType::KFunctor { params, ret } => match part {
                ExpressionPart::Future(KObject::KFunction(f, _)) => {
                    if !f.is_functor {
                        return false;
                    }
                    function_compat(&f.signature, params, ret, true)
                }
                ExpressionPart::Future(KObject::KFuture(_, _)) => true,
                _ => false,
            },
            KType::Identifier => matches!(part, ExpressionPart::Identifier(_)),
            KType::KExpression => matches!(part, ExpressionPart::Expression(_)),
            // A `KTypeValue` carrier of a first-class module or signature is NOT a
            // `TypeExprRef` admission — those route through the dedicated `AnyModule` /
            // `AnySignature` / `Module` / `Signature` slot shapes. Otherwise an
            // `[ATTR <m> <field>]` chained-attr call would tie between the
            // `body_module` and `body_type_lhs` overloads.
            KType::TypeExprRef => match part {
                ExpressionPart::Type(_) => true,
                ExpressionPart::Future(KObject::KTypeValue(KType::Module { .. }))
                | ExpressionPart::Future(KObject::KTypeValue(KType::Signature { .. })) => false,
                ExpressionPart::Future(KObject::KTypeValue(_)) => true,
                _ => false,
            },
            // Same module/signature wall as `TypeExprRef` above. Admitting other
            // `KTypeValue` carriers lets bare builtin type tokens fill a `:Type` slot
            // without a signature-typed-wrapper-module workaround at the call site.
            KType::Type => match part {
                ExpressionPart::Type(_) => true,
                ExpressionPart::Future(KObject::KTypeValue(KType::Module { .. }))
                | ExpressionPart::Future(KObject::KTypeValue(KType::Signature { .. })) => false,
                // Struct / union / Result type tokens flow as `KTypeValue(UserType)` now —
                // admitted by this arm (no separate schema-carrier variant).
                ExpressionPart::Future(KObject::KTypeValue(_)) => true,
                _ => false,
            },
            // Strict equality is the abstraction-barrier check for opaquely-ascribed
            // module abstract types (`Foo.Type`).
            KType::UserType { .. } => {
                matches!(part, ExpressionPart::Future(obj) if &obj.ktype() == self)
            }
            KType::AnyUserType { kind } => match part {
                ExpressionPart::Future(obj) => matches!(
                    (kind, obj),
                    (UserTypeKind::Struct { .. }, KObject::Struct { .. })
                        | (UserTypeKind::Tagged { .. }, KObject::Tagged { .. })
                        | (UserTypeKind::Newtype { .. }, KObject::Wrapped { .. })
                ),
                _ => false,
            },
            KType::AnyModule => matches!(
                part,
                ExpressionPart::Future(KObject::KTypeValue(KType::Module { .. }))
            ),
            KType::AnySignature => matches!(
                part,
                ExpressionPart::Future(KObject::KTypeValue(KType::Signature { .. }))
            ),
            KType::Module { .. } => matches!(
                part,
                ExpressionPart::Future(obj) if obj.ktype() == *self
            ),
            KType::AbstractType { .. } => matches!(
                part,
                ExpressionPart::Future(obj) if obj.ktype() == *self
            ),
            // Constraint role: a `:S` slot admits a *module* satisfying `S` (+ pinned-slot
            // check). Unascribed source modules carry an empty `compatible_sigs` and never
            // match; they must pass through `:|` / `:!` first. A signature *value* is
            // admitted by `AnySignature` above, never here.
            KType::Signature { sig, pinned_slots } => match part {
                ExpressionPart::Future(KObject::KTypeValue(KType::Module {
                    module: m, ..
                })) => {
                    if !m.compatible_sigs.borrow().contains(&sig.sig_id()) {
                        return false;
                    }
                    if pinned_slots.is_empty() {
                        return true;
                    }
                    let tm = m.type_members.borrow();
                    pinned_slots.iter().all(|(name, expected)| {
                        tm.get(name)
                            .map(|actual| actual == expected)
                            .unwrap_or(false)
                    })
                }
                _ => false,
            },
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

/// Field→type-parameter linkage for the builtin `Result` parameterized union:
/// `ok`→0 (`T`), `error`→1 (`E`), mirroring the `param_names: ["T", "E"]` registered
/// in [`crate::builtins::result`]. Returns `None` for any other carrier — user UNIONs
/// don't yet carry runtime type arguments, so their `ConstructorApply` admission
/// falls back to a ctor-identity-only check.
pub fn result_field_param_index(carrier_name: &str, tag: &str) -> Option<usize> {
    match (carrier_name, tag) {
        ("Result", "ok") => Some(0),
        ("Result", "error") => Some(1),
        _ => None,
    }
}

/// Strict structural equality between `sig`'s declared arg/return types and the
/// slot's expectations — not subtyping. A function declared `(x: Number) -> Str`
/// only fills a slot typed `Function<(Number) -> Str>`, not `Function<(Any) -> Str>`.
/// A `Deferred(_)` return collapses to `KType::Any` for this check; see
/// [roadmap/kfunction-deferred-ret-precision.md](../../../../roadmap/type_language/kfunction-deferred-ret-precision.md).
pub(super) fn function_compat<'a>(
    sig: &ExpressionSignature<'a>,
    args: &[KType<'a>],
    ret: &KType<'a>,
    _slot_is_functor: bool,
) -> bool {
    use crate::machine::model::types::ReturnType;
    let sig_ret_kt: &KType<'a> = match &sig.return_type {
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
