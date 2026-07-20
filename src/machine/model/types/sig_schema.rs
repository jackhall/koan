//! The signature-subtyping relation and the schema it is defined over.
//!
//! A [`SigSchema`] is the normalized carrier of a signature's shape — the abstract type
//! members, the manifest (fixed) type members, and the value slots — projected out of either
//! a SIG declaration ([`SigSchema::project_decl`]) or a [`Module`]'s own body
//! ([`SigSchema::raw_self_sig`], the self-sig). [`sig_subtype`] is the canonical relation:
//! `Sub <: Super` iff `Sub` supplies every member `Super` names, with each manifest member
//! equal, each abstract member present at the right kind and over the same parameter names, and
//! each value slot covariantly compatible after abstract-member substitution.
//!
//! [`SigContent`] is the owned bundle a `KType::Signature` carries: a schema plus the
//! diagnostic path and same-declaration `sig_id` a `KType::Signature` needs alongside it.
//!
//! See [design/typing/modules.md](../../../../design/typing/modules.md).

use std::collections::HashMap;

use crate::machine::core::{Scope, ScopeId};

use super::kkind::KKind;
use super::ktype::KType;
use super::recursive_set::{ProjectedSchema, RecursiveSet};
use super::registry::TypeRegistry;
use super::type_digest::{empty_schema_digest, schema_content_digest, TypeDigest};
use crate::machine::model::values::Module;

/// Normalized signature schema — the carrier the subtyping relation is defined over.
///
/// Members are split by *representation*, not by surface syntax: an abstract member carries no
/// concrete witness (a [`KType::AbstractType`], of either order), a manifest member fixes a
/// concrete type. A module self-sig never has abstract members — `TYPE` is a SIG-body-only
/// construct.
#[derive(Clone)]
pub struct SigSchema {
    /// `Some(sig_id)` when derived from a SIG declaration — `Sig`-sourced abstract refs in
    /// value-slot types substitute against this id. `None` for a module self-sig (whose slot
    /// types name no SIG-decl-sourced refs).
    pub sig_id: Option<ScopeId>,
    /// Abstract type members: name → the bound `AbstractType` as found in the decl scope. Its
    /// `param_names` carry the member's order (empty = first-order, non-empty = a constructor
    /// over those parameters), read on demand through [`constructor_param_names`].
    pub abstract_members: HashMap<String, KType>,
    /// Manifest type members: name → the fixed type.
    pub manifest_members: HashMap<String, KType>,
    /// Value slots: name → declared (SIG) or derived (self-sig) type.
    pub value_slots: HashMap<String, KType>,
}

impl SigSchema {
    /// Project a SIG decl scope into its schema, at SIG finish. Every type-table entry is a
    /// genuine type member (the token-class partition holds — value slots live in the scope's
    /// slot collector, not in `types`), classified abstract/manifest by representation; the
    /// value slots come from the scope's own slot collector. The only place this
    /// classification runs — once per SIG.
    pub(crate) fn project_decl(decl_scope: &Scope<'_>) -> SigSchema {
        let mut abstract_members = HashMap::new();
        let mut manifest_members = HashMap::new();
        for (name, kt) in decl_scope.bindings().iter_types() {
            if is_abstract_sig_member(kt) {
                abstract_members.insert(name, kt.clone());
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

    /// Apply `WITH` pins to a schema: clone it and convert each pinned abstract member into a
    /// manifest one fixed to the pin's type (a pin naming an already-manifest member overwrites
    /// it — unreachable through `WITH`, which normalizes equal pins away and errors on unequal
    /// ones). A no-op clone when `pins` is empty (a self-sig or `:Module`, which carry no
    /// abstract members to fold).
    pub fn with_pins(&self, pins: &[(String, KType)]) -> SigSchema {
        let mut schema = self.clone();
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
    pub fn raw_self_sig(module: &Module<'_>) -> SigSchema {
        let child = module.child_scope();
        let mut manifest_members: HashMap<String, KType> = HashMap::new();
        for (name, kt) in child.bindings().iter_types() {
            manifest_members.insert(name, kt.clone());
        }
        for (name, kt) in module.type_members.borrow().iter() {
            manifest_members.insert(name.clone(), kt.clone());
        }
        let mut value_slots: HashMap<String, KType> = HashMap::new();
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

/// Owned content of a `KType::Signature` — everything the type carries about the interface it
/// names. Shared by `Rc` until interning replaces the transport.
pub struct SigContent {
    /// Diagnostic-only path label ("Ordered", "int_ord", "Module").
    pub path: String,
    /// Same-declaration key for WITH-pin specificity refinement; [`ScopeId::SENTINEL`] for the
    /// scopeless `:Module` mint. Never part of identity.
    pub sig_id: ScopeId,
    pub schema: SigSchema,
    /// [`schema_content_digest`] of `schema`, computed once at construction.
    pub schema_digest: TypeDigest,
}

impl SigContent {
    pub fn new(path: String, sig_id: ScopeId, schema: SigSchema) -> Self {
        let schema_digest = schema_content_digest(&schema);
        SigContent {
            path,
            sig_id,
            schema,
            schema_digest,
        }
    }

    /// The `:Module` mint — empty schema, `SENTINEL` id, path `"Module"`, digest ==
    /// [`empty_schema_digest`] (the schema's own `sig_id` is `None`, matching a self-sig).
    pub fn empty() -> Self {
        SigContent::new(
            "Module".to_string(),
            ScopeId::SENTINEL,
            SigSchema {
                sig_id: None,
                abstract_members: HashMap::new(),
                manifest_members: HashMap::new(),
                value_slots: HashMap::new(),
            },
        )
    }

    /// Content-keyed lattice-top test — true for [`Self::empty`] and, by content, for any other
    /// signature whose schema has no members: an empty interface is an empty interface
    /// regardless of how it was minted. Keyed off the schema digest so it tracks the identity
    /// relation, letting the specificity walk place the top by content, not by mint.
    pub fn is_empty_interface(&self) -> bool {
        self.schema_digest == empty_schema_digest()
    }
}

/// `Some(parameter names)` iff `kt` is a type constructor — a declared family (a
/// `TypeConstructor`-kind `SetRef`, whose names come off the projected schema) or a SIG's abstract
/// higher-kinded member (an [`KType::AbstractType`] carrying them directly). `None` for a
/// first-order type. Arity is the returned list's length.
pub fn constructor_param_names(kt: &KType) -> Option<Vec<String>> {
    match kt {
        KType::AbstractType { param_names, .. } if !param_names.is_empty() => {
            Some(param_names.clone())
        }
        KType::SetRef { set, index } if set.member(*index).kind == KKind::TypeConstructor => {
            match RecursiveSet::projected_schema(set, *index) {
                ProjectedSchema::TypeConstructor { param_names, .. } => Some(param_names),
                ProjectedSchema::NewType(_) => None,
            }
        }
        _ => None,
    }
}

/// Order-blind comparison of two constructor parameter lists: identity is the name set, and
/// declaration order is presentation.
fn name_sets_equal(left: &[String], right: &[String]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut left: Vec<&str> = left.iter().map(String::as_str).collect();
    let mut right: Vec<&str> = right.iter().map(String::as_str).collect();
    left.sort_unstable();
    right.sort_unstable();
    left == right
}

/// Rewrite `kt`, replacing references to `sig_id`'s abstract members with the caller's bindings
/// for them. Returns a plain value used only for comparison — never region-allocated.
///
/// One reference shape substitutes: an `AbstractType { source: sig_id, name }` of either order —
/// a first-order slot type, or a higher-kinded member in the ctor position of a
/// `ConstructorApply`. Compound types recurse; every other variant is a clone.
pub fn substitute_sig_members(
    kt: &KType,
    sig_id: ScopeId,
    members: &HashMap<String, KType>,
) -> KType {
    match kt {
        KType::AbstractType { source, name, .. } if *source == sig_id => {
            members.get(name).cloned().unwrap_or_else(|| kt.clone())
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
            args.map(|a| substitute_sig_members(a, sig_id, members)),
        ),
        _ => kt.clone(),
    }
}

/// Why a [`sig_subtype`] check failed — one of the five per-member rules, carrying the offending
/// member name and the *rendered* types that disagreed. Rendering to `String` at the failure
/// site (the only thing [`Self::render_fragment`] ever does with them) keeps this type free of
/// any `KType` reference, so it travels as plain diagnostic data.
pub enum SigSubtypeFailure {
    MissingTypeMember {
        name: String,
    },
    ManifestMismatch {
        name: String,
        got: String,
        expected: String,
    },
    /// A type member's kind or parameter names disagreed. `expected_params` is `Some(names)` when
    /// the super signature declares a constructor over those parameters, `None` when it declares
    /// a first-order proper type; `got` is the rendered sub binding that failed to match.
    KindMismatch {
        name: String,
        expected_params: Option<Vec<String>>,
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
                expected_params: Some(params),
                got,
            } => {
                let mut sorted: Vec<&str> = params.iter().map(String::as_str).collect();
                sorted.sort_unstable();
                format!(
                    "type member `{name}` must be a type constructor with parameters {{{}}}, got `{got}`",
                    sorted.join(", ")
                )
            }
            SigSubtypeFailure::KindMismatch {
                name,
                expected_params: None,
                got,
            } => format!(
                "type member `{name}` must be a proper type, got the type constructor `{got}`"
            ),
            SigSubtypeFailure::MissingValueSlot { name } => format!("missing member `{name}`"),
            SigSubtypeFailure::ValueSlotMismatch {
                name,
                got,
                expected,
            } => {
                format!("member `{name}` has type `{got}` but the signature declares `{expected}`")
            }
        }
    }
}

/// The canonical signature-subtyping relation: `sub <: sup`. Ok iff `sub` supplies every member
/// `sup` names (width — members `sup` does not name are ignored), with each manifest member
/// equal, each abstract member present at the matching kind and parameter names, and each value slot
/// covariantly compatible after substituting `sup`'s abstract members with `sub`'s bindings.
///
/// The failure is boxed: `SigSubtypeFailure` carries `KType`s and is large relative to the
/// common `Ok` path.
pub fn sig_subtype(
    sub: &SigSchema,
    sup: &SigSchema,
    types: &TypeRegistry,
) -> Result<(), Box<SigSubtypeFailure>> {
    // 1. Abstract members: present at the matching kind, and — for a constructor — over the same
    // parameter-name *set*. Parameter names are interface: a family declaring `{Item}` does not
    // supply a slot declared over `{Elem}`. The sub binding may be manifest or abstract.
    for (name, sup_repr) in &sup.abstract_members {
        let sub_binding = sub
            .manifest_members
            .get(name)
            .or_else(|| sub.abstract_members.get(name));
        let Some(sub_binding) = sub_binding else {
            return Err(Box::new(SigSubtypeFailure::MissingTypeMember {
                name: name.clone(),
            }));
        };
        let sup_params = constructor_param_names(sup_repr);
        let sub_params = constructor_param_names(sub_binding);
        let agrees = match (&sup_params, &sub_params) {
            (None, None) => true,
            (Some(expected), Some(got)) => name_sets_equal(expected, got),
            _ => false,
        };
        if !agrees {
            return Err(Box::new(SigSubtypeFailure::KindMismatch {
                name: name.clone(),
                expected_params: sup_params,
                got: sub_binding.render(),
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
                if let Some(repr) = sub.abstract_members.get(name) {
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
    // referencing one of `sup`'s abstract members reads through `sub`'s binding for it.
    // `slot_satisfied_by` computes the same verdict as
    // `substitute_sig_members(declared, id, sub_member_map).satisfied_by(sub_type)` by comparing
    // structurally and swapping in `sub`'s binding on reaching a self-abstract reference, so no
    // substituted type is ever built.
    let mut sub_member_map: HashMap<String, KType> = HashMap::new();
    for (name, kt) in &sub.manifest_members {
        sub_member_map.insert(name.clone(), kt.clone());
    }
    for (name, repr) in &sub.abstract_members {
        sub_member_map.insert(name.clone(), repr.clone());
    }
    for (name, declared) in &sup.value_slots {
        let Some(sub_type) = sub.value_slots.get(name) else {
            return Err(Box::new(SigSubtypeFailure::MissingValueSlot {
                name: name.clone(),
            }));
        };
        let ok = match sup.sig_id {
            Some(id) => slot_satisfied_by(declared, sub_type, &sub_member_map, id, types),
            // No `sig_id`: nothing to substitute, so the heterogeneous `satisfied_by` is exact.
            None => declared.satisfied_by(sub_type, types),
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
/// [`substitute_sig_members`] would rewrite (an `AbstractType` sourced at `sig_id` whose name
/// `members` binds). When false, substitution is the identity and a plain compare on `declared`
/// is exact.
fn references_sig_member(
    declared: &KType,
    sig_id: ScopeId,
    members: &HashMap<String, KType>,
) -> bool {
    match declared {
        KType::AbstractType { source, name, .. } => *source == sig_id && members.contains_key(name),
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
                    .values()
                    .any(|a| references_sig_member(a, sig_id, members))
        }
        _ => false,
    }
}

/// The `sub`-side binding a substitution point in `declared` resolves to, if any — the type
/// `substitute_sig_members` would splice in for this node.
fn substitution_binding<'m>(
    declared: &KType,
    sig_id: ScopeId,
    members: &'m HashMap<String, KType>,
) -> Option<&'m KType> {
    match declared {
        KType::AbstractType { source, name, .. } if *source == sig_id => members.get(name),
        _ => None,
    }
}

/// Verdict of `substitute_sig_members(declared, sig_id, members).satisfied_by(sub_type)` — does the
/// `sub` value slot fill the substituted `sup` slot? — computed without materializing the
/// substituted type. On reaching a self-abstract reference the walk switches to a direct
/// compare against `sub`'s binding; on a member-free node it falls to plain
/// `satisfied_by`; otherwise it descends the shared container structure with the same covariance
/// [`KType::satisfied_by`] applies (`Dict`/`Record`/`KFunction` component rules included).
fn slot_satisfied_by(
    declared: &KType,
    sub_type: &KType,
    members: &HashMap<String, KType>,
    sig_id: ScopeId,
    types: &TypeRegistry,
) -> bool {
    if let Some(binding) = substitution_binding(declared, sig_id, members) {
        return binding.satisfied_by(sub_type, types);
    }
    if !references_sig_member(declared, sig_id, members) {
        return declared.satisfied_by(sub_type, types);
    }
    match (declared, sub_type) {
        (KType::List { element: ed, .. }, KType::List { element: es, .. }) => {
            slot_satisfied_by(ed, es, members, sig_id, types)
        }
        (
            KType::Dict {
                key: kd, value: vd, ..
            },
            KType::Dict {
                key: ks, value: vs, ..
            },
        ) => {
            slot_satisfied_by(kd, ks, members, sig_id, types)
                && slot_satisfied_by(vd, vs, members, sig_id, types)
        }
        (KType::Record { fields: fd, .. }, KType::Record { fields: fs, .. }) => {
            // Record-value covariance: every slot field present in the value, covariantly.
            fd.iter().all(|(name, dt)| {
                fs.get(name)
                    .is_some_and(|st| slot_satisfied_by(dt, st, members, sig_id, types))
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
                && ad.iter().all(|(name, d)| {
                    as_.get(name)
                        .is_some_and(|s| slot_satisfied_by(d, s, members, sig_id, types))
                })
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
                    pd.get(name).is_some_and(|dp| {
                        slot_more_specific_or_equal(dp, sp, members, sig_id, types)
                    })
                })
                && slot_satisfied_by(rd, rs, members, sig_id, types)
        }
        (KType::Union { members: ud, .. }, _) => {
            // A value satisfies a substituted union slot iff it (each of its members, if it is
            // itself a union) refines some slot member — the union-membership rule of `satisfied_by`.
            let ys: Vec<&KType> = match sub_type {
                KType::Union { members: us, .. } => us.iter().collect(),
                other => vec![other],
            };
            ys.iter().all(|y| {
                ud.iter()
                    .any(|md| slot_satisfied_by(md, y, members, sig_id, types))
            })
        }
        _ => false,
    }
}

/// Verdict of `substitute_sig_members(declared, ...) == target
/// || substitute_sig_members(declared, ...).is_more_specific_than(target)` — the contravariant
/// direction [`slot_satisfied_by`] needs for a function parameter, computed without building the
/// substituted type.
fn slot_more_specific_or_equal(
    declared: &KType,
    target: &KType,
    members: &HashMap<String, KType>,
    sig_id: ScopeId,
    types: &TypeRegistry,
) -> bool {
    if let Some(binding) = substitution_binding(declared, sig_id, members) {
        return binding == target || binding.is_more_specific_than(target, types);
    }
    if !references_sig_member(declared, sig_id, members) {
        return declared == target || declared.is_more_specific_than(target, types);
    }
    // The substituted slot outranks `Any` / an unconstrained name, and refines a union it has a
    // member in — the top guards of `more_specific_walk`, mirrored here.
    match target {
        KType::Any => return true,
        KType::Identifier | KType::OfKind(KKind::ProperType) => return true,
        KType::Union { members: ts, .. } => {
            return ts
                .iter()
                .any(|t| slot_more_specific_or_equal(declared, t, members, sig_id, types))
        }
        _ => {}
    }
    match (declared, target) {
        (KType::List { element: ed, .. }, KType::List { element: et, .. }) => {
            slot_more_specific_or_equal(ed, et, members, sig_id, types)
        }
        (
            KType::Dict {
                key: kd, value: vd, ..
            },
            KType::Dict {
                key: kt, value: vt, ..
            },
        ) => {
            slot_more_specific_or_equal(kd, kt, members, sig_id, types)
                && slot_more_specific_or_equal(vd, vt, members, sig_id, types)
        }
        (KType::Record { fields: fd, .. }, KType::Record { fields: ft, .. }) => {
            // Record-value covariance with width-superset: the more-specific record has every
            // field of `target`, each covariantly refined.
            ft.keys().all(|k| fd.get(k).is_some())
                && ft.iter().all(|(name, tt)| {
                    fd.get(name).is_some_and(|dt| {
                        slot_more_specific_or_equal(dt, tt, members, sig_id, types)
                    })
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
                && ad.iter().all(|(name, d)| {
                    at.get(name)
                        .is_some_and(|t| slot_more_specific_or_equal(d, t, members, sig_id, types))
                })
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
                        .is_some_and(|tp| slot_satisfied_by(dp, tp, members, sig_id, types))
                })
                && slot_more_specific_or_equal(rd, rt, members, sig_id, types)
        }
        _ => false,
    }
}

/// Verdict of `substitute_sig_members(declared, ...) == other` — structural equality with `sub`'s
/// bindings spliced in. Only the constructor identity of a `ConstructorApply` needs this (a
/// constructor is a leaf `SetRef`, so the recursion bottoms out immediately in practice).
fn slot_types_equal(
    declared: &KType,
    other: &KType,
    members: &HashMap<String, KType>,
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
                && ad.iter().all(|(name, d)| {
                    ao.get(name)
                        .is_some_and(|o| slot_types_equal(d, o, members, sig_id))
                })
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
/// concrete witness, which is exactly a [`KType::AbstractType`] — the first-order `TYPE Elt` slot
/// and the higher-kinded `TYPE (Elem AS Wrap)` slot alike, both sourced at the SIG decl scope.
/// Everything else — a manifest `LET Tag = Number` binding a concrete type, a minted constructor
/// family — is manifest.
pub(crate) fn is_abstract_sig_member(kt: &KType) -> bool {
    matches!(kt, KType::AbstractType { .. })
}

#[cfg(test)]
mod tests;
