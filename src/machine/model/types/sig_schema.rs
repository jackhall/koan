//! The signature-subtyping relation and the schema it is defined over.
//!
//! A [`SigSchema`] is the normalized carrier of a signature's shape — the abstract type
//! members, the manifest (fixed) type members, and the value slots — projected out of either
//! a [`ModuleSignature`] declaration ([`SigSchema::of_sig`]) or a [`Module`]'s own body
//! ([`SigSchema::raw_self_sig`], the self-sig). [`sig_subtype`] is the canonical relation:
//! `Sub <: Super` iff `Sub` supplies every member `Super` names, with each manifest member
//! equal, each abstract member present at the right kind/arity, and each value slot
//! covariantly compatible after abstract-member substitution.
//!
//! See [design/typing/modules.md](../../../../design/typing/modules.md).

use std::collections::HashMap;

use crate::machine::core::{Scope, ScopeId};

use super::kkind::KKind;
use super::ktype::KType;
use super::recursive_set::{ProjectedSchema, RecursiveSet};
use crate::machine::model::values::{Module, ModuleSignature};

/// Normalized signature schema — the carrier the subtyping relation is defined over.
///
/// Members are split by *representation*, not by surface syntax: an abstract member carries no
/// concrete witness (a [`KType::AbstractType`] or a sentinel type-constructor
/// `SetRef`), a manifest member fixes a concrete type. A module self-sig never has abstract
/// members — `TYPE` is a SIG-body-only construct.
#[derive(Clone)]
pub struct SigSchema<'a> {
    /// `Some(sig_id)` when derived from a SIG declaration — `Sig`-sourced abstract refs in
    /// value-slot types substitute against this id. `None` for a module self-sig (whose slot
    /// types name no SIG-decl-sourced refs).
    pub sig_id: Option<ScopeId>,
    /// Abstract type members: name → (the bound representation as found in the decl scope —
    /// the `AbstractType` or the sentinel constructor `SetRef` — and the constructor
    /// arity: `None` = first-order, `Some(n)` = higher-kinded taking `n` parameters).
    pub abstract_members: HashMap<String, (KType<'a>, Option<usize>)>,
    /// Manifest type members: name → the fixed type.
    pub manifest_members: HashMap<String, KType<'a>>,
    /// Value slots: name → declared (SIG) or derived (self-sig) type.
    pub value_slots: HashMap<String, KType<'a>>,
}

impl<'a> SigSchema<'a> {
    /// Project a SIG decl scope into its schema, at SIG finish. Every type-table entry is a
    /// genuine type member (the token-class partition holds — value slots live in the scope's
    /// slot collector, not in `types`), classified abstract/manifest by representation; the
    /// value slots come from the scope's own slot collector. The only place this
    /// classification runs — once per SIG.
    pub(crate) fn project_decl(decl_scope: &Scope<'a>) -> SigSchema<'a> {
        let mut abstract_members = HashMap::new();
        let mut manifest_members = HashMap::new();
        for (name, kt) in decl_scope.bindings().iter_types() {
            if is_abstract_sig_member(kt) {
                abstract_members.insert(name, (kt.clone(), constructor_arity(kt)));
            } else {
                manifest_members.insert(name, kt.clone());
            }
        }
        let mut value_slots = HashMap::new();
        for (name, kt) in decl_scope.sig_value_slots() {
            value_slots.insert(name, kt.clone());
        }
        SigSchema {
            sig_id: Some(decl_scope.id),
            abstract_members,
            manifest_members,
            value_slots,
        }
    }

    /// Apply `WITH` pins to a SIG's stored schema: clone it and convert each pinned abstract
    /// member into a manifest one fixed to the pin's type (a pin naming an already-manifest
    /// member overwrites it — unreachable through `WITH`, which normalizes equal pins away and
    /// errors on unequal ones).
    pub fn of_sig(sig: &ModuleSignature<'a>, pins: &[(String, KType<'a>)]) -> SigSchema<'a> {
        let mut schema = sig.schema().clone();
        for (name, kt) in pins {
            schema.abstract_members.remove(name);
            schema.manifest_members.insert(name.clone(), kt.clone());
        }
        schema
    }

    /// Derive a module's principal signature (self-sig) directly from its body.
    ///
    /// A module never carries abstract members. The manifest members are the union of the
    /// module's `type_members` map (the per-call mints + mirrored manifests an ascription
    /// installs) and the child scope's type-class entries — the map wins on a shared name, so
    /// this covers a plain module (map ∪ scope agree), an opaque view (map only — the view
    /// scope carries no type entries), and a transparent view (scope only — the map is empty).
    /// Value slots are the child scope's data bindings read through [`KObject::ktype`], with the
    /// `slot_type_tags` map overriding by name (an opaque view's abstract slot identities).
    ///
    /// [`KObject::ktype`]: crate::machine::model::values::KObject::ktype
    pub fn raw_self_sig(module: &Module<'a>) -> SigSchema<'a> {
        let child = module.child_scope();
        let mut manifest_members: HashMap<String, KType<'a>> = HashMap::new();
        for (name, kt) in child.bindings().iter_types() {
            manifest_members.insert(name, kt.clone());
        }
        for (name, kt) in module.type_members.borrow().iter() {
            manifest_members.insert(name.clone(), kt.clone());
        }
        let mut value_slots: HashMap<String, KType<'a>> = HashMap::new();
        for (name, obj) in child.bindings().iter_data() {
            value_slots.insert(name, obj.ktype());
        }
        for (name, tag) in module.slot_type_tags.borrow().iter() {
            value_slots.insert(name.clone(), tag.clone());
        }
        SigSchema {
            sig_id: None,
            abstract_members: HashMap::new(),
            manifest_members,
            value_slots,
        }
    }
}

/// `Some(param count)` iff `kt` is a `TypeConstructor`-kind `SetRef` (sentinel or real); `None`
/// for a first-order type. The arity is the length of the projected constructor's parameter
/// list.
pub(crate) fn constructor_arity(kt: &KType<'_>) -> Option<usize> {
    match kt {
        KType::SetRef { set, index } if set.member(*index).kind == KKind::TypeConstructor => {
            match RecursiveSet::projected_schema(set, *index) {
                ProjectedSchema::TypeConstructor { param_names, .. } => Some(param_names.len()),
                ProjectedSchema::NewType(_) => None,
            }
        }
        _ => None,
    }
}

/// Rewrite `kt`, replacing references to `sig_id`'s abstract members with the caller's bindings
/// for them. Returns a plain value used only for comparison — never region-allocated.
///
/// Two reference shapes substitute: a first-order `AbstractType { source: sig_id, name }`
/// slot type, and a sentinel type-constructor `SetRef` naming a higher-kinded member (e.g. the
/// ctor position of a `ConstructorApply`). Compound types recurse; every other variant is a
/// clone.
pub fn substitute_sig_members<'a>(
    kt: &KType<'a>,
    sig_id: ScopeId,
    members: &HashMap<String, KType<'a>>,
) -> KType<'a> {
    match kt {
        KType::AbstractType { source, name } if *source == sig_id => {
            members.get(name).cloned().unwrap_or_else(|| kt.clone())
        }
        KType::SetRef { set, index }
            if set.member(*index).kind == KKind::TypeConstructor
                && set.member(*index).scope_id == ScopeId::SENTINEL
                && members.contains_key(&set.member(*index).name) =>
        {
            members[&set.member(*index).name].clone()
        }
        KType::List { element, .. } => {
            KType::list(Box::new(substitute_sig_members(element, sig_id, members)))
        }
        KType::Dict { key, value, .. } => KType::dict(
            Box::new(substitute_sig_members(key, sig_id, members)),
            Box::new(substitute_sig_members(value, sig_id, members)),
        ),
        KType::Record { fields, .. } => KType::record(Box::new(
            fields.map(|v| substitute_sig_members(v, sig_id, members)),
        )),
        KType::KFunction { params, ret, .. } => KType::function_type(
            params.map(|v| substitute_sig_members(v, sig_id, members)),
            Box::new(substitute_sig_members(ret, sig_id, members)),
        ),
        KType::Union { members: us, .. } => KType::union_of(
            us.iter()
                .map(|m| substitute_sig_members(m, sig_id, members))
                .collect(),
        ),
        KType::ConstructorApply { ctor, args, .. } => KType::constructor_apply(
            Box::new(substitute_sig_members(ctor, sig_id, members)),
            args.iter()
                .map(|a| substitute_sig_members(a, sig_id, members))
                .collect(),
        ),
        _ => kt.clone(),
    }
}

/// Why a [`sig_subtype`] check failed — one of the five per-member rules, carrying the offending
/// member name and the *rendered* types that disagreed. The disagreeing types come from the `sub`
/// and `sup` schemas, which carry independent lifetimes; rendering them to `String` at the failure
/// site (the only thing [`Self::render_fragment`] ever does with them) keeps this type lifetime-free
/// so the heterogeneous [`sig_subtype`] can return it without unifying the two lifetimes.
pub enum SigSubtypeFailure {
    MissingTypeMember {
        name: String,
    },
    ManifestMismatch {
        name: String,
        got: String,
        expected: String,
    },
    /// A type member's kind/arity disagreed. `expected_arity` is `Some(n)` when the super
    /// signature declares a constructor taking `n` parameters, `None` when it declares a
    /// first-order proper type; `got` is the rendered sub binding that failed to match.
    KindMismatch {
        name: String,
        expected_arity: Option<usize>,
        got: String,
    },
    MissingValueSlot {
        name: String,
    },
    ValueSlotMismatch {
        name: String,
        got: String,
        expected: String,
    },
}

impl SigSubtypeFailure {
    /// Render the failure as the message fragment an ascription error embeds after
    /// `` module does not satisfy signature `{path}`: ``.
    pub fn render_fragment(&self) -> String {
        match self {
            SigSubtypeFailure::MissingTypeMember { name } => {
                format!("missing type member `{name}`")
            }
            SigSubtypeFailure::ManifestMismatch {
                name,
                got,
                expected,
            } => format!(
                "type member `{name}` is `{got}` but the signature fixes it to `{expected}`"
            ),
            SigSubtypeFailure::KindMismatch {
                name,
                expected_arity: Some(n),
                got,
            } => format!(
                "type member `{name}` must be a type constructor taking {n} parameter(s), got `{got}`"
            ),
            SigSubtypeFailure::KindMismatch {
                name,
                expected_arity: None,
                got,
            } => format!(
                "type member `{name}` must be a proper type, got the type constructor `{got}`"
            ),
            SigSubtypeFailure::MissingValueSlot { name } => format!("missing member `{name}`"),
            SigSubtypeFailure::ValueSlotMismatch {
                name,
                got,
                expected,
            } => format!(
                "member `{name}` has type `{got}` but the signature declares `{expected}`"
            ),
        }
    }
}

/// The canonical signature-subtyping relation: `sub <: sup`. Ok iff `sub` supplies every member
/// `sup` names (width — members `sup` does not name are ignored), with each manifest member
/// equal, each abstract member present at the matching kind/arity, and each value slot
/// covariantly compatible after substituting `sup`'s abstract members with `sub`'s bindings.
///
/// The failure is boxed: `SigSubtypeFailure` carries `KType`s and is large relative to the
/// common `Ok` path.
pub fn sig_subtype<'s, 'p>(
    sub: &SigSchema<'s>,
    sup: &SigSchema<'p>,
) -> Result<(), Box<SigSubtypeFailure>> {
    // 1. Abstract members: present at the matching kind/arity (manifest or abstract in `sub`).
    for (name, (_, sup_arity)) in &sup.abstract_members {
        let (sub_repr, sub_arity) = if let Some(kt) = sub.manifest_members.get(name) {
            (kt.render(), constructor_arity(kt))
        } else if let Some((kt, arity)) = sub.abstract_members.get(name) {
            (kt.render(), *arity)
        } else {
            return Err(Box::new(SigSubtypeFailure::MissingTypeMember {
                name: name.clone(),
            }));
        };
        if sub_arity != *sup_arity {
            return Err(Box::new(SigSubtypeFailure::KindMismatch {
                name: name.clone(),
                expected_arity: *sup_arity,
                got: sub_repr,
            }));
        }
    }

    // 2. Manifest members: present manifest in `sub` with an equal type.
    for (name, fixed) in &sup.manifest_members {
        match sub.manifest_members.get(name) {
            Some(got) if got == fixed => {}
            Some(got) => {
                return Err(Box::new(SigSubtypeFailure::ManifestMismatch {
                    name: name.clone(),
                    got: got.render(),
                    expected: fixed.render(),
                }))
            }
            None => {
                // An abstract `sub` member supplies no witness for a manifest requirement.
                if let Some((repr, _)) = sub.abstract_members.get(name) {
                    return Err(Box::new(SigSubtypeFailure::ManifestMismatch {
                        name: name.clone(),
                        got: repr.render(),
                        expected: fixed.render(),
                    }));
                }
                return Err(Box::new(SigSubtypeFailure::MissingTypeMember {
                    name: name.clone(),
                }));
            }
        }
    }

    // 3. Value slots: present and covariantly compatible after abstract-member substitution.
    // The substitution binds every `sub` type-member name to its representation, so a `sup` slot
    // referencing one of `sup`'s abstract members reads through `sub`'s binding for it. `sub` and
    // `sup` carry independent lifetimes, so the substituted type would mix `'s` and `'p` content —
    // unrepresentable. `slot_satisfied_by` computes the same verdict as
    // `substitute_sig_members(declared, id, sub_member_map).satisfied_by(sub_type)` by comparing
    // structurally and swapping in `sub`'s binding on reaching a self-abstract reference, so no
    // mixed type is ever built.
    let mut sub_member_map: HashMap<String, KType<'s>> = HashMap::new();
    for (name, kt) in &sub.manifest_members {
        sub_member_map.insert(name.clone(), kt.clone());
    }
    for (name, (repr, _)) in &sub.abstract_members {
        sub_member_map.insert(name.clone(), repr.clone());
    }
    for (name, declared) in &sup.value_slots {
        let Some(sub_type) = sub.value_slots.get(name) else {
            return Err(Box::new(SigSubtypeFailure::MissingValueSlot {
                name: name.clone(),
            }));
        };
        let ok = match sup.sig_id {
            Some(id) => slot_satisfied_by(declared, sub_type, &sub_member_map, id),
            // No `sig_id`: nothing to substitute, so the heterogeneous `satisfied_by` is exact.
            None => declared.satisfied_by(sub_type),
        };
        if !ok {
            return Err(Box::new(SigSubtypeFailure::ValueSlotMismatch {
                name: name.clone(),
                got: sub_type.render(),
                expected: declared.render(),
            }));
        }
    }
    Ok(())
}

/// True iff `declared` contains a reference to one of `sig_id`'s abstract members that
/// [`substitute_sig_members`] would rewrite (a first-order `AbstractType`, or a sentinel
/// type-constructor `SetRef`, whose name `members` binds). When false, substitution is the
/// identity and a plain heterogeneous compare on `declared` is exact.
fn references_sig_member<'p, 's>(
    declared: &KType<'p>,
    sig_id: ScopeId,
    members: &HashMap<String, KType<'s>>,
) -> bool {
    match declared {
        KType::AbstractType { source, name } => *source == sig_id && members.contains_key(name),
        KType::SetRef { set, index } => {
            let m = set.member(*index);
            m.kind == KKind::TypeConstructor
                && m.scope_id == ScopeId::SENTINEL
                && members.contains_key(&m.name)
        }
        KType::List { element, .. } => references_sig_member(element, sig_id, members),
        KType::Dict { key, value, .. } => {
            references_sig_member(key, sig_id, members)
                || references_sig_member(value, sig_id, members)
        }
        KType::Record { fields, .. } => fields
            .values()
            .any(|v| references_sig_member(v, sig_id, members)),
        KType::KFunction { params, ret, .. } => {
            params
                .values()
                .any(|v| references_sig_member(v, sig_id, members))
                || references_sig_member(ret, sig_id, members)
        }
        KType::Union { members: us, .. } => {
            us.iter().any(|m| references_sig_member(m, sig_id, members))
        }
        KType::ConstructorApply { ctor, args, .. } => {
            references_sig_member(ctor, sig_id, members)
                || args
                    .iter()
                    .any(|a| references_sig_member(a, sig_id, members))
        }
        _ => false,
    }
}

/// The `sub`-side binding a substitution point in `declared` resolves to, if any — the type
/// `substitute_sig_members` would splice in for this node.
fn substitution_binding<'p, 's, 'm>(
    declared: &KType<'p>,
    sig_id: ScopeId,
    members: &'m HashMap<String, KType<'s>>,
) -> Option<&'m KType<'s>> {
    match declared {
        KType::AbstractType { source, name } if *source == sig_id => members.get(name),
        KType::SetRef { set, index } => {
            let m = set.member(*index);
            if m.kind == KKind::TypeConstructor && m.scope_id == ScopeId::SENTINEL {
                members.get(&m.name)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Verdict of `substitute_sig_members(declared, sig_id, members).satisfied_by(sub_type)` — does the
/// `sub` value slot fill the substituted `sup` slot? — computed without materializing the
/// mixed-lifetime substituted type. `declared` (the `sup` slot) rides `'p`; `sub_type` and the
/// member bindings ride `'s`. On reaching a self-abstract reference the walk switches to a pure-`'s`
/// compare against `sub`'s binding; on a member-free node it falls to the heterogeneous
/// `satisfied_by`; otherwise it descends the shared container structure with the same covariance
/// [`KType::satisfied_by`] applies (`Dict`/`Record`/`KFunction` component rules included).
fn slot_satisfied_by<'p, 's>(
    declared: &KType<'p>,
    sub_type: &KType<'s>,
    members: &HashMap<String, KType<'s>>,
    sig_id: ScopeId,
) -> bool {
    if let Some(binding) = substitution_binding(declared, sig_id, members) {
        return binding.satisfied_by(sub_type);
    }
    if !references_sig_member(declared, sig_id, members) {
        return declared.satisfied_by(sub_type);
    }
    match (declared, sub_type) {
        (KType::List { element: ed, .. }, KType::List { element: es, .. }) => {
            slot_satisfied_by(ed, es, members, sig_id)
        }
        (
            KType::Dict {
                key: kd, value: vd, ..
            },
            KType::Dict {
                key: ks, value: vs, ..
            },
        ) => {
            slot_satisfied_by(kd, ks, members, sig_id) && slot_satisfied_by(vd, vs, members, sig_id)
        }
        (KType::Record { fields: fd, .. }, KType::Record { fields: fs, .. }) => {
            // Record-value covariance: every slot field present in the value, covariantly.
            fd.iter().all(|(name, dt)| {
                fs.get(name)
                    .is_some_and(|st| slot_satisfied_by(dt, st, members, sig_id))
            })
        }
        (
            KType::ConstructorApply {
                ctor: cd, args: ad, ..
            },
            KType::ConstructorApply {
                ctor: cs,
                args: as_,
                ..
            },
        ) => {
            ad.len() == as_.len()
                && slot_types_equal(cd, cs, members, sig_id)
                && ad
                    .iter()
                    .zip(as_.iter())
                    .all(|(d, s)| slot_satisfied_by(d, s, members, sig_id))
        }
        (
            KType::KFunction {
                params: pd,
                ret: rd,
                ..
            },
            KType::KFunction {
                params: ps,
                ret: rs,
                ..
            },
        ) => {
            // Contravariant params (width-drop): every value param names a slot param the
            // substituted slot fixes equal-or-more-specific. Covariant return.
            ps.keys().all(|k| pd.get(k).is_some())
                && ps.iter().all(|(name, sp)| {
                    pd.get(name)
                        .is_some_and(|dp| slot_more_specific_or_equal(dp, sp, members, sig_id))
                })
                && slot_satisfied_by(rd, rs, members, sig_id)
        }
        (KType::Union { members: ud, .. }, _) => {
            // A value satisfies a substituted union slot iff it (each of its members, if it is
            // itself a union) refines some slot member — the union-membership rule of `satisfied_by`.
            let ys: Vec<&KType<'s>> = match sub_type {
                KType::Union { members: us, .. } => us.iter().collect(),
                other => vec![other],
            };
            ys.iter().all(|y| {
                ud.iter()
                    .any(|md| slot_satisfied_by(md, y, members, sig_id))
            })
        }
        _ => false,
    }
}

/// Verdict of `substitute_sig_members(declared, ...) == target
/// || substitute_sig_members(declared, ...).is_more_specific_than(target)` — the contravariant
/// direction [`slot_satisfied_by`] needs for a function parameter, computed without building the
/// substituted type.
fn slot_more_specific_or_equal<'p, 's>(
    declared: &KType<'p>,
    target: &KType<'s>,
    members: &HashMap<String, KType<'s>>,
    sig_id: ScopeId,
) -> bool {
    if let Some(binding) = substitution_binding(declared, sig_id, members) {
        return binding == target || binding.is_more_specific_than(target);
    }
    if !references_sig_member(declared, sig_id, members) {
        return declared == target || declared.is_more_specific_than(target);
    }
    // The substituted slot outranks `Any` / an unconstrained name, and refines a union it has a
    // member in — the top guards of `more_specific_walk`, mirrored here.
    match target {
        KType::Any => return true,
        KType::Identifier | KType::OfKind(KKind::ProperType) => return true,
        KType::Union { members: ts, .. } => {
            return ts
                .iter()
                .any(|t| slot_more_specific_or_equal(declared, t, members, sig_id))
        }
        _ => {}
    }
    match (declared, target) {
        (KType::List { element: ed, .. }, KType::List { element: et, .. }) => {
            slot_more_specific_or_equal(ed, et, members, sig_id)
        }
        (
            KType::Dict {
                key: kd, value: vd, ..
            },
            KType::Dict {
                key: kt, value: vt, ..
            },
        ) => {
            slot_more_specific_or_equal(kd, kt, members, sig_id)
                && slot_more_specific_or_equal(vd, vt, members, sig_id)
        }
        (KType::Record { fields: fd, .. }, KType::Record { fields: ft, .. }) => {
            // Record-value covariance with width-superset: the more-specific record has every
            // field of `target`, each covariantly refined.
            ft.keys().all(|k| fd.get(k).is_some())
                && ft.iter().all(|(name, tt)| {
                    fd.get(name)
                        .is_some_and(|dt| slot_more_specific_or_equal(dt, tt, members, sig_id))
                })
        }
        (
            KType::ConstructorApply {
                ctor: cd, args: ad, ..
            },
            KType::ConstructorApply {
                ctor: ct, args: at, ..
            },
        ) => {
            ad.len() == at.len()
                && slot_types_equal(cd, ct, members, sig_id)
                && ad
                    .iter()
                    .zip(at.iter())
                    .all(|(d, t)| slot_more_specific_or_equal(d, t, members, sig_id))
        }
        (
            KType::KFunction {
                params: pd,
                ret: rd,
                ..
            },
            KType::KFunction {
                params: pt,
                ret: rt,
                ..
            },
        ) => {
            // Contravariant params, covariant return — the dual of the `slot_satisfied_by` case.
            pd.keys().all(|k| pt.get(k).is_some())
                && pd.iter().all(|(name, dp)| {
                    pt.get(name)
                        .is_some_and(|tp| slot_satisfied_by(dp, tp, members, sig_id))
                })
                && slot_more_specific_or_equal(rd, rt, members, sig_id)
        }
        _ => false,
    }
}

/// Verdict of `substitute_sig_members(declared, ...) == other` — structural equality with `sub`'s
/// bindings spliced in. Only the constructor identity of a `ConstructorApply` needs this (a
/// constructor is a leaf `SetRef`, so the recursion bottoms out immediately in practice).
fn slot_types_equal<'p, 's>(
    declared: &KType<'p>,
    other: &KType<'s>,
    members: &HashMap<String, KType<'s>>,
    sig_id: ScopeId,
) -> bool {
    if let Some(binding) = substitution_binding(declared, sig_id, members) {
        return binding == other;
    }
    if !references_sig_member(declared, sig_id, members) {
        return declared == other;
    }
    match (declared, other) {
        (KType::List { element: ed, .. }, KType::List { element: eo, .. }) => {
            slot_types_equal(ed, eo, members, sig_id)
        }
        (
            KType::Dict {
                key: kd, value: vd, ..
            },
            KType::Dict {
                key: ko, value: vo, ..
            },
        ) => slot_types_equal(kd, ko, members, sig_id) && slot_types_equal(vd, vo, members, sig_id),
        (KType::Record { fields: fd, .. }, KType::Record { fields: fo, .. }) => {
            fd.len() == fo.len()
                && fd.iter().all(|(name, dt)| {
                    fo.get(name)
                        .is_some_and(|ot| slot_types_equal(dt, ot, members, sig_id))
                })
        }
        (
            KType::ConstructorApply {
                ctor: cd, args: ad, ..
            },
            KType::ConstructorApply {
                ctor: co, args: ao, ..
            },
        ) => {
            ad.len() == ao.len()
                && slot_types_equal(cd, co, members, sig_id)
                && ad
                    .iter()
                    .zip(ao.iter())
                    .all(|(d, o)| slot_types_equal(d, o, members, sig_id))
        }
        (
            KType::KFunction {
                params: pd,
                ret: rd,
                ..
            },
            KType::KFunction {
                params: po,
                ret: ro,
                ..
            },
        ) => {
            pd.len() == po.len()
                && pd.iter().all(|(name, dt)| {
                    po.get(name)
                        .is_some_and(|ot| slot_types_equal(dt, ot, members, sig_id))
                })
                && slot_types_equal(rd, ro, members, sig_id)
        }
        (KType::Union { members: ud, .. }, KType::Union { members: uo, .. }) => {
            ud.len() == uo.len()
                && ud.iter().all(|dm| {
                    uo.iter()
                        .any(|om| slot_types_equal(dm, om, members, sig_id))
                })
        }
        _ => false,
    }
}

/// Classify a SIG type-table entry by its *representation*: an abstract member carries no
/// concrete witness. Two abstract shapes — a [`KType::AbstractType`] (the first-order `TYPE Elt`
/// slot, sourced at the SIG decl scope) and a sentinel [`KKind::TypeConstructor`] `SetRef` (the
/// higher-kinded `TYPE (Type AS Wrap)` slot, `ScopeId::SENTINEL` marking it "awaiting per-call
/// mint"). Everything else — a manifest `LET Tag = Number` binding a concrete type, a real
/// minted constructor — is manifest.
pub(crate) fn is_abstract_sig_member(kt: &KType<'_>) -> bool {
    match kt {
        KType::AbstractType { .. } => true,
        KType::SetRef { set, index } => {
            let member = set.member(*index);
            member.kind == KKind::TypeConstructor && member.scope_id == ScopeId::SENTINEL
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests;
