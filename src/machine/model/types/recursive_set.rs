//! `RecursiveSet` — the atomic unit of allocation, identity, and lift for nominal types.
//!
//! A strongly-connected component of mutually-recursive types (a self-recursive type, an
//! `A ↔ B` pair, a longer cycle) is sealed into one [`RecursiveSet`]. A non-recursive type
//! is a set of one. The set is `Rc`-owned (never arena-owned): lifting any reference to a
//! member is just `Rc::clone` of the set, so the whole group travels as a unit — no copy,
//! no visited-map cycle-walk, no `Rc<CallArena>` anchor.
//!
//! References:
//! - *Intra-set* (a member's schema naming a sibling) is [`KType::SetLocal`] — a bare index
//!   resolved against the ambient set during deep traversal. Never an `Rc`, so the set holds
//!   no internal refcount cycle and frees normally once its last external handle drops.
//! - *External* (the `bindings.types` entry, a field of a non-member, a param slot, a
//!   constructed value's `ktype()`) is [`KType::SetRef`] carrying `Rc<RecursiveSet>` + index.
//!
//! Identity is `(Rc::as_ptr(set), index)` — lift-stable because `Rc::clone` shares the same
//! allocation. A member's `name` / `scope_id` are diagnostics only, never identity.
//!
//! Members are sealed at SCC cycle-close: the set is created with the membership known (from
//! `detect_pending_cycle`), `kind` recorded eagerly, and each member's `schema` filled at its
//! own finalize — hence the [`RefCell`] two-phase cell.

use std::cell::{Ref, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use crate::machine::core::ScopeId;

use super::ktype::KType;
use super::record::Record;

/// Surface family of a nominal type. Drives `AnyUserType { kind }` wildcard admission;
/// payload-free, so it is `Copy` and cheap to compare.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum NominalKind {
    Struct,
    Tagged,
    Newtype,
    TypeConstructor,
}

impl NominalKind {
    /// Surface keyword rendered in diagnostics and `AnyUserType` names.
    pub fn surface_keyword(self) -> &'static str {
        match self {
            NominalKind::Struct => "Struct",
            NominalKind::Tagged => "Tagged",
            NominalKind::Newtype => "Newtype",
            NominalKind::TypeConstructor => "TypeConstructor",
        }
    }
}

/// A member's schema, owned by value inside the set. Sibling references inside these
/// `KType`s are [`KType::SetLocal`] indices into the enclosing [`RecursiveSet`].
#[derive(Debug)]
pub enum NominalSchema<'a> {
    /// Record schema in declaration order.
    Struct(Record<KType<'a>>),
    /// Tagged-union schema keyed by tag.
    Tagged(HashMap<String, KType<'a>>),
    /// Fresh nominal over a transparent representation; `repr` is not part of identity.
    Newtype(Box<KType<'a>>),
    /// Higher-kinded constructor: erased-parameter variant schema plus parameter names.
    TypeConstructor {
        schema: HashMap<String, KType<'a>>,
        param_names: Vec<String>,
    },
}

/// One nominal type within a [`RecursiveSet`]. `kind` is known at cycle-close; `schema` is
/// filled at the member's own finalize (two-phase), so it rides a [`RefCell`].
pub struct NominalMember<'a> {
    /// Diagnostics / rendering only — never identity.
    pub name: String,
    /// Origin scope, diagnostics only — never identity.
    pub scope_id: ScopeId,
    pub kind: NominalKind,
    schema: RefCell<Option<NominalSchema<'a>>>,
}

impl<'a> NominalMember<'a> {
    /// A member whose schema is not yet filled (cycle-close pre-seal).
    pub fn pending(name: String, scope_id: ScopeId, kind: NominalKind) -> Self {
        Self {
            name,
            scope_id,
            kind,
            schema: RefCell::new(None),
        }
    }

    /// Install the member's schema. Idempotent on a re-fill with equal shape is the caller's
    /// concern; a double-fill is a sealing bug.
    pub fn fill(&self, schema: NominalSchema<'a>) {
        *self.schema.borrow_mut() = Some(schema);
    }

    /// Whether the schema has been filled (the member's finalize has run).
    pub fn is_filled(&self) -> bool {
        self.schema.borrow().is_some()
    }

    /// Borrow the filled schema for deep traversal. `None` until the member finalizes.
    pub fn schema(&self) -> Ref<'_, Option<NominalSchema<'a>>> {
        self.schema.borrow()
    }
}

/// A sealed strongly-connected component of nominal types. Created at cycle-close with the
/// membership fixed; members fill their schemas as they finalize.
pub struct RecursiveSet<'a> {
    members: Vec<NominalMember<'a>>,
    /// `name → index` for sealing a member's transient `RecursiveRef(name)` to
    /// `SetLocal(index)`.
    index_of: HashMap<String, usize>,
}

impl<'a> RecursiveSet<'a> {
    /// Seal a set over the given members (declaration order). The `index_of` map keys each
    /// member's name to its slot so schema sealing can resolve sibling references.
    pub fn new(members: Vec<NominalMember<'a>>) -> Self {
        let index_of = members
            .iter()
            .enumerate()
            .map(|(i, m)| (m.name.clone(), i))
            .collect();
        Self { members, index_of }
    }

    pub fn member(&self, index: usize) -> &NominalMember<'a> {
        &self.members[index]
    }

    pub fn members(&self) -> &[NominalMember<'a>] {
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
    pub fn singleton(name: String, scope_id: ScopeId, schema: NominalSchema<'a>) -> Rc<Self> {
        let member = NominalMember::pending(name, scope_id, schema.kind());
        member.fill(schema);
        Rc::new(RecursiveSet::new(vec![member]))
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
pub fn seal_recursive_refs<'a>(
    set: &Rc<RecursiveSet<'a>>,
    kt: &KType<'a>,
    missing: &RefCell<Vec<String>>,
) -> KType<'a> {
    let recurse = |inner: &KType<'a>| seal_recursive_refs(set, inner, missing);
    match kt {
        KType::RecursiveRef(name) => match set.index_of(name) {
            Some(i) => KType::SetLocal(i),
            None => {
                missing.borrow_mut().push(name.clone());
                // Leave the transient in place; the caller records the miss and aborts
                // before this survives into a sealed type.
                kt.clone()
            }
        },
        // A `SetRef` into *this* set (cross-sibling, resolved post-seal) folds to `SetLocal`;
        // a `SetRef` into another set is an external handle and passes through.
        KType::SetRef { set: other, index } if Rc::ptr_eq(other, set) => KType::SetLocal(*index),
        KType::List(inner) => KType::List(Box::new(recurse(inner))),
        KType::Dict(k, v) => KType::Dict(Box::new(recurse(k)), Box::new(recurse(v))),
        KType::Record(fields) => KType::Record(Box::new(
            fields.map(|t| seal_recursive_refs(set, t, missing)),
        )),
        KType::KFunction { params, ret } => KType::KFunction {
            params: params.map(|t| seal_recursive_refs(set, t, missing)),
            ret: Box::new(recurse(ret)),
        },
        KType::KFunctor { params, ret, body } => KType::KFunctor {
            params: params.map(|t| seal_recursive_refs(set, t, missing)),
            ret: Box::new(recurse(ret)),
            body: *body,
        },
        KType::ConstructorApply { ctor, args } => KType::ConstructorApply {
            ctor: Box::new(recurse(ctor)),
            args: args.iter().map(recurse).collect(),
        },
        // Leaves and external handles pass through.
        other => other.clone(),
    }
}

/// Deep-walk `kt`, replacing every intra-set [`KType::SetLocal`] leaf with an external
/// [`KType::SetRef`] into `set`. Used when projecting a member's schema for navigation /
/// construction / matching, where a sibling reference must become a real handle. Other
/// leaves pass through unchanged; nested `SetRef`s (references to *other* sets) are left
/// alone — only the ambient set's own `SetLocal`s are bound.
pub fn resolve_set_locals<'a>(set: &Rc<RecursiveSet<'a>>, kt: &KType<'a>) -> KType<'a> {
    match kt {
        KType::SetLocal(i) => KType::SetRef {
            set: Rc::clone(set),
            index: *i,
        },
        KType::List(inner) => KType::List(Box::new(resolve_set_locals(set, inner))),
        KType::Dict(k, v) => KType::Dict(
            Box::new(resolve_set_locals(set, k)),
            Box::new(resolve_set_locals(set, v)),
        ),
        KType::Record(fields) => {
            KType::Record(Box::new(fields.map(|t| resolve_set_locals(set, t))))
        }
        KType::KFunction { params, ret } => KType::KFunction {
            params: params.map(|t| resolve_set_locals(set, t)),
            ret: Box::new(resolve_set_locals(set, ret)),
        },
        KType::KFunctor { params, ret, body } => KType::KFunctor {
            params: params.map(|t| resolve_set_locals(set, t)),
            ret: Box::new(resolve_set_locals(set, ret)),
            body: *body,
        },
        KType::ConstructorApply { ctor, args } => KType::ConstructorApply {
            ctor: Box::new(resolve_set_locals(set, ctor)),
            args: args.iter().map(|a| resolve_set_locals(set, a)).collect(),
        },
        // Leaves and external handles pass through; only the ambient set's `SetLocal`s bind.
        other => other.clone(),
    }
}

impl<'a> NominalSchema<'a> {
    /// Surface family of this schema.
    pub fn kind(&self) -> NominalKind {
        match self {
            NominalSchema::Struct(_) => NominalKind::Struct,
            NominalSchema::Tagged(_) => NominalKind::Tagged,
            NominalSchema::Newtype(_) => NominalKind::Newtype,
            NominalSchema::TypeConstructor { .. } => NominalKind::TypeConstructor,
        }
    }
}

/// Projected, navigable schema of one set member: its `SetLocal` sibling references are
/// resolved to external [`KType::SetRef`] handles, so each field/variant type matches and
/// navigates directly. Produced by [`RecursiveSet::projected_schema`].
pub enum ProjectedSchema<'a> {
    Struct(Record<KType<'a>>),
    Tagged(HashMap<String, KType<'a>>),
    Newtype(KType<'a>),
    TypeConstructor {
        schema: HashMap<String, KType<'a>>,
        param_names: Vec<String>,
    },
}

impl<'a> RecursiveSet<'a> {
    /// Project member `index`'s filled schema with sibling `SetLocal`s resolved to external
    /// `SetRef`s into `set`. Panics if the member's schema is not yet filled — every
    /// construction / navigation site runs after the member finalized.
    pub fn projected_schema(set: &Rc<Self>, index: usize) -> ProjectedSchema<'a> {
        let member = set.member(index);
        let borrow = member.schema();
        let schema = borrow
            .as_ref()
            .expect("projected_schema on an unfilled member — finalize must run first");
        match schema {
            NominalSchema::Struct(record) => {
                ProjectedSchema::Struct(record.map(|t| resolve_set_locals(set, t)))
            }
            NominalSchema::Tagged(map) => ProjectedSchema::Tagged(
                map.iter()
                    .map(|(k, v)| (k.clone(), resolve_set_locals(set, v)))
                    .collect(),
            ),
            NominalSchema::Newtype(repr) => ProjectedSchema::Newtype(resolve_set_locals(set, repr)),
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
