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
//! A `SigSchema` is what the `Signature` [`TypeNode`] owns; the node computes and stores the
//! schema's content digest once at intern time, so the schema itself carries no digest field.
//!
//! See [design/typing/modules.md](../../../../design/typing/modules.md).

use std::collections::HashMap;

use crate::machine::core::{Scope, ScopeId};

use super::kkind::KKind;
use super::ktype::KType;
use super::node::{NodeSchema, TypeNode};
use super::registry::TypeRegistry;
use crate::machine::model::values::Module;

/// Normalized signature schema — the carrier the subtyping relation is defined over.
///
/// Members are split by *representation*, not by surface syntax: an abstract member carries no
/// concrete witness (an `AbstractType` node, of either order), a manifest member fixes a
/// concrete type. A module self-sig never has abstract members — `TYPE` is a SIG-body-only
/// construct.
#[derive(Clone)]
pub struct SigSchema {
    /// The binder this schema's own abstract members are sourced at: `Some(ScopeId::SENTINEL)`
    /// for a SIG declaration, `None` for a module self-sig (whose slot types name no
    /// SIG-declared refs).
    ///
    /// The binder is *canonical*, not the declaring scope's id: [`Self::project_decl`] rewrites
    /// every SIG-own member's `source` to [`ScopeId::SENTINEL`] as it projects, so two textually
    /// identical `SIG` declarations project to one schema and intern to one type. `SENTINEL` is
    /// never a minted scope id, so a canonical binder cannot alias a real one, and the
    /// substitution and comparison walks below keep testing `source == sig_id` unchanged.
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
    /// The member-free schema — the module-lattice top the `:Module` name lowers to, and the
    /// content any zero-member `SIG E = ()` declaration projects to. `sig_id` is `None`: an empty
    /// interface names no abstract member for a slot type to substitute against.
    pub fn empty() -> SigSchema {
        SigSchema {
            sig_id: None,
            abstract_members: HashMap::new(),
            manifest_members: HashMap::new(),
            value_slots: HashMap::new(),
        }
    }

    /// Project a SIG decl scope into its schema, at SIG finish. Every type-table entry is a
    /// genuine type member (the token-class partition holds — value slots live in the scope's
    /// slot collector, not in `types`), classified abstract/manifest by representation; the
    /// value slots come from the scope's own slot collector. The only place this
    /// classification runs — once per SIG.
    pub(crate) fn project_decl(decl_scope: &Scope<'_>, types: &TypeRegistry) -> SigSchema {
        let declared = decl_scope.id;
        let mut abstract_members = HashMap::new();
        let mut manifest_members = HashMap::new();
        for (name, kt) in decl_scope.bindings().iter_types() {
            let canonical = canonicalize_binder(*kt, declared, types);
            if is_abstract_sig_member(canonical, types) {
                abstract_members.insert(name, canonical);
            } else {
                manifest_members.insert(name, canonical);
            }
        }
        let mut value_slots = HashMap::new();
        for (name, kt) in decl_scope.sig_value_slots() {
            value_slots.insert(name, canonicalize_binder(*kt, declared, types));
        }
        SigSchema {
            sig_id: Some(ScopeId::SENTINEL),
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
            schema.manifest_members.insert(name.clone(), *kt);
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
            manifest_members.insert(name, *kt);
        }
        for (name, kt) in module.type_members.borrow().iter() {
            manifest_members.insert(name.clone(), *kt);
        }
        let mut value_slots: HashMap<String, KType> = HashMap::new();
        for (name, obj) in child.bindings().iter_data() {
            value_slots.insert(name, obj.ktype());
        }
        for (name, tag) in module.slot_type_tags.borrow().iter() {
            value_slots.insert(name.clone(), *tag);
        }
        SigSchema {
            sig_id: None,
            abstract_members: HashMap::new(),
            manifest_members,
            value_slots,
        }
    }
}

/// `Some(parameter names)` iff `kt` is a type constructor — a declared family (a
/// `TypeConstructor`-kind member, whose names ride its sealed schema) or a SIG's abstract
/// higher-kinded member (an `AbstractType` node carrying them directly). `None` for a
/// first-order type. Arity is the returned list's length.
pub fn constructor_param_names(kt: KType, types: &TypeRegistry) -> Option<Vec<String>> {
    match types.node(kt) {
        TypeNode::AbstractType { param_names, .. } if !param_names.is_empty() => Some(param_names),
        TypeNode::SetMember {
            kind: KKind::TypeConstructor,
            schema: NodeSchema::TypeConstructor { param_names, .. },
            ..
        } => Some(param_names),
        _ => None,
    }
}

/// The diagnostic for a bare type constructor standing in a value type position, or `None` when
/// `kt` is well-kinded there.
///
/// A value's type must be a proper type — kind `*`. The ill-kinded shapes are exactly the two
/// [`constructor_param_names`] names: a declared family at `TypeConstructor` kind and
/// a SIG's higher-kinded abstract member, each of kind `* -> *` and standing with none of its
/// parameters supplied. A saturated application (`ConstructorApply`), a first-order abstract
/// member, and every ground type are proper, so they yield `None`. A *type* position — the head
/// of an application, a `TYPE (Elem AS Wrap)` declaration, a module's type-constructor member —
/// takes a bare constructor legitimately and never consults this.
///
/// `position` is a noun phrase naming the *type slot* the constructor stands in — "the type of FN
/// parameter `x`", "the FN return type", "the element type of `LIST OF`" — so it reads as the
/// subject of "must be a proper type". It names the type, never the value or field whose type it
/// is: "the type of SIG value slot `boxed`", not "SIG value slot `boxed`", since a slot is not
/// itself a type. The constructor's parameter names follow, since supplying them is the fix.
pub fn unsaturated_constructor_message(
    kt: KType,
    position: &str,
    types: &TypeRegistry,
) -> Option<String> {
    let param_names = constructor_param_names(kt, types)?;
    let name = kt.name(types);
    let plural = if param_names.len() == 1 { "" } else { "s" };
    let listed = param_names
        .iter()
        .map(|p| format!("`{p}`"))
        .collect::<Vec<_>>()
        .join(", ");
    let applied = param_names
        .iter()
        .map(|p| format!("{p} = <Type>"))
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!(
        "`{name}` is a type constructor taking {arity} type parameter{plural} ({listed}), but \
         {position} must be a proper type — apply it, as `:({name} {{{applied}}})`",
        arity = param_names.len(),
    ))
}

/// Order-blind comparison of two constructor parameter lists: identity is the name set, and
/// declaration order is presentation.
pub(super) fn name_sets_equal(left: &[String], right: &[String]) -> bool {
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
/// for them. Returns an interned handle used only for comparison.
///
/// One reference shape substitutes: a nonce-free `AbstractType { source: sig_id, name }` of either
/// order — a first-order slot type, or a higher-kinded member in the constructor position of a
/// `ConstructorApply`. Compound types recurse; every other shape is returned unchanged. A nonced
/// `AbstractType` is an opaque ascription's generative mint, not a reference to a declaration, so
/// it never substitutes even when it shares its binder's `source` and name.
pub fn substitute_sig_members(
    kt: KType,
    sig_id: ScopeId,
    members: &HashMap<String, KType>,
    types: &TypeRegistry,
) -> KType {
    match types.node(kt) {
        TypeNode::AbstractType {
            source,
            ref name,
            nonce: None,
            ..
        } if source == sig_id => members.get(name).copied().unwrap_or(kt),
        TypeNode::List { element } => {
            let element = substitute_sig_members(element, sig_id, members, types);
            types.list(element)
        }
        TypeNode::Dict { key, value } => {
            let key = substitute_sig_members(key, sig_id, members, types);
            let value = substitute_sig_members(value, sig_id, members, types);
            types.dict(key, value)
        }
        TypeNode::Record { fields } => {
            let fields = fields.map(|v| substitute_sig_members(*v, sig_id, members, types));
            types.record(fields)
        }
        TypeNode::KFunction { params, ret } => {
            let params = params.map(|v| substitute_sig_members(*v, sig_id, members, types));
            let ret = substitute_sig_members(ret, sig_id, members, types);
            types.function_type(params, ret)
        }
        TypeNode::Union { members: us } => types.union_of(
            us.into_iter()
                .map(|m| substitute_sig_members(m, sig_id, members, types))
                .collect(),
        ),
        TypeNode::ConstructorApply {
            constructor,
            arguments,
        } => {
            let constructor = substitute_sig_members(constructor, sig_id, members, types);
            let arguments = arguments.map(|a| substitute_sig_members(*a, sig_id, members, types));
            types.constructor_apply(constructor, arguments)
        }
        _ => kt,
    }
}

/// Rewrite every reference to `declared`'s own abstract members so it is sourced at
/// [`ScopeId::SENTINEL`] instead — the canonical binder every projected SIG schema shares.
///
/// Structurally this is [`substitute_sig_members`] with the substitution being a re-source rather
/// than a lookup: the same shapes recurse, and only a nonce-free `AbstractType` at `declared`
/// changes. Running it at projection is what makes two textually identical declarations one
/// interned type, since after it nothing in the schema records which scope declared it.
fn canonicalize_binder(kt: KType, declared: ScopeId, types: &TypeRegistry) -> KType {
    match types.node(kt) {
        TypeNode::AbstractType {
            source,
            name,
            param_names,
            nonce: None,
        } if source == declared => types.intern(TypeNode::AbstractType {
            source: ScopeId::SENTINEL,
            name,
            param_names,
            nonce: None,
        }),
        TypeNode::List { element } => {
            let element = canonicalize_binder(element, declared, types);
            types.list(element)
        }
        TypeNode::Dict { key, value } => {
            let key = canonicalize_binder(key, declared, types);
            let value = canonicalize_binder(value, declared, types);
            types.dict(key, value)
        }
        TypeNode::Record { fields } => {
            let fields = fields.map(|v| canonicalize_binder(*v, declared, types));
            types.record(fields)
        }
        TypeNode::KFunction { params, ret } => {
            let params = params.map(|v| canonicalize_binder(*v, declared, types));
            let ret = canonicalize_binder(ret, declared, types);
            types.function_type(params, ret)
        }
        TypeNode::Union { members } => types.union_of(
            members
                .into_iter()
                .map(|m| canonicalize_binder(m, declared, types))
                .collect(),
        ),
        TypeNode::ConstructorApply {
            constructor,
            arguments,
        } => {
            let constructor = canonicalize_binder(constructor, declared, types);
            let arguments = arguments.map(|a| canonicalize_binder(*a, declared, types));
            types.constructor_apply(constructor, arguments)
        }
        _ => kt,
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
/// The failure is boxed: `SigSubtypeFailure` carries rendered member names and types, and is
/// large relative to the common `Ok` path.
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
        let sup_params = constructor_param_names(*sup_repr, types);
        let sub_params = constructor_param_names(*sub_binding, types);
        let agrees = match (&sup_params, &sub_params) {
            (None, None) => true,
            (Some(expected), Some(got)) => name_sets_equal(expected, got),
            _ => false,
        };
        if !agrees {
            return Err(Box::new(SigSubtypeFailure::KindMismatch {
                name: name.clone(),
                expected_params: sup_params,
                got: sub_binding.render(types),
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
                    got: got.render(types),
                    expected: fixed.render(types),
                }))
            }
            None => {
                // An abstract `sub` member supplies no witness for a manifest requirement.
                if let Some(repr) = sub.abstract_members.get(name) {
                    return Err(Box::new(SigSubtypeFailure::ManifestMismatch {
                        name: name.clone(),
                        got: repr.render(types),
                        expected: fixed.render(types),
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
        sub_member_map.insert(name.clone(), *kt);
    }
    for (name, repr) in &sub.abstract_members {
        sub_member_map.insert(name.clone(), *repr);
    }
    for (name, declared) in &sup.value_slots {
        let Some(sub_type) = sub.value_slots.get(name) else {
            return Err(Box::new(SigSubtypeFailure::MissingValueSlot {
                name: name.clone(),
            }));
        };
        let ok = match sup.sig_id {
            Some(id) => slot_satisfied_by(*declared, *sub_type, &sub_member_map, id, types),
            // No `sig_id`: nothing to substitute, so the heterogeneous `satisfied_by` is exact.
            None => declared.satisfied_by(*sub_type, types),
        };
        if !ok {
            return Err(Box::new(SigSubtypeFailure::ValueSlotMismatch {
                name: name.clone(),
                got: sub_type.render(types),
                expected: declared.render(types),
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
    declared: KType,
    sig_id: ScopeId,
    members: &HashMap<String, KType>,
    types: &TypeRegistry,
) -> bool {
    match types.node(declared) {
        TypeNode::AbstractType {
            source,
            name,
            nonce: None,
            ..
        } => source == sig_id && members.contains_key(&name),
        TypeNode::List { element } => references_sig_member(element, sig_id, members, types),
        TypeNode::Dict { key, value } => {
            references_sig_member(key, sig_id, members, types)
                || references_sig_member(value, sig_id, members, types)
        }
        TypeNode::Record { fields } => fields
            .values()
            .any(|v| references_sig_member(*v, sig_id, members, types)),
        TypeNode::KFunction { params, ret } => {
            params
                .values()
                .any(|v| references_sig_member(*v, sig_id, members, types))
                || references_sig_member(ret, sig_id, members, types)
        }
        TypeNode::Union { members: us } => us
            .iter()
            .any(|m| references_sig_member(*m, sig_id, members, types)),
        TypeNode::ConstructorApply {
            constructor,
            arguments,
        } => {
            references_sig_member(constructor, sig_id, members, types)
                || arguments
                    .values()
                    .any(|a| references_sig_member(*a, sig_id, members, types))
        }
        _ => false,
    }
}

/// The `sub`-side binding a substitution point in `declared` resolves to, if any — the type
/// `substitute_sig_members` would splice in for this node.
fn substitution_binding(
    declared: KType,
    sig_id: ScopeId,
    members: &HashMap<String, KType>,
    types: &TypeRegistry,
) -> Option<KType> {
    match types.node(declared) {
        TypeNode::AbstractType {
            source,
            name,
            nonce: None,
            ..
        } if source == sig_id => members.get(&name).copied(),
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
    declared: KType,
    sub_type: KType,
    members: &HashMap<String, KType>,
    sig_id: ScopeId,
    types: &TypeRegistry,
) -> bool {
    if let Some(binding) = substitution_binding(declared, sig_id, members, types) {
        return binding.satisfied_by(sub_type, types);
    }
    if !references_sig_member(declared, sig_id, members, types) {
        return declared.satisfied_by(sub_type, types);
    }
    match (types.node(declared), types.node(sub_type)) {
        (TypeNode::List { element: ed }, TypeNode::List { element: es }) => {
            slot_satisfied_by(ed, es, members, sig_id, types)
        }
        (TypeNode::Dict { key: kd, value: vd }, TypeNode::Dict { key: ks, value: vs }) => {
            slot_satisfied_by(kd, ks, members, sig_id, types)
                && slot_satisfied_by(vd, vs, members, sig_id, types)
        }
        (TypeNode::Record { fields: fd }, TypeNode::Record { fields: fs }) => {
            // Record-value covariance: every slot field present in the value, covariantly.
            fd.iter().all(|(name, dt)| {
                fs.get(name)
                    .is_some_and(|st| slot_satisfied_by(*dt, *st, members, sig_id, types))
            })
        }
        (
            TypeNode::ConstructorApply {
                constructor: cd,
                arguments: ad,
            },
            TypeNode::ConstructorApply {
                constructor: cs,
                arguments: as_,
            },
        ) => {
            ad.len() == as_.len()
                && slot_types_equal(cd, cs, members, sig_id, types)
                && ad.iter().all(|(name, d)| {
                    as_.get(name)
                        .is_some_and(|s| slot_satisfied_by(*d, *s, members, sig_id, types))
                })
        }
        (
            TypeNode::KFunction {
                params: pd,
                ret: rd,
            },
            TypeNode::KFunction {
                params: ps,
                ret: rs,
            },
        ) => {
            // Contravariant params (width-drop): every value param names a slot param the
            // substituted slot fixes equal-or-more-specific. Covariant return.
            ps.keys().all(|k| pd.get(k).is_some())
                && ps.iter().all(|(name, sp)| {
                    pd.get(name).is_some_and(|dp| {
                        slot_more_specific_or_equal(*dp, *sp, members, sig_id, types)
                    })
                })
                && slot_satisfied_by(rd, rs, members, sig_id, types)
        }
        (TypeNode::Union { members: ud }, sub_node) => {
            // A value satisfies a substituted union slot iff it (each of its members, if it is
            // itself a union) refines some slot member — the union-membership rule of
            // `satisfied_by`.
            let ys: Vec<KType> = match sub_node {
                TypeNode::Union { members: us } => us,
                _ => vec![sub_type],
            };
            ys.iter().all(|y| {
                ud.iter()
                    .any(|md| slot_satisfied_by(*md, *y, members, sig_id, types))
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
    declared: KType,
    target: KType,
    members: &HashMap<String, KType>,
    sig_id: ScopeId,
    types: &TypeRegistry,
) -> bool {
    if let Some(binding) = substitution_binding(declared, sig_id, members, types) {
        return binding == target || binding.is_more_specific_than(target, types);
    }
    if !references_sig_member(declared, sig_id, members, types) {
        return declared == target || declared.is_more_specific_than(target, types);
    }
    // The substituted slot outranks `Any` / an unconstrained name, and refines a union it has a
    // member in — the top guards of `more_specific_walk`, mirrored here.
    if target == KType::ANY
        || target == KType::IDENTIFIER
        || target == KType::of_kind(KKind::ProperType)
    {
        return true;
    }
    let target_node = types.node(target);
    if let TypeNode::Union { members: ts } = &target_node {
        return ts
            .iter()
            .any(|t| slot_more_specific_or_equal(declared, *t, members, sig_id, types));
    }
    match (types.node(declared), target_node) {
        (TypeNode::List { element: ed }, TypeNode::List { element: et }) => {
            slot_more_specific_or_equal(ed, et, members, sig_id, types)
        }
        (TypeNode::Dict { key: kd, value: vd }, TypeNode::Dict { key: kt, value: vt }) => {
            slot_more_specific_or_equal(kd, kt, members, sig_id, types)
                && slot_more_specific_or_equal(vd, vt, members, sig_id, types)
        }
        (TypeNode::Record { fields: fd }, TypeNode::Record { fields: ft }) => {
            // Record-value covariance with width-superset: the more-specific record has every
            // field of `target`, each covariantly refined.
            ft.keys().all(|k| fd.get(k).is_some())
                && ft.iter().all(|(name, tt)| {
                    fd.get(name).is_some_and(|dt| {
                        slot_more_specific_or_equal(*dt, *tt, members, sig_id, types)
                    })
                })
        }
        (
            TypeNode::ConstructorApply {
                constructor: cd,
                arguments: ad,
            },
            TypeNode::ConstructorApply {
                constructor: ct,
                arguments: at,
            },
        ) => {
            ad.len() == at.len()
                && slot_types_equal(cd, ct, members, sig_id, types)
                && ad.iter().all(|(name, d)| {
                    at.get(name).is_some_and(|t| {
                        slot_more_specific_or_equal(*d, *t, members, sig_id, types)
                    })
                })
        }
        (
            TypeNode::KFunction {
                params: pd,
                ret: rd,
            },
            TypeNode::KFunction {
                params: pt,
                ret: rt,
            },
        ) => {
            // Contravariant params, covariant return — the dual of the `slot_satisfied_by` case.
            pd.keys().all(|k| pt.get(k).is_some())
                && pd.iter().all(|(name, dp)| {
                    pt.get(name)
                        .is_some_and(|tp| slot_satisfied_by(*dp, *tp, members, sig_id, types))
                })
                && slot_more_specific_or_equal(rd, rt, members, sig_id, types)
        }
        _ => false,
    }
}

/// Verdict of `substitute_sig_members(declared, ...) == other` — structural equality with `sub`'s
/// bindings spliced in. Only the constructor identity of a `ConstructorApply` needs this (a
/// constructor is a leaf member reference, so the recursion bottoms out immediately in practice).
fn slot_types_equal(
    declared: KType,
    other: KType,
    members: &HashMap<String, KType>,
    sig_id: ScopeId,
    types: &TypeRegistry,
) -> bool {
    if let Some(binding) = substitution_binding(declared, sig_id, members, types) {
        return binding == other;
    }
    if !references_sig_member(declared, sig_id, members, types) {
        return declared == other;
    }
    match (types.node(declared), types.node(other)) {
        (TypeNode::List { element: ed }, TypeNode::List { element: eo }) => {
            slot_types_equal(ed, eo, members, sig_id, types)
        }
        (TypeNode::Dict { key: kd, value: vd }, TypeNode::Dict { key: ko, value: vo }) => {
            slot_types_equal(kd, ko, members, sig_id, types)
                && slot_types_equal(vd, vo, members, sig_id, types)
        }
        (TypeNode::Record { fields: fd }, TypeNode::Record { fields: fo }) => {
            fd.len() == fo.len()
                && fd.iter().all(|(name, dt)| {
                    fo.get(name)
                        .is_some_and(|ot| slot_types_equal(*dt, *ot, members, sig_id, types))
                })
        }
        (
            TypeNode::ConstructorApply {
                constructor: cd,
                arguments: ad,
            },
            TypeNode::ConstructorApply {
                constructor: co,
                arguments: ao,
            },
        ) => {
            ad.len() == ao.len()
                && slot_types_equal(cd, co, members, sig_id, types)
                && ad.iter().all(|(name, d)| {
                    ao.get(name)
                        .is_some_and(|o| slot_types_equal(*d, *o, members, sig_id, types))
                })
        }
        (
            TypeNode::KFunction {
                params: pd,
                ret: rd,
            },
            TypeNode::KFunction {
                params: po,
                ret: ro,
            },
        ) => {
            pd.len() == po.len()
                && pd.iter().all(|(name, dt)| {
                    po.get(name)
                        .is_some_and(|ot| slot_types_equal(*dt, *ot, members, sig_id, types))
                })
                && slot_types_equal(rd, ro, members, sig_id, types)
        }
        (TypeNode::Union { members: ud }, TypeNode::Union { members: uo }) => {
            ud.len() == uo.len()
                && ud.iter().all(|dm| {
                    uo.iter()
                        .any(|om| slot_types_equal(*dm, *om, members, sig_id, types))
                })
        }
        _ => false,
    }
}

/// Classify a SIG type-table entry by its *representation*: an abstract member carries no
/// concrete witness, which is exactly an `AbstractType` node — the first-order `TYPE Elt` slot
/// and the higher-kinded `TYPE (Elem AS Wrap)` slot alike, both sourced at the SIG decl scope.
/// Everything else — a manifest `LET Tag = Number` binding a concrete type, a minted constructor
/// family — is manifest.
pub(crate) fn is_abstract_sig_member(kt: KType, types: &TypeRegistry) -> bool {
    matches!(types.node(kt), TypeNode::AbstractType { .. })
}

#[cfg(test)]
mod tests;
