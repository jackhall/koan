//! Per-`ExpressionPart` admissibility, per-value type-tag checks, and specificity
//! ordering for dispatch tie-breaking on `KType`. See
//! [design/typing/ktype/README.md](../../../../design/typing/ktype/README.md).

use super::kkind::KKind;
use super::ktraits::Parseable;
use super::ktype::KType;
use super::record::Record;
use super::recursive_set::same_nominal;
use super::registry::{digest_is_content, Relation, TypeRegistry};
use super::sig_schema::{sig_subtype, SigContent};
use super::signature::{ExpressionSignature, SignatureElement};
use super::type_digest::TypeDigest;
use crate::machine::model::ast::{ExpressionPart, KLiteral};
use crate::machine::model::values::{Carried, Held, KObject};
use crate::machine::DeliveredCarried;

/// Whether a value reporting a `ConstructorApply` `ktype()` satisfies a `ConstructorApply`
/// slot: the ctor nominal `(set, index)` matches via [`same_nominal`], the arities agree, and
/// each arg matches pairwise (`Any` slot admits anything, else structural equality). Both ctors
/// must be `SetRef`s. Drives both [`KType::matches_value`]'s `Wrapped` arm and
/// [`KType::accepts_carried`]'s dispatch arm.
fn constructor_apply_admits(
    slot_ctor: &KType,
    slot_args: &[KType],
    value_ctor: &KType,
    value_args: &[KType],
) -> bool {
    let ctor_matches = matches!(
        (slot_ctor, value_ctor),
        (
            KType::SetRef { set: cset, index: ci },
            KType::SetRef { set: vset, index: vi },
        ) if same_nominal(cset, *ci, vset, *vi)
    );
    ctor_matches
        && value_args.len() == slot_args.len()
        && value_args
            .iter()
            .zip(slot_args.iter())
            .all(|(value_arg, slot_arg)| matches!(slot_arg, KType::Any) || value_arg == slot_arg)
}

impl KType {
    /// True iff a parameter declared with this `KType` carries a value whose nominal
    /// identity is meaningful as a *type* binding (not just a value binding), so the
    /// per-call binding must be dual-written into the types-side scope. Only the
    /// *type-value* `OfKind` kinds qualify — a nominal-family kind (`Tagged` / `NewType` /
    /// `TypeConstructor`) classifies a type value but is not itself used as a binding-side
    /// slot, so it stays out (an `OfKind` is type-channel-only and never binds a runtime
    /// instance).
    /// Whether this type is a **scalar leaf** — a primitive (`Number`, `Str`, `Bool`, `Null`,
    /// `Identifier`) with no nested `KType` at all (`List` / `Dict` / `Record` / `KFunction`
    /// carry one; `Module` / `Signature` / `AbstractType` carry owned schema content). Every
    /// `KType` is owned data, so a scalar leaf and a composite differ only in whether a
    /// construction needs to recurse into children, never in ownership.
    pub fn is_region_free_scalar(&self) -> bool {
        matches!(
            self,
            KType::Number | KType::Str | KType::Bool | KType::Null | KType::Identifier
        )
    }

    /// Strict specificity ordering. Concrete types outrank `Any` and the
    /// unconstrained-name slot types (`Identifier`, `ProperType`), so an overload
    /// like `ATTR <s:NewType>` beats its `ATTR <s:Identifier>` sibling when both admit.
    /// A nominal-family kind out-specifies `OfKind(Proper)` (`OfKind(NewType) ≺
    /// OfKind(Proper)`), and a sealed `SetRef` member out-specifies the
    /// `OfKind(kind)` of its own family. Parameterized containers are covariant in their
    /// inner slots. Returns `false` for equal types.
    pub fn is_more_specific_than(&self, other: &KType, types: &TypeRegistry) -> bool {
        if self.is_stored_digest_variant() && other.is_stored_digest_variant() {
            let (subject, candidate) = (self.digest(), other.digest());
            if let Some(verdict) = types.verdict(subject, candidate, Relation::MoreSpecific) {
                return verdict;
            }
            let verdict = self.more_specific_walk(other, types);
            if digest_is_content(self) && digest_is_content(other) {
                types.record_verdict(subject, candidate, Relation::MoreSpecific, verdict);
            }
            verdict
        } else {
            self.more_specific_walk(other, types)
        }
    }

    /// True iff `self` is one of the composite variants whose digest is a stored field
    /// (`digest_of` in `type_digest.rs` reads it directly rather than recomputing it) —
    /// the set of pairs [`is_more_specific_than`](Self::is_more_specific_than) consults the
    /// registry for. Gating on both sides being composite keeps a leaf-vs-leaf or
    /// `OfKind`/`SetRef` compare (already O(1)-ish) out of the registry, where a probe would
    /// only slow it down.
    fn is_stored_digest_variant(&self) -> bool {
        matches!(
            self,
            KType::List { .. }
                | KType::Dict { .. }
                | KType::Record { .. }
                | KType::KFunction { .. }
                | KType::Union { .. }
                | KType::Signature { .. }
                | KType::ConstructorApply { .. }
        )
    }

    fn more_specific_walk(&self, other: &KType, types: &TypeRegistry) -> bool {
        use KType::*;
        if matches!(other, Any) && !matches!(self, Any) {
            return true;
        }
        if matches!(other, Identifier | OfKind(KKind::ProperType))
            && !matches!(self, Identifier | OfKind(KKind::ProperType) | Any)
        {
            return true;
        }
        match (self, other) {
            (List { element: a, .. }, List { element: b, .. }) => a.is_more_specific_than(b, types),
            (
                Dict {
                    key: ka, value: va, ..
                },
                Dict {
                    key: kb, value: vb, ..
                },
            ) => {
                let k_more = ka.is_more_specific_than(kb, types);
                let v_more = va.is_more_specific_than(vb, types);
                let k_eq = **ka == **kb;
                let v_eq = **va == **vb;
                (k_more && (v_more || v_eq)) || (k_eq && v_more)
            }
            // Record-value subtyping: width-superset + covariant depth (the dual of the
            // contravariant width-drop `param_record_more_specific` for function params).
            (Record { fields: a, .. }, Record { fields: b, .. }) => {
                record_value_more_specific(a, b, types)
            }
            // Function subtyping: contravariant params (width-subset), covariant return —
            // see `param_record_more_specific`.
            (
                KFunction {
                    params: pa,
                    ret: ra,
                    ..
                },
                KFunction {
                    params: pb,
                    ret: rb,
                    ..
                },
            ) => param_record_more_specific(pa, ra, pb, rb, types),
            // Value role: a concrete signature type is more specific than the
            // `:Signature` wildcard.
            (Signature { .. }, OfKind(KKind::Signature)) => true,
            // Any non-empty signature refines the empty interface (the lattice top). Keyed on
            // empty *content*, not the mint that produced it, so a zero-member `SIG E = ()` is
            // the same top as `:Module` — and pins must agree (an empty interface pins nothing).
            (
                Signature {
                    content: ca,
                    pinned_slots: pa,
                    ..
                },
                Signature {
                    content: cb,
                    pinned_slots: pb,
                    ..
                },
            ) if cb.is_empty_interface()
                && pb.is_empty()
                && !(ca.is_empty_interface() && pa.is_empty()) =>
            {
                true
            }
            // Same-declaration: strict refinement iff `pa` covers every `(name, kt)` in `pb`
            // with equal `KType` AND carries at least one constraint `pb` lacks.
            // Disjoint or same-key-different-`KType` pin sets are incomparable.
            (
                Signature {
                    content: ca,
                    pinned_slots: pa,
                    ..
                },
                Signature {
                    content: cb,
                    pinned_slots: pb,
                    ..
                },
            ) if ca.sig_id == cb.sig_id => {
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
            // Two signature types with different `sig_id`s — SIG-declared or self-sig, any
            // combination — compare by strict structural subtyping over their pin-folded
            // schemas: `a ≺ b` iff `a`'s schema strictly `sig_subtype`s `b`'s schema (forward
            // holds, reverse fails). Two structurally-identical schemas are mutually-satisfying,
            // hence incomparable.
            (
                Signature {
                    content: ca,
                    pinned_slots: pa,
                    ..
                },
                Signature {
                    content: cb,
                    pinned_slots: pb,
                    ..
                },
            ) if ca.sig_id != cb.sig_id => {
                sig_content_more_specific(ca, pa, self.digest(), cb, pb, other.digest(), types)
            }
            // A nominal-family kind out-specifies `OfKind(Proper)` — `OfKind(NewType) ≺
            // OfKind(Proper)`. (Against `Identifier` / `OfKind(Proper)` the generic rule
            // above already fires; this covers a nominal-vs-nominal-supertype tie.)
            (OfKind(a), OfKind(b)) if a.strictly_below(*b) => true,
            // A sealed nominal member is more specific than the `OfKind` wildcard of the
            // same surface family — read the member's `kind` off its set, by index.
            (SetRef { set, index }, OfKind(b)) if set.member(*index).kind == *b => true,
            (
                ConstructorApply {
                    ctor: ca, args: aa, ..
                },
                ConstructorApply {
                    ctor: cb, args: ab, ..
                },
            ) if **ca == **cb && aa.len() == ab.len() => {
                let any_more = aa
                    .iter()
                    .zip(ab.iter())
                    .any(|(x, y)| x.is_more_specific_than(y, types));
                let all_eq_or_more = aa
                    .iter()
                    .zip(ab.iter())
                    .all(|(x, y)| x == y || x.is_more_specific_than(y, types));
                any_more && all_eq_or_more
            }
            // Union subset: `a` refines `b` iff they are not the same set and every member of
            // `a` is equal to or more specific than some member of `b`. Set equality (not the
            // positional `Vec` compare) gates the strictness, matching order-blind identity.
            (Union { members: a, .. }, Union { members: b, .. }) => {
                let same_set = a.len() == b.len() && a.iter().all(|m| b.iter().any(|bm| bm == m));
                !same_set
                    && a.iter().all(|x| {
                        b.iter()
                            .any(|y| x == y || x.is_more_specific_than(y, types))
                    })
            }
            // Each member of a union is a subtype of it: a non-union `x` is more specific than
            // `Union(ms)` iff it equals or refines one of the members.
            (x, Union { members: ms, .. }) => ms
                .iter()
                .any(|m| x == m || x.is_more_specific_than(m, types)),
            _ => false,
        }
    }

    /// True iff `carried` satisfies a slot declared as `self` — exact match or covariant
    /// refinement. A `List<Any>` value (the join an empty or heterogeneous literal
    /// memoizes) does not satisfy `:(LIST OF Number)`.
    pub fn satisfied_by(&self, carried: &KType, types: &TypeRegistry) -> bool {
        *self == *carried || carried.is_more_specific_than(self, types)
    }

    /// True iff a runtime `KObject` value satisfies this declared type.
    /// Aggregate-cell satisfaction: an `Object` cell defers to [`matches_value`]; a `Type`
    /// cell (a first-class type stored in a list/dict/record) satisfies a type-accepting
    /// slot — `Any`, an `OfKind` kind that subsumes the type's `kind_of`, or an exact type
    /// identity.
    pub fn matches_held(&self, cell: &Held<'_>, types: &TypeRegistry) -> bool {
        match cell {
            Held::Object(o) => self.matches_value(o, types),
            Held::Type(t) => match self {
                KType::Any => true,
                KType::OfKind(k) => k.admits(t.kind_of()),
                _ => self == t,
            },
        }
    }

    pub fn matches_value(&self, obj: &KObject<'_>, types: &TypeRegistry) -> bool {
        match self {
            KType::Any => true,
            KType::List { element: elem, .. } => match obj {
                KObject::List(items, _) => items.iter().all(|x| elem.matches_held(x, types)),
                _ => false,
            },
            KType::Dict {
                key: k_ty,
                value: v_ty,
                ..
            } => match obj {
                KObject::Dict(map, _, _) => map.iter().all(|(k_key, v_obj)| {
                    let k_t = k_key.ktype();
                    (matches!(k_ty.as_ref(), KType::Any) || **k_ty == k_t)
                        && v_ty.matches_held(v_obj, types)
                }),
                _ => false,
            },
            // Every slot field must be present in the value and match (depth). Extra value
            // fields are fine — a wider record value is more specific than a narrower slot.
            KType::Record { fields, .. } => match obj {
                KObject::Record(values, _) => fields.iter().all(|(name, ft)| {
                    values
                        .get(name)
                        .map(|v| ft.matches_held(v, types))
                        .unwrap_or(false)
                }),
                _ => false,
            },
            KType::KFunction { params, ret, .. } => match obj {
                KObject::KFunction(f) => function_compat(&f.signature, params, ret, types),
                _ => false,
            },
            // Constraint role: a `Signature { .. }` slot is satisfied by a module value on the
            // Object channel, via [`Module::satisfies_sig_content`] plus pinned-slot agreement.
            KType::Signature {
                content,
                pinned_slots,
                ..
            } => match obj {
                KObject::Module(m) => {
                    m.satisfies_sig_content(content, types)
                        && (pinned_slots.is_empty() || m.satisfies_pins(pinned_slots))
                }
                _ => false,
            },
            // A type-accepting slot is **type-channel-only**: no runtime `KObject` is a type
            // value, so a value is never matched by a kind. `Proper` / `Any` keep a
            // defensive identity check for the rare case of a type carried as a value
            // (`OfKind(Proper) == ktype()`); every other kind admits no runtime instance.
            KType::OfKind(k) => match k {
                KKind::ProperType | KKind::AnyType => *self == obj.ktype(),
                _ => false,
            },
            // A stamped `type_args` carrier (from ascription) takes precedence and is
            // checked structurally per-arg; an erased carrier falls back to checking the
            // inhabited tag's payload against the arg that field maps to (see
            // `result_field_param_index`).
            KType::ConstructorApply { ctor, args, .. } => match obj {
                KObject::Tagged {
                    tag,
                    value,
                    set,
                    index,
                    type_args,
                } => {
                    // Ctor identity is the nominal `(set, index)` — the same content-digest
                    // key dispatch uses everywhere (see `same_nominal`), never a schema descent.
                    let ctor_matches = matches!(
                        ctor.as_ref(),
                        KType::SetRef { set: cset, index: ci }
                            if same_nominal(cset, *ci, set, *index)
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
                        Some(arg) => arg.matches_value(value, types),
                        None => true,
                    }
                }
                // An identity-wrapper value (`NEWTYPE (Type AS Wrapper)`): its `type_id` is itself
                // a `ConstructorApply`. Match the ctor nominal `(set, index)` via `same_nominal`
                // (both ctors are `SetRef`s), then check the stamped args pairwise by the same rule
                // the stamped-`type_args` `Tagged` path uses (`Any` slot admits anything).
                KObject::Wrapped {
                    type_id:
                        KType::ConstructorApply {
                            ctor: vctor,
                            args: vargs,
                            ..
                        },
                    ..
                } => constructor_apply_admits(ctor, args, vctor, vargs),
                _ => false,
            },
            // A sealed nominal slot admits a value whose `ktype()` reports the same
            // `(set ptr, index)` identity — a per-variant newtype `Wrapped` value or a
            // `TypeConstructor` (`Result`) value.
            KType::SetRef { .. } => *self == obj.ktype(),
            // A union slot admits a value any of its members admits.
            KType::Union { members, .. } => members.iter().any(|m| m.matches_value(obj, types)),
            _ => *self == obj.ktype(),
        }
    }

    /// True iff a first-class type `t` (flowing in the type channel) satisfies this declared
    /// slot — the type-channel analog of [`matches_value`]. An `OfKind` slot is satisfied when its
    /// kind subsumes `t.kind_of()` (so `OfKind(Proper)` admits any proper type, including a
    /// `Tagged`/`NewType`-classified nominal, while the signature wall keeps `Proper` from
    /// admitting a signature); `Any` by anything; a signature *value* slot by structural identity.
    /// A `Signature` slot admits no first-class type here — it is a constraint on a module, and a
    /// module surfaces on the Object channel, matched by [`matches_value`]. Other concrete slots
    /// compare against the `OfKind(Proper)` dispatch identity a non-signature type carrier reports,
    /// so they admit no bare type value.
    pub fn matches_type(&self, t: &KType) -> bool {
        // The shallow dispatch identity a concrete slot compares against: a signature carries its
        // identity directly; every other type fills the `OfKind(Proper)` marker.
        let carrier_ktype = match t {
            KType::Signature { .. } => t.clone(),
            _ => KType::OfKind(KKind::ProperType),
        };
        match self {
            KType::Any => true,
            // A `Signature` slot is a constraint on a module, and a module surfaces on the Object
            // channel (matched by `matches_value`); a signature *value* is admitted by the
            // `OfKind(Signature)` wildcard, never here.
            KType::Signature { .. } => false,
            KType::OfKind(k) => k.admits(t.kind_of()),
            // A union slot is satisfied by any type its members are satisfied by.
            KType::Union { members, .. } => members.iter().any(|m| m.matches_type(t)),
            _ => *self == carrier_ktype,
        }
    }

    /// Per-value admissibility for a resolved [`Carried`] argument — the classifier the spliced
    /// arms of [`accepts_part`] delegate to, and what a spliced cell opens against at its own brand.
    /// The slot is owned data and the value (`'v`) is a region borrow, so every comparison is a
    /// verdict-only structural check (`== self`, `satisfied_by`, `same_nominal`), none of which
    /// needs the value's own lifetime. "Dispatch trusts the carried element type": a container's
    /// memoized carried `KType` is read via `satisfied_by`, never by walking its contents.
    pub fn accepts_carried<'v>(&self, c: Carried<'v>, types: &TypeRegistry) -> bool {
        match self {
            KType::Any => true,
            KType::Number => matches!(c, Carried::Object(KObject::Number(_))),
            KType::Str => matches!(c, Carried::Object(KObject::KString(_))),
            KType::Bool => matches!(c, Carried::Object(KObject::Bool(_))),
            KType::Null => matches!(c, Carried::Object(KObject::Null)),
            // Evaluated container: compare the memoized carried element/field type against the slot
            // via `satisfied_by` — pure type-level, no element walk.
            KType::List { element: elem, .. } => match c {
                Carried::Object(KObject::List(_, carried)) => elem.satisfied_by(carried, types),
                _ => false,
            },
            KType::Dict {
                key: k_ty,
                value: v_ty,
                ..
            } => match c {
                Carried::Object(KObject::Dict(_, carried_k, carried_v)) => {
                    k_ty.satisfied_by(carried_k, types) && v_ty.satisfied_by(carried_v, types)
                }
                _ => false,
            },
            KType::Record { .. } => match c {
                Carried::Object(KObject::Record(_, carried)) => {
                    self.satisfied_by(&KType::record(carried.clone()), types)
                }
                _ => false,
            },
            KType::KFunction { params, ret, .. } => match c {
                Carried::Object(KObject::KFunction(f)) => {
                    function_compat(&f.signature, params, ret, types)
                }
                _ => false,
            },
            // Part-shape-only slots (identifier / expression / type-expr / record-type) admit a
            // parser part shape, never a resolved value.
            KType::Identifier | KType::KExpression | KType::SigiledTypeExpr | KType::RecordType => {
                false
            }
            // Type-accepting slot, type-channel-only, by shallow kind via `kind_of` subsumption: a
            // first-class type value is admitted iff the slot kind subsumes the value's `kind_of`, so
            // `Any` takes every type value (signatures included), `Proper` takes any non-signature
            // type, `Signature` takes only its own carrier, and a nominal-kind slot only its own
            // family. An object value reports a non-type `kind_of` and is refused.
            KType::OfKind(k) => match c {
                Carried::Type(ty) => k.admits(ty.kind_of()),
                _ => false,
            },
            // Strict `(set ptr, index)` equality is the per-declaration identity check for a sealed
            // nominal type — `ktype()` yields a `SetRef` whose `PartialEq` keys on the shared
            // allocation and index. A per-variant newtype value carries that member `SetRef`, so a
            // union-typed slot admits each variant via the member delegation below.
            KType::SetRef { .. } => &c.ktype() == self,
            // A union slot admits an argument any of its members admits. `Carried` is `Copy`,
            // so each member reads the same carried value.
            KType::Union { members, .. } => members.iter().any(|m| m.accepts_carried(c, types)),
            KType::AbstractType { .. } => c.ktype() == *self,
            // Constraint role: a `:S` slot admits a *module* whose self-sig satisfies the
            // signature source (+ pinned-slot residue for a `WITH`-pinned slot) — no ascription
            // required. A module is a value, so both the overload-picker probe and the built
            // argument cell carry it on the Object channel. A signature *value* is admitted by the
            // `OfKind(Signature)` wildcard above, never here.
            KType::Signature {
                content,
                pinned_slots,
                ..
            } => match c {
                Carried::Object(KObject::Module(m)) => {
                    m.satisfies_sig_content(content, types)
                        && (pinned_slots.is_empty() || m.satisfies_pins(pinned_slots))
                }
                _ => false,
            },
            // Transient / intra-set leaves never reach a real argument slot: `RecursiveRef` is sealed
            // away before dispatch, and `SetLocal` only appears inside a member's schema.
            KType::RecursiveRef(_) | KType::Unresolved(_) => true,
            KType::SetLocal(_) => false,
            // A whole-set handle names a group of types, not a value type — it admits no argument.
            KType::RecursiveGroup(_) => false,
            // Confined to a synthesized FN `ret` slot — never a free-standing argument slot.
            KType::DeferredReturn(_) => false,
            // Two carriers satisfy a `ConstructorApply` slot: a first-class meta-type value with a
            // structurally-equal inner `KType`, and an identity-wrapper `Wrapped` object whose
            // `ktype()` is itself a `ConstructorApply` (a `NEWTYPE (Type AS Wrapper)`-constructed
            // value) — admitted by the same ctor-nominal + per-arg rule the `matches_value` arm uses.
            KType::ConstructorApply {
                ctor: slot_ctor,
                args: slot_args,
                ..
            } => match c {
                Carried::Type(kt) => kt == self,
                Carried::Object(obj) => match obj.ktype() {
                    KType::ConstructorApply {
                        ctor: value_ctor,
                        args: value_args,
                        ..
                    } => constructor_apply_admits(slot_ctor, slot_args, &value_ctor, &value_args),
                    _ => false,
                },
            },
        }
    }

    /// Classify a spliced **cell** against this slot without adopting it — opens the delivery
    /// envelope at a fresh brand under its retained host pin and routes the opened value through
    /// [`accepts_carried`](Self::accepts_carried) at that brand. No cast: the slot is owned data,
    /// so it carries no brand of its own for the opened value's brand to relate to — a
    /// verdict-only walk needs no re-anchoring. The picker may reject the candidate, so this
    /// deliberately does not adopt.
    pub(crate) fn accepts_cell(&self, cell: &DeliveredCarried, types: &TypeRegistry) -> bool {
        cell.open(|c| self.accepts_carried(c, types))
    }

    /// Per-`ExpressionPart` admissibility for argument slots. Unevaluated container
    /// literals admit shape-only (element types unknown until evaluation); a spliced cell
    /// ([`ExpressionPart::Spliced`]) classifies through [`accepts_cell`](Self::accepts_cell),
    /// which opens it at its own brand. Non-satisfying containers fall through the scope walk
    /// rather than failing the bind.
    pub fn accepts_part(&self, part: &ExpressionPart<'_>, types: &TypeRegistry) -> bool {
        // A spliced cell opens at its own brand through `accepts_cell`, which routes the opened
        // value through `accepts_carried` — no cast. Every remaining arm is a shape check on the
        // parser part, so no coercion of `part` is needed.
        if let ExpressionPart::Spliced { cell } = part {
            return self.accepts_cell(cell, types);
        }
        match self {
            KType::Any => true,
            KType::Number => matches!(part, ExpressionPart::Literal(KLiteral::Number(_))),
            KType::Str => matches!(part, ExpressionPart::Literal(KLiteral::String(_))),
            KType::Bool => matches!(part, ExpressionPart::Literal(KLiteral::Boolean(_))),
            KType::Null => matches!(part, ExpressionPart::Literal(KLiteral::Null)),
            // An unevaluated container literal admits shape-only (element types unknown until
            // evaluation, so two container-typed overloads tie and defer-then-reevaluate).
            KType::List { .. } => matches!(part, ExpressionPart::ListLiteral(_)),
            KType::Dict { .. } => matches!(part, ExpressionPart::DictLiteral(_)),
            KType::Record { .. } => matches!(part, ExpressionPart::RecordLiteral(_)),
            // A function slot admits no parser part shape — only a resolved value, handled
            // above by `accepts_carried`.
            KType::KFunction { .. } => false,
            KType::Identifier => matches!(part, ExpressionPart::Identifier(_)),
            // A `:KExpression` slot captures a parenthesized expression raw, and a `#(...)` quote —
            // whose body is already data — with it. It also captures a bare list literal raw, the
            // shape a `Unary`-mode operator run reduces to (`[Keyword, ListLiteral]`), so the
            // receiving builtin owns the operand run.
            KType::KExpression => matches!(
                part,
                ExpressionPart::Expression(_)
                    | ExpressionPart::QuotedExpression(_)
                    | ExpressionPart::ListLiteral(_)
            ),
            KType::SigiledTypeExpr => matches!(part, ExpressionPart::SigiledTypeExpr(_)),
            KType::RecordType => matches!(part, ExpressionPart::RecordType(_)),
            // A raw parser type token is a proper type name, admitted only for `Proper` / `Any`; a
            // first-class type *value* reaches `accepts_carried` above.
            KType::OfKind(k) => match part {
                ExpressionPart::Type(_) => matches!(k, KKind::ProperType | KKind::AnyType),
                _ => false,
            },
            // The nominal / signature / constructor slots classify only resolved values
            // (via `accepts_carried`); no parser part shape satisfies them. A union delegates to
            // its members, and a member admits a part only for a shape it classifies — a literal
            // for `Number` / `Str` / `Bool` / `Null`.
            KType::Union { members, .. } => members.iter().any(|m| m.accepts_part(part, types)),
            KType::SetRef { .. }
            | KType::AbstractType { .. }
            | KType::Signature { .. }
            | KType::ConstructorApply { .. } => false,
            // Transient / intra-set leaves never reach a real argument slot: `RecursiveRef` is
            // sealed away before dispatch (consumed by `Scope::resolve_type_identifier`), and
            // `SetLocal` only appears inside a member's schema.
            KType::RecursiveRef(_) | KType::Unresolved(_) => true,
            KType::SetLocal(_) => false,
            // A whole-set handle names a group of types, not a value type — it admits no argument;
            // the `RECURSIVE TYPES` group name is a reserved value-language seam.
            KType::RecursiveGroup(_) => false,
            // Confined to a synthesized FN `ret` slot — never a free-standing argument slot.
            KType::DeferredReturn(_) => false,
        }
    }
}

/// Strict cross-signature specificity for two signature types with DISTINCT `sig_id`s (SIG-declared
/// or self-sig, any combination). `a` is strictly more specific than `b` iff `a`'s pin-folded
/// schema is a `sig_subtype` of `b`'s pin-folded schema in the forward direction only — the reverse
/// must fail, or the two are mutually-satisfying (structurally equal) and neither strictly refines.
/// Both directions record a verdict under `SigSatisfies`, keyed by the two signature digests (which
/// fold their pins, so the key is exact).
fn sig_content_more_specific(
    a: &SigContent,
    pins_a: &[(String, KType)],
    digest_a: TypeDigest,
    b: &SigContent,
    pins_b: &[(String, KType)],
    digest_b: TypeDigest,
    types: &TypeRegistry,
) -> bool {
    let forward_hit = types.verdict(digest_a, digest_b, Relation::SigSatisfies);
    let reverse_hit = types.verdict(digest_b, digest_a, Relation::SigSatisfies);
    if let (Some(forward), Some(reverse)) = (forward_hit, reverse_hit) {
        return forward && !reverse;
    }
    // At least one direction missed: build both schemas once (the walk we're memoizing).
    let schema_a = a.schema.with_pins(pins_a);
    let schema_b = b.schema.with_pins(pins_b);
    // The key (digest_a, digest_b) is content-derived only if BOTH sides' pin types are
    // content-digested — a pin embedding an unsealed set makes the digest pointer-derived.
    let insertable = pins_a.iter().all(|(_, kt)| digest_is_content(kt))
        && pins_b.iter().all(|(_, kt)| digest_is_content(kt));
    let forward = forward_hit.unwrap_or_else(|| {
        let verdict = sig_subtype(&schema_a, &schema_b, types).is_ok();
        if insertable {
            types.record_verdict(digest_a, digest_b, Relation::SigSatisfies, verdict);
        }
        verdict
    });
    let reverse = reverse_hit.unwrap_or_else(|| {
        let verdict = sig_subtype(&schema_b, &schema_a, types).is_ok();
        if insertable {
            types.record_verdict(digest_b, digest_a, Relation::SigSatisfies, verdict);
        }
        verdict
    });
    forward && !reverse
}

/// Name-keyed specificity for the `KFunction` arm of
/// [`KType::is_more_specific_than`]. Function subtyping is
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
fn param_record_more_specific(
    pa: &Record<KType>,
    ra: &KType,
    pb: &Record<KType>,
    rb: &KType,
    types: &TypeRegistry,
) -> bool {
    if !pa.keys().all(|k| pb.get(k).is_some()) {
        return false;
    }
    let params_ok = pa.iter().all(|(name, s)| {
        let o = pb.get(name).unwrap();
        o == s || o.is_more_specific_than(s, types)
    });
    let params_more = pa.keys().any(|k| {
        pb.get(k)
            .unwrap()
            .is_more_specific_than(pa.get(k).unwrap(), types)
    });
    let ret_more = ra.is_more_specific_than(rb, types);
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
fn record_value_more_specific(a: &Record<KType>, b: &Record<KType>, types: &TypeRegistry) -> bool {
    if !b.keys().all(|k| a.get(k).is_some()) {
        return false;
    }
    let depth_ok = b.iter().all(|(name, bt)| {
        let at = a.get(name).unwrap();
        at == bt || at.is_more_specific_than(bt, types)
    });
    let depth_more = b.keys().any(|k| {
        a.get(k)
            .unwrap()
            .is_more_specific_than(b.get(k).unwrap(), types)
    });
    let width_strict = a.len() > b.len();
    depth_ok && (width_strict || depth_more)
}

/// Field→type-parameter linkage for the builtin `Result` parameterized union:
/// `Ok`→0 (`T`), `Error`→1 (`E`), mirroring the `param_names: ["T", "E"]` registered
/// in [`crate::builtins::result`]. Returns `None` for any other carrier — user UNIONs
/// don't yet carry runtime type arguments, so their `ConstructorApply` admission
/// falls back to a ctor-identity-only check.
pub fn result_field_param_index(carrier_name: &str, tag: &str) -> Option<usize> {
    match (carrier_name, tag) {
        ("Result", "Ok") => Some(0),
        ("Result", "Error") => Some(1),
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
///   [ktype/parameterization-and-variance.md § Variance](../../../../design/typing/ktype/parameterization-and-variance.md#variance).
/// - Params contravariant with width-drop: every `Argument` the value declares must
///   appear in `params` (a value-required param the slot doesn't promise is a width
///   violation → `false`); for a shared name, the slot's param must be equal-or-more-
///   specific than the value's (`slot_pt == &a.ktype || slot_pt ≺ &a.ktype`). Extra
///   slot params the value doesn't declare are fine — under call-by-name they arrive
///   unbound (width drop), so there is no exhaustiveness check.
pub(super) fn function_compat<'v>(
    sig: &ExpressionSignature<'v>,
    params: &Record<KType>,
    ret: &KType,
    types: &TypeRegistry,
) -> bool {
    use crate::machine::model::types::{DeferredReturnSurface, ReturnType};
    let ret_ok = match &sig.return_type {
        ReturnType::Resolved(kt) => kt == ret || kt.is_more_specific_than(ret, types),
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
                    if !(slot_pt == &a.ktype || slot_pt.is_more_specific_than(&a.ktype, types)) {
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
