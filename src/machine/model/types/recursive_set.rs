//! `RecursiveSet` — the atomic unit of allocation, identity, and lift for nominal types.
//!
//! A strongly-connected component of mutually-recursive types (a self-recursive type, an
//! `A ↔ B` pair, a longer cycle) is sealed into one [`RecursiveSet`]. A non-recursive type
//! is a set of one. The set is `Rc`-owned (never region-owned): lifting any reference to a
//! member is just `Rc::clone` of the set, so the whole group travels as a unit — no copy,
//! no visited-map cycle-walk, no `Rc<CallFrame>` anchor.
//!
//! References:
//! - *Intra-set* (a member's schema naming a sibling) is [`KType::SetLocal`] — a bare index
//!   resolved against the ambient set during deep traversal. Never an `Rc`, so the set holds
//!   no internal refcount cycle and frees normally once its last external handle drops.
//! - *External* (the `bindings.types` entry, a field of a non-member, a param slot, a
//!   constructed value's `ktype()`) is [`KType::SetRef`] carrying `Rc<RecursiveSet>` + index.
//!
//! Identity is `(set digest, index)` — the set's content digest, sealed at fill (see
//! [`set_digest`](super::type_digest::set_digest)), so two independently built sets with the
//! same content denote the same type. A member's `name` and `kind` join the digested content;
//! `scope_id` stays diagnostics-only, excluded from identity so the same declaration
//! elaborated twice unifies. The `Rc` remains solely as content transport — `Rc::clone` shares
//! the allocation on lift, and [`same_nominal`] keeps a pointer fast path for that shared case
//! and for the pre-seal window before a digest exists.
//!
//! A set is created with its membership known — a singleton for a non-recursive or
//! self-recursive type, or the whole group for a `RECURSIVE TYPES` block — with each
//! member's `kind` recorded eagerly and its `schema` filled at its own finalize, hence the
//! [`RefCell`] two-phase cell.

use std::cell::{OnceCell, Ref, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use crate::machine::core::ScopeId;

use super::kkind::KKind;
use super::ktype::KType;
use super::type_digest::{set_digest, TypeDigest};

/// A member's schema, owned by value inside the set. Sibling references inside these
/// `KType`s are [`KType::SetLocal`] indices into the enclosing [`RecursiveSet`].
#[derive(Debug)]
pub enum NominalSchema {
    /// Fresh nominal over a transparent representation; `repr` is not part of identity.
    NewType(Box<KType>),
    /// Higher-kinded constructor: erased-parameter variant schema plus parameter names.
    TypeConstructor {
        schema: HashMap<String, KType>,
        param_names: Vec<String>,
    },
}

/// One nominal type within a [`RecursiveSet`]. `kind` is known when the set is created;
/// `schema` is filled at the member's own finalize (two-phase), so it rides a [`RefCell`].
pub struct NominalMember {
    /// Diagnostics / rendering only — never identity.
    pub name: String,
    /// Origin scope, diagnostics only — never identity.
    pub scope_id: ScopeId,
    /// Always one of the three nominal families `Tagged` / `NewType` / `TypeConstructor`.
    pub kind: KKind,
    schema: RefCell<Option<NominalSchema>>,
}

impl NominalMember {
    /// A member whose schema is not yet filled — created before its declaration finalizes.
    pub fn pending(name: String, scope_id: ScopeId, kind: KKind) -> Self {
        Self {
            name,
            scope_id,
            kind,
            schema: RefCell::new(None),
        }
    }

    /// Install the member's schema. Private so no site can bypass
    /// [`RecursiveSet::fill_member`], which seals the set's digest once the last member
    /// fills. Idempotent on a re-fill with equal shape is the caller's concern; a double-fill
    /// is a sealing bug.
    fn fill(&self, schema: NominalSchema) {
        *self.schema.borrow_mut() = Some(schema);
    }

    /// Whether the schema has been filled (the member's finalize has run).
    pub fn is_filled(&self) -> bool {
        self.schema.borrow().is_some()
    }

    /// Borrow the filled schema for deep traversal. `None` until the member finalizes.
    pub fn schema(&self) -> Ref<'_, Option<NominalSchema>> {
        self.schema.borrow()
    }
}

/// A sealed strongly-connected component of nominal types — a singleton for a non-recursive
/// or self-recursive type, or the whole group declared in a `RECURSIVE TYPES` block. Created
/// with the membership fixed; members fill their schemas as they finalize.
pub struct RecursiveSet {
    members: Vec<NominalMember>,
    /// `name → index` for sealing a member's transient `RecursiveRef(name)` to
    /// `SetLocal(index)`.
    index_of: HashMap<String, usize>,
    /// The set's content digest, sealed by [`Self::fill_member`] once every member fills —
    /// `(set digest, index)` is a `SetRef`'s identity. Empty during the two-phase fill
    /// window, where identity falls back to the set pointer.
    digest: OnceCell<TypeDigest>,
    /// Set when opaque ascription mints this set, so its per-application nonce folds into the
    /// digest and two applications never unify. `None` for every content-addressed set.
    generative_nonce: Option<ScopeId>,
}

impl RecursiveSet {
    /// Seal a set over the given members (declaration order). The `index_of` map keys each
    /// member's name to its slot so schema sealing can resolve sibling references.
    pub fn new(members: Vec<NominalMember>) -> Self {
        let index_of = members
            .iter()
            .enumerate()
            .map(|(i, m)| (m.name.clone(), i))
            .collect();
        Self {
            members,
            index_of,
            digest: OnceCell::new(),
            generative_nonce: None,
        }
    }

    /// A generative set: opaque ascription's per-application mint. `nonce` (the minted
    /// module's `scope_id`) folds into the digest, so two `:|` applications of the same
    /// signature member over the same representation stay distinct types.
    pub fn new_generative(members: Vec<NominalMember>, nonce: ScopeId) -> Self {
        let mut set = Self::new(members);
        set.generative_nonce = Some(nonce);
        set
    }

    /// The set's sealed content digest, or `None` in the two-phase window before the last
    /// member fills.
    pub fn digest(&self) -> Option<TypeDigest> {
        self.digest.get().copied()
    }

    /// The generative nonce folded into this set's digest, if it is a generative mint.
    pub fn generative_nonce(&self) -> Option<ScopeId> {
        self.generative_nonce
    }

    /// Fill member `index`'s schema and, if that was the last unfilled member, seal the set's
    /// content digest. The single sealing seam — [`NominalMember::fill`] is private so no
    /// site can install a schema without reaching this digest computation.
    pub fn fill_member(&self, index: usize, schema: NominalSchema) {
        self.members[index].fill(schema);
        if self.digest.get().is_none() && self.members.iter().all(NominalMember::is_filled) {
            let _ = self.digest.set(set_digest(self));
        }
    }

    pub fn member(&self, index: usize) -> &NominalMember {
        &self.members[index]
    }

    pub fn members(&self) -> &[NominalMember] {
        &self.members
    }

    pub fn len(&self) -> usize {
        self.members.len()
    }

    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    /// Index of the member named `name`, if any — used when sealing a transient
    /// `RecursiveRef(name)` reference into `SetLocal(index)`.
    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.index_of.get(name).copied()
    }

    /// A singleton set whose one member carries `schema`. The common non-recursive case.
    pub fn singleton(name: String, scope_id: ScopeId, schema: NominalSchema) -> Rc<Self> {
        let member = NominalMember::pending(name, scope_id, schema.kind());
        let set = RecursiveSet::new(vec![member]);
        set.fill_member(0, schema);
        Rc::new(set)
    }
}

/// Whether two member references `(set, index)` denote the same nominal type — the identity
/// rule shared by [`KType`](super::ktype::KType)'s `SetRef` `PartialEq` arm and the
/// dispatch-time constructor-identity check in `ktype_predicates`.
///
/// The set-pointer fast path (`Rc::ptr_eq`) is the ONLY path that can answer "equal" while a
/// set is unsealed — the two-phase window before its digest exists — and it is also the cheap
/// common case (lift shares a set by `Rc::clone`). Once both sets are sealed, identity is the
/// content digest plus index, so two independently built sets with the same content unify. A
/// digestless set in a *different* allocation is never equal: a pre-seal `SetRef` never
/// escapes its declaring elaboration, so this can only fire on a mid-declaration
/// self-comparison, which the pointer path has already settled. There is no structural
/// fallback — the digest is the truth.
pub(crate) fn same_nominal(
    s1: &Rc<RecursiveSet>,
    i1: usize,
    s2: &Rc<RecursiveSet>,
    i2: usize,
) -> bool {
    // The shared-set fast path when a set was lifted by `Rc::clone`.
    if Rc::ptr_eq(s1, s2) {
        return i1 == i2;
    }
    match (s1.digest(), s2.digest()) {
        (Some(d1), Some(d2)) => d1 == d2 && i1 == i2,
        _ => false,
    }
}

/// Deep-walk `kt`, sealing every intra-set reference to a [`KType::SetLocal`] index against
/// the set being finalized:
///
/// - A transient [`KType::RecursiveRef`] (a self / forward-sibling reference that lowered to
///   a name during elaboration) whose name is a member of `set` → `SetLocal(index)`.
/// - A [`KType::SetRef`] that resolved *back into this same set* during elaboration (a
///   cross-sibling reference that hit the seal's pre-installed `SetRef` in `bindings.types`)
///   → `SetLocal(index)`. This is load-bearing: leaving an internal `SetRef` would hold an
///   `Rc` to the set's own allocation, a refcount cycle that leaks the whole group.
///
/// A `RecursiveRef` naming no member is a sealing bug; its name is pushed into `missing` so
/// the caller can surface a shape error. References to *other* sets pass through unchanged.
/// Uses interior mutability so the `Fn`-bound [`Record::map`] sub-walks can record misses.
pub fn seal_recursive_refs(
    set: &Rc<RecursiveSet>,
    kt: &KType,
    missing: &RefCell<Vec<String>>,
) -> KType {
    seal_refs_inner(set, None, kt, missing)
}

/// [`seal_recursive_refs`] with an extra binder rule: a [`KType::RecursiveRef`] naming
/// `binder.0` — the declaring name, which is *not* itself a set member — seals to a clone of
/// `binder.1`. A `UNION` uses this so a variant payload referencing the union's own name
/// (`Node :Tree` in `UNION Tree = (Leaf … Node :Tree)`) seals to the union of the set's
/// variant members (ruling F2), while variant-sibling references keep the `index_of` mapping.
pub fn seal_union_refs(
    set: &Rc<RecursiveSet>,
    binder_name: &str,
    binder_union: &KType,
    kt: &KType,
    missing: &RefCell<Vec<String>>,
) -> KType {
    seal_refs_inner(set, Some((binder_name, binder_union)), kt, missing)
}

/// Deep-seal core shared by [`seal_recursive_refs`] and [`seal_union_refs`]. `binder`, when
/// present, maps the declaring name to a replacement `KType` before the `index_of` lookup —
/// the name→`KType` widening ruling F2 needs.
fn seal_refs_inner(
    set: &Rc<RecursiveSet>,
    binder: Option<(&str, &KType)>,
    kt: &KType,
    missing: &RefCell<Vec<String>>,
) -> KType {
    let recurse = |inner: &KType| seal_refs_inner(set, binder, inner, missing);
    match kt {
        KType::RecursiveRef(name) => {
            if let Some((binder_name, binder_union)) = binder {
                if name == binder_name {
                    return binder_union.clone();
                }
            }
            match set.index_of(name) {
                Some(i) => KType::SetLocal(i),
                None => {
                    missing.borrow_mut().push(name.clone());
                    // Leave the transient in place; the caller records the miss and aborts
                    // before this survives into a sealed type.
                    kt.clone()
                }
            }
        }
        // A `SetRef` into *this* set (cross-sibling, resolved post-seal) folds to `SetLocal`;
        // a `SetRef` into another set is an external handle and passes through.
        KType::SetRef { set: other, index } if Rc::ptr_eq(other, set) => KType::SetLocal(*index),
        KType::List { element, .. } => KType::list(Box::new(recurse(element))),
        KType::Dict { key, value, .. } => {
            KType::dict(Box::new(recurse(key)), Box::new(recurse(value)))
        }
        KType::Record { fields, .. } => KType::record(Box::new(
            fields.map(|t| seal_refs_inner(set, binder, t, missing)),
        )),
        KType::KFunction { params, ret, .. } => KType::function_type(
            params.map(|t| seal_refs_inner(set, binder, t, missing)),
            Box::new(recurse(ret)),
        ),
        KType::ConstructorApply { ctor, args, .. } => {
            KType::constructor_apply(Box::new(recurse(ctor)), args.map(recurse))
        }
        // A union inside a schema seals member-wise, so a self / sibling reference among its
        // members folds to a `SetLocal` like any other.
        KType::Union { members, .. } => KType::union_of(members.iter().map(recurse).collect()),
        // Leaves and external handles pass through.
        other => other.clone(),
    }
}

/// Deep-walk `kt`, replacing every intra-set [`KType::SetLocal`] leaf with an external
/// [`KType::SetRef`] into `set`. Used when projecting a member's schema for navigation /
/// construction / matching, where a sibling reference must become a real handle. Other
/// leaves pass through unchanged; nested `SetRef`s (references to *other* sets) are left
/// alone — only the ambient set's own `SetLocal`s are bound.
pub fn resolve_set_locals(set: &Rc<RecursiveSet>, kt: &KType) -> KType {
    match kt {
        KType::SetLocal(i) => KType::SetRef {
            set: Rc::clone(set),
            index: *i,
        },
        KType::List { element, .. } => KType::list(Box::new(resolve_set_locals(set, element))),
        KType::Dict { key, value, .. } => KType::dict(
            Box::new(resolve_set_locals(set, key)),
            Box::new(resolve_set_locals(set, value)),
        ),
        KType::Record { fields, .. } => {
            KType::record(Box::new(fields.map(|t| resolve_set_locals(set, t))))
        }
        KType::KFunction { params, ret, .. } => KType::function_type(
            params.map(|t| resolve_set_locals(set, t)),
            Box::new(resolve_set_locals(set, ret)),
        ),
        KType::ConstructorApply { ctor, args, .. } => KType::constructor_apply(
            Box::new(resolve_set_locals(set, ctor)),
            args.map(|a| resolve_set_locals(set, a)),
        ),
        // Projecting a union member-wise binds each member's ambient `SetLocal`s to real handles.
        KType::Union { members, .. } => {
            KType::union_of(members.iter().map(|m| resolve_set_locals(set, m)).collect())
        }
        // Leaves and external handles pass through; only the ambient set's `SetLocal`s bind.
        other => other.clone(),
    }
}

impl NominalSchema {
    pub fn kind(&self) -> KKind {
        match self {
            NominalSchema::NewType(_) => KKind::NewType,
            NominalSchema::TypeConstructor { .. } => KKind::TypeConstructor,
        }
    }
}

/// Projected, navigable schema of one set member: its `SetLocal` sibling references are
/// resolved to external [`KType::SetRef`] handles, so each field/variant type matches and
/// navigates directly. Produced by [`RecursiveSet::projected_schema`].
pub enum ProjectedSchema {
    NewType(KType),
    TypeConstructor {
        schema: HashMap<String, KType>,
        param_names: Vec<String>,
    },
}

impl RecursiveSet {
    /// Project member `index`'s filled schema with sibling `SetLocal`s resolved to external
    /// `SetRef`s into `set`. Panics if the member's schema is not yet filled — every
    /// construction / navigation site runs after the member finalized.
    pub fn projected_schema(set: &Rc<Self>, index: usize) -> ProjectedSchema {
        let member = set.member(index);
        let borrow = member.schema();
        let schema = borrow
            .as_ref()
            .expect("projected_schema on an unfilled member — finalize must run first");
        match schema {
            NominalSchema::NewType(repr) => ProjectedSchema::NewType(resolve_set_locals(set, repr)),
            NominalSchema::TypeConstructor {
                schema,
                param_names,
            } => ProjectedSchema::TypeConstructor {
                schema: schema
                    .iter()
                    .map(|(k, v)| (k.clone(), resolve_set_locals(set, v)))
                    .collect(),
                param_names: param_names.clone(),
            },
        }
    }
}
