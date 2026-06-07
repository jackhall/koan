//! Per-`ExpressionPart` admissibility, per-value type-tag checks, and specificity
//! ordering for dispatch tie-breaking on `KType`. See
//! [design/typing/ktype.md](../../../../design/typing/ktype.md).

use std::rc::Rc;

use super::ktype::KType;
use super::record::Record;
use super::recursive_set::NominalKind;
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
            // Record-value subtyping: width-superset + covariant depth (the dual of the
            // contravariant width-drop `param_record_more_specific` for function params).
            (Record(a), Record(b)) => record_value_more_specific(a, b),
            // Function subtyping: contravariant params with width-subset, covariant
            // return (see `param_record_more_specific`). A param the more-specific side
            // doesn't declare is fine (width drop); a param it declares but the other
            // side lacks makes them incomparable, so the helper's `keys()` guard returns
            // `false`. The variant tags stay matched separately (KFunction never compares
            // against KFunctor), but the shared `params`/`ret` shape lets both arms
            // delegate to one helper.
            (
                KFunction {
                    params: pa,
                    ret: ra,
                },
                KFunction {
                    params: pb,
                    ret: rb,
                },
            ) => param_record_more_specific(pa, ra, pb, rb),
            (
                KFunctor {
                    params: pa,
                    ret: ra,
                    ..
                },
                KFunctor {
                    params: pb,
                    ret: rb,
                    ..
                },
            ) => param_record_more_specific(pa, ra, pb, rb),
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
            // A sealed nominal member is more specific than the `AnyUserType` wildcard of
            // the same surface family — read the member's `kind` off its set, by index.
            (SetRef { set, index }, AnyUserType { kind: b }) if set.member(*index).kind == *b => {
                true
            }
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
            // Every slot field must be present in the value and match (depth). Extra value
            // fields are fine — a wider record value is more specific than a narrower slot.
            KType::Record(fields) => match obj {
                KObject::Record(values, _) => fields.iter().all(|(name, ft)| {
                    values
                        .get(name)
                        .map(|v| ft.matches_value(v))
                        .unwrap_or(false)
                }),
                _ => false,
            },
            KType::KFunction { params, ret } => match obj {
                KObject::KFunction(f, _) => {
                    if f.is_functor {
                        return false;
                    }
                    function_compat(&f.signature, params, ret, false)
                }
                KObject::KFuture(_, _) => true,
                _ => false,
            },
            KType::KFunctor { params, ret, .. } => match obj {
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
                (NominalKind::Tagged, KObject::Tagged { .. })
                    | (NominalKind::Newtype, KObject::Wrapped { .. })
            ),
            // A stamped `type_args` carrier (from ascription) takes precedence and is
            // checked structurally per-arg; an erased carrier falls back to checking the
            // inhabited tag's payload against the arg that field maps to (see
            // `result_field_param_index`).
            KType::ConstructorApply { ctor, args } => match obj {
                KObject::Tagged {
                    tag,
                    value,
                    set,
                    index,
                    type_args,
                } => {
                    // Ctor identity is `(set ptr, index)` — the same shallow key dispatch
                    // uses everywhere, never a schema descent.
                    let ctor_matches = matches!(
                        ctor.as_ref(),
                        KType::SetRef { set: cset, index: ci }
                            if Rc::ptr_eq(cset, set) && ci == index
                    );
                    if !ctor_matches {
                        return false;
                    }
                    let name = set.member(*index).name.as_str();
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
            // Mirrors the List/Dict split: an unevaluated record literal admits
            // shape-only (field types unknown until evaluation, so two record-typed
            // overloads tie and defer-then-reevaluate); an evaluated record compares its
            // memoized field-type record against the slot via `satisfied_by`.
            KType::Record(_) => match part {
                ExpressionPart::RecordLiteral(_) => true,
                ExpressionPart::Future(KObject::Record(_, carried)) => {
                    self.satisfied_by(&KType::Record(carried.clone()))
                }
                _ => false,
            },
            KType::KFunction { params, ret } => match part {
                ExpressionPart::Future(KObject::KFunction(f, _)) => {
                    if f.is_functor {
                        return false;
                    }
                    function_compat(&f.signature, params, ret, false)
                }
                ExpressionPart::Future(KObject::KFuture(_, _)) => true,
                _ => false,
            },
            KType::KFunctor { params, ret, .. } => match part {
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
            KType::SigiledTypeExpr => matches!(part, ExpressionPart::SigiledTypeExpr(_)),
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
                // Struct / union / Result type tokens flow as `KTypeValue(SetRef)` —
                // admitted by this arm (no separate schema-carrier variant).
                ExpressionPart::Future(KObject::KTypeValue(_)) => true,
                _ => false,
            },
            // Strict `(set ptr, index)` equality is the per-declaration identity check for a
            // sealed nominal type — `obj.ktype()` yields a `SetRef` whose `PartialEq` keys on
            // the shared allocation and index.
            KType::SetRef { .. } => {
                matches!(part, ExpressionPart::Future(obj) if &obj.ktype() == self)
            }
            KType::AnyUserType { kind } => match part {
                ExpressionPart::Future(obj) => matches!(
                    (kind, obj),
                    (NominalKind::Tagged, KObject::Tagged { .. })
                        | (NominalKind::Newtype, KObject::Wrapped { .. })
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
            // Transient / intra-set leaves never reach a real argument slot: `RecursiveRef`
            // is sealed away before dispatch, and `SetLocal` only appears inside a member's
            // schema (reached by navigation, which carries the ambient set).
            KType::RecursiveRef(_) => true,
            KType::SetLocal(_) => false,
            // A whole-set handle names a group of types, not a value type — it admits no
            // argument; the `RECURSIVE TYPES` group name is a reserved value-language seam.
            KType::RecursiveGroup(_) => false,
            // Confined to a synthesized FN/FUNCTOR `ret` slot — never a free-standing
            // argument slot, so it admits nothing on its own.
            KType::DeferredReturn(_) => false,
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

/// Shared name-keyed specificity for the structurally-identical `KFunction` /
/// `KFunctor` arms of [`KType::is_more_specific_than`]. Function subtyping is
/// contravariant in parameters (with width-subset) and covariant in the return,
/// matching the value-into-slot gate in [`function_compat`] so most-specific-wins
/// stays consistent. `self` (the `a` side) is strictly more specific than `other`
/// (the `b` side) iff:
/// - width-subset: `pa.keys() ⊆ pb.keys()` (the more-specific function declares no
///   more parameters — guard returns `false` otherwise);
/// - per shared name, contravariant: `pb[name] == pa[name] || pb[name] ≺ pa[name]`
///   (the more-specific function's params are equal-or-more-general);
/// - covariant return: `ra == rb || ra ≺ rb`;
/// - at least one strict edge (narrower width, a strictly-more-general param, or a
///   strictly-more-specific return).
fn param_record_more_specific<'a>(
    pa: &Record<KType<'a>>,
    ra: &KType<'a>,
    pb: &Record<KType<'a>>,
    rb: &KType<'a>,
) -> bool {
    if !pa.keys().all(|k| pb.get(k).is_some()) {
        return false;
    }
    let params_ok = pa.iter().all(|(name, s)| {
        let o = pb.get(name).unwrap();
        o == s || o.is_more_specific_than(s)
    });
    let params_more = pa
        .keys()
        .any(|k| pb.get(k).unwrap().is_more_specific_than(pa.get(k).unwrap()));
    let ret_more = ra.is_more_specific_than(rb);
    let ret_ok = ra == rb || ret_more;
    let width_strict = pa.len() < pb.len();
    params_ok && ret_ok && (width_strict || params_more || ret_more)
}

/// Width/depth specificity for *record values* — the **dual** of
/// [`param_record_more_specific`]. A record value's fields are covariant (the value is
/// immutable — see [memory-model](../../../../design/memory-model.md)), and a *wider*
/// record is more specific: a `{x, y}` value fills an `{x}` slot. So `a` is strictly more
/// specific than `b` iff:
/// - width-superset: `b.keys() ⊆ a.keys()` (`a` declares every field `b` does, maybe
///   more — guard returns `false` otherwise);
/// - per shared name, covariant: `a[name] == b[name] || a[name] ≺ b[name]`;
/// - at least one strict edge (wider width, or a strictly-more-specific shared field).
///
/// Contrast `param_record_more_specific`, which is *contravariant* with width-*drop* for
/// call-by-name function parameters. Records and function params share the `Record`
/// substrate but order opposite ways — do **not** unify the two helpers.
fn record_value_more_specific<'a>(a: &Record<KType<'a>>, b: &Record<KType<'a>>) -> bool {
    if !b.keys().all(|k| a.get(k).is_some()) {
        return false;
    }
    let depth_ok = b.iter().all(|(name, bt)| {
        let at = a.get(name).unwrap();
        at == bt || at.is_more_specific_than(bt)
    });
    let depth_more = b
        .keys()
        .any(|k| a.get(k).unwrap().is_more_specific_than(b.get(k).unwrap()));
    let width_strict = a.len() > b.len();
    depth_ok && (width_strict || depth_more)
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

/// Sound, order-blind, name-keyed function subtyping: does the value function `sig`
/// fill the slot whose params record is `params` and return type is `ret`? Reasoned
/// against call-by-name invocation (params arrive name-keyed), so the variance is:
/// - Return covariant for a `Resolved` value return: `sig_ret == ret || sig_ret ≺ ret`
///   — a value returning a subtype of the slot's promised return fills the slot.
/// - Return *syntactic* for a `Deferred` value return: the deferred surface form is
///   compared against the slot's `ret`. An `Any` slot admits any deferred return; a
///   `KType::DeferredReturn` slot (synthesized from another deferred-return FN) admits
///   iff its surface shadow equals the candidate's; every other slot rejects, because a
///   deferred return is opaque until per-call elaboration and so refines nothing more
///   precise than its own shadow. See
///   [ktype.md § Variance](../../../../design/typing/ktype.md#variance).
/// - Params contravariant with width-drop: every `Argument` the value declares must
///   appear in `params` (a value-required param the slot doesn't promise is a width
///   violation → `false`); for a shared name, the slot's param must be equal-or-more-
///   specific than the value's (`slot_pt == &a.ktype || slot_pt ≺ &a.ktype`). Extra
///   slot params the value doesn't declare are fine — under call-by-name they arrive
///   unbound (width drop), so there is no exhaustiveness check.
pub(super) fn function_compat<'a>(
    sig: &ExpressionSignature<'a>,
    params: &Record<KType<'a>>,
    ret: &KType<'a>,
    _slot_is_functor: bool,
) -> bool {
    use crate::machine::model::types::{DeferredReturnSurface, ReturnType};
    let ret_ok = match &sig.return_type {
        ReturnType::Resolved(kt) => kt == ret || kt.is_more_specific_than(ret),
        ReturnType::Deferred(d) => match ret {
            KType::Any => true,
            KType::DeferredReturn(slot) => &DeferredReturnSurface::from_deferred(d) == slot,
            _ => false,
        },
    };
    if !ret_ok {
        return false;
    }
    for el in &sig.elements {
        if let SignatureElement::Argument(a) = el {
            match params.get(&a.name) {
                None => return false,
                Some(slot_pt) => {
                    if !(slot_pt == &a.ktype || slot_pt.is_more_specific_than(&a.ktype)) {
                        return false;
                    }
                }
            }
        }
    }
    true
}

#[cfg(test)]
mod tests;
