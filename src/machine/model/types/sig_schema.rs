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

use crate::machine::core::ScopeId;

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
    /// Project a SIG declaration into a schema, then apply `WITH` pins.
    ///
    /// The decl scope's type table splits by name class (`is_type_name`): type-class names are
    /// members (abstract / manifest via [`is_abstract_sig_member`]), value-class names are value
    /// slots recording their declared type. A pin `(name, kt)` converts an abstract member to a
    /// manifest one fixed to `kt` (a pin naming an already-manifest member overwrites it —
    /// unreachable through `WITH`, which normalizes equal pins away and errors on unequal ones).
    pub fn of_sig(sig: &ModuleSignature<'a>, pins: &[(String, KType<'a>)]) -> SigSchema<'a> {
        let mut abstract_members = HashMap::new();
        let mut manifest_members = HashMap::new();
        let mut value_slots = HashMap::new();
        for (name, kt) in sig.decl_scope().bindings().iter_types() {
            if crate::parse::is_type_name(&name) {
                if is_abstract_sig_member(kt) {
                    abstract_members.insert(name, (kt.clone(), constructor_arity(kt)));
                } else {
                    manifest_members.insert(name, kt.clone());
                }
            } else {
                value_slots.insert(name, kt.clone());
            }
        }
        for (name, kt) in pins {
            abstract_members.remove(name);
            manifest_members.insert(name.clone(), kt.clone());
        }
        SigSchema {
            sig_id: Some(sig.sig_id()),
            abstract_members,
            manifest_members,
            value_slots,
        }
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
            if crate::parse::is_type_name(&name) {
                manifest_members.insert(name, kt.clone());
            }
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
/// member name and the types that disagreed.
pub enum SigSubtypeFailure<'a> {
    MissingTypeMember {
        name: String,
    },
    ManifestMismatch {
        name: String,
        got: KType<'a>,
        expected: KType<'a>,
    },
    /// A type member's kind/arity disagreed. `expected_arity` is `Some(n)` when the super
    /// signature declares a constructor taking `n` parameters, `None` when it declares a
    /// first-order proper type; `got` is the sub binding that failed to match.
    KindMismatch {
        name: String,
        expected_arity: Option<usize>,
        got: KType<'a>,
    },
    MissingValueSlot {
        name: String,
    },
    ValueSlotMismatch {
        name: String,
        got: KType<'a>,
        expected: KType<'a>,
    },
}

impl SigSubtypeFailure<'_> {
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
                "type member `{}` is `{}` but the signature fixes it to `{}`",
                name,
                got.render(),
                expected.render()
            ),
            SigSubtypeFailure::KindMismatch {
                name,
                expected_arity: Some(n),
                got,
            } => format!(
                "type member `{}` must be a type constructor taking {} parameter(s), got `{}`",
                name,
                n,
                got.render()
            ),
            SigSubtypeFailure::KindMismatch {
                name,
                expected_arity: None,
                got,
            } => format!(
                "type member `{}` must be a proper type, got the type constructor `{}`",
                name,
                got.render()
            ),
            SigSubtypeFailure::MissingValueSlot { name } => format!("missing member `{name}`"),
            SigSubtypeFailure::ValueSlotMismatch {
                name,
                got,
                expected,
            } => format!(
                "member `{}` has type `{}` but the signature declares `{}`",
                name,
                got.render(),
                expected.render()
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
pub fn sig_subtype<'a>(
    sub: &SigSchema<'a>,
    sup: &SigSchema<'a>,
) -> Result<(), Box<SigSubtypeFailure<'a>>> {
    // 1. Abstract members: present at the matching kind/arity (manifest or abstract in `sub`).
    for (name, (_, sup_arity)) in &sup.abstract_members {
        let (sub_repr, sub_arity) = if let Some(kt) = sub.manifest_members.get(name) {
            (kt.clone(), constructor_arity(kt))
        } else if let Some((kt, arity)) = sub.abstract_members.get(name) {
            (kt.clone(), *arity)
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
                    got: got.clone(),
                    expected: fixed.clone(),
                }))
            }
            None => {
                // An abstract `sub` member supplies no witness for a manifest requirement.
                if let Some((repr, _)) = sub.abstract_members.get(name) {
                    return Err(Box::new(SigSubtypeFailure::ManifestMismatch {
                        name: name.clone(),
                        got: repr.clone(),
                        expected: fixed.clone(),
                    }));
                }
                return Err(Box::new(SigSubtypeFailure::MissingTypeMember {
                    name: name.clone(),
                }));
            }
        }
    }

    // 3. Value slots: present and covariantly compatible after abstract-member substitution.
    // The substitution map binds every `sub` type-member name to its representation, so a
    // `sup` slot referencing an abstract member reads through `sub`'s binding for it.
    let mut sub_member_map: HashMap<String, KType<'a>> = HashMap::new();
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
        let expected = match sup.sig_id {
            Some(id) => substitute_sig_members(declared, id, &sub_member_map),
            None => declared.clone(),
        };
        if !expected.satisfied_by(sub_type) {
            return Err(Box::new(SigSubtypeFailure::ValueSlotMismatch {
                name: name.clone(),
                got: sub_type.clone(),
                expected,
            }));
        }
    }
    Ok(())
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

/// Type-class-named type-table entries that classify abstract by [`is_abstract_sig_member`].
/// Value slots (`VAL …`, value-class names) filter out by name class.
pub(crate) fn abstract_members_of<'a>(scope: &crate::machine::Scope<'a>) -> Vec<String> {
    scope
        .bindings()
        .iter_types()
        .into_iter()
        .filter(|(n, kt)| crate::parse::is_type_name(n) && is_abstract_sig_member(kt))
        .map(|(n, _)| n)
        .collect()
}

/// Type-class-named type-table entries that classify manifest (the concrete witness a
/// satisfying module must match), paired with their fixed `KType`.
pub(crate) fn manifest_type_members_of<'a>(
    scope: &crate::machine::Scope<'a>,
) -> Vec<(String, KType<'a>)> {
    scope
        .bindings()
        .iter_types()
        .into_iter()
        .filter(|(n, kt)| crate::parse::is_type_name(n) && !is_abstract_sig_member(kt))
        .map(|(n, kt)| (n, kt.clone()))
        .collect()
}

#[cfg(test)]
mod tests;
