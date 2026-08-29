//! `RecursiveGroupWindow` — the declarator-local pre-seal record a group of co-declared nominal
//! types elaborates against — and [`seal_group`], the pure identity computation both this and the
//! ambient [`AnnouncedWindow`](super::declaration_window::AnnouncedWindow) reach.
//!
//! A window is a held record, not registry state: it holds the group's announced member names, each
//! member's owner and schema slot, the generativity nonce, and the declaring binders. Several can
//! be open at once under the park-capable scheduler — which a registry-hosted stack could not
//! express. Nothing on a window is digestible; nothing on it survives the seal.
//!
//! Inside the window a reference to a co-declared member is a [`TypeNode::Sibling`] handle: a bare
//! relative index, ordinary interned content, meaningful only against the window that minted it.
//! The seal rewrites every one of them to an absolute member handle.
//!
//! # Member identity is the computed component
//!
//! At the last fill the window seals. Identity is **not** the declared group: it is each member's
//! strongly-connected component under the sibling-reference relation, presented canonically in
//! name-symbol order (with the owning binder as a tiebreak the digest never sees). [`seal_group`]
//! extracts the reference edges, runs Tarjan, and digests the condensation in topological order —
//! every component after the components it references, so a cross-component reference folds the
//! referent's already-finished handle as ordinary external content while an intra-component one
//! stays relative.
//!
//! The consequences are the point:
//!
//! - A standalone declaration is a singleton component, and its presentation is byte-identical to
//!   the whole-declaration recipe — so no existing single-type digest moves.
//! - Adding an unreferenced member to a group perturbs nobody else's identity.
//! - A non-recursive member declared inside a group unifies with its standalone twin.
//! - Declaration order is immaterial; only name-symbol order and reference structure are.
//! - Two groups alike but for an external reference stay distinct, because that reference's
//!   handle is in the fold.
//!
//! Soundness rests on one observation: a sibling is either inside the member's own component — in
//! which case its content is part of the same fold — or upstream of it, in which case its full
//! finished digest is in the fold. There is no third case, so two members can share a digest only
//! by sharing content.
//!
//! See [design/typing/type-registry.md](../../../../design/typing/type-registry.md) and
//! [design/typing/type-identity.md](../../../../design/typing/type-identity.md).

use std::cell::RefCell;
use std::collections::HashMap;

use crate::machine::core::ScopeId;
use crate::machine::model::labels::{Symbol, TypeSymbol};

use super::kkind::KKind;
use super::ktype::KType;
use super::node::{NodeSchema, TypeNode};
use super::registry::TypeRegistry;
use super::sig_schema::TypeMemberMap;
use super::type_digest::{ComponentMember, TypeDigest, component_digest, member_ref_digest};

/// A member's schema while its window is open: the same shape as [`NodeSchema`], but its handles
/// may name a [`TypeNode::Sibling`] — a relative reference resolved only against this window.
#[derive(Clone)]
pub enum RelativeSchema {
    /// Fresh nominal over a transparent representation.
    NewType(KType),
    /// Higher-kinded constructor: erased-parameter variant schema plus parameter names, the
    /// Type-class labels the declaration interned.
    TypeConstructor {
        schema: TypeMemberMap,
        param_names: Vec<TypeSymbol>,
    },
}

impl RelativeSchema {
    /// The nominal family this schema declares.
    pub fn kind(&self) -> KKind {
        match self {
            RelativeSchema::NewType(_) => KKind::NewType,
            RelativeSchema::TypeConstructor { .. } => KKind::TypeConstructor,
        }
    }

    /// Rewrite every sibling handle through `resolve`, yielding the same shape.
    fn map_handles(&self, types: &TypeRegistry, resolve: &impl Fn(usize) -> KType) -> Self {
        match self {
            RelativeSchema::NewType(repr) => {
                RelativeSchema::NewType(rewrite_siblings(types, *repr, resolve))
            }
            RelativeSchema::TypeConstructor {
                schema,
                param_names,
            } => RelativeSchema::TypeConstructor {
                schema: schema
                    .iter()
                    .map(|(k, v)| (*k, rewrite_siblings(types, *v, resolve)))
                    .collect(),
                param_names: param_names.clone(),
            },
        }
    }

    /// The absolute twin, once every handle in `self` is already absolute.
    fn into_node_schema(self) -> NodeSchema {
        match self {
            RelativeSchema::NewType(repr) => NodeSchema::NewType(repr),
            RelativeSchema::TypeConstructor {
                schema,
                param_names,
            } => NodeSchema::TypeConstructor {
                schema,
                param_names,
            },
        }
    }

    /// The sibling indices this schema references, at any depth, in walk order.
    fn sibling_references(&self, types: &TypeRegistry, out: &mut Vec<usize>) {
        match self {
            RelativeSchema::NewType(repr) => collect_siblings(types, *repr, out),
            RelativeSchema::TypeConstructor { schema, .. } => {
                for value in schema.values() {
                    collect_siblings(types, *value, out);
                }
            }
        }
    }
}

/// One announced member of an open window. `kind` is known when the member is announced; the
/// schema arrives at the member's own finalize, hence the [`RefCell`].
pub struct PendingMember {
    /// The declared name — the bare tag for a variant. Unique among the members one binder owns,
    /// which with `owner` is what makes the canonical component presentation deterministic.
    pub name: TypeSymbol,
    /// The binder that owns this member: a `UNION`'s name, whose variants are reachable only
    /// through it. `None` for a member that is a standalone type in its own right.
    pub owner: Option<TypeSymbol>,
    /// The nominal family this member declares.
    pub kind: KKind,
    fill: RefCell<Option<RelativeSchema>>,
}

impl PendingMember {
    fn new(name: TypeSymbol, owner: Option<TypeSymbol>, kind: KKind) -> Self {
        Self {
            name,
            owner,
            kind,
            fill: RefCell::new(None),
        }
    }

    /// Whether the member's finalize has run.
    pub fn is_filled(&self) -> bool {
        self.fill.borrow().is_some()
    }
}

/// The declarator-local record a group of co-declared nominal types elaborates against, from
/// announcement to seal. The representation for a window a single declarator opens and seals —
/// it carries [`KKind::TypeConstructor`] schemas and grows by threaded discovery
/// ([`Self::sibling`]), neither of which the ambient
/// [`AnnouncedWindow`](super::declaration_window::AnnouncedWindow) needs.
pub struct RecursiveGroupWindow {
    members: RefCell<Vec<PendingMember>>,
    /// Each declaring binder and the indices of the members it owns — a `UNION`'s name over its
    /// variants. The binder is not itself a member: it denotes the union of the members it owns.
    binders: RefCell<Vec<(TypeSymbol, Vec<usize>)>>,
    /// Set when opaque ascription mints this window, so its per-application nonce folds into the
    /// minted member's component digest and two applications never unify. A generative window
    /// always has exactly one member, so the nonce belongs unambiguously to its one component.
    generative_nonce: Option<ScopeId>,
    /// What the seal minted. `None` while the window is open.
    sealed: RefCell<Option<SealedGroup>>,
}

/// What a window's seal produced: one absolute handle per member in announcement order, plus one
/// union handle per binder over exactly the members that binder owns.
#[derive(Clone)]
pub struct SealedGroup {
    pub members: Vec<KType>,
    pub binder_types: Vec<(TypeSymbol, KType)>,
}

impl SealedGroup {
    /// The union handle binder `name` denotes, if this seal declared one.
    pub fn binder_type(&self, name: TypeSymbol) -> Option<KType> {
        self.binder_types
            .iter()
            .find(|(binder, _)| *binder == name)
            .map(|(_, kt)| *kt)
    }
}

impl RecursiveGroupWindow {
    /// A window over `members` in announcement order, every one of them a standalone type owned by
    /// no binder — a `NEWTYPE`'s singleton, a type constructor's mint.
    pub fn new(members: Vec<(TypeSymbol, KKind)>) -> Self {
        Self {
            members: RefCell::new(
                members
                    .into_iter()
                    .map(|(name, kind)| PendingMember::new(name, None, kind))
                    .collect(),
            ),
            binders: RefCell::new(Vec::new()),
            generative_nonce: None,
            sealed: RefCell::new(None),
        }
    }

    /// A standalone `UNION`'s window: `binder` owns every one of `tags`, so no tag is
    /// bare-name-resolvable and the binder itself denotes their union. The one-binder special case
    /// of the same machinery a module-announced group runs.
    pub fn for_binder(binder: TypeSymbol, tags: Vec<TypeSymbol>) -> Self {
        let owned: Vec<usize> = (0..tags.len()).collect();
        Self {
            members: RefCell::new(
                tags.into_iter()
                    .map(|tag| PendingMember::new(tag, Some(binder), KKind::NewType))
                    .collect(),
            ),
            binders: RefCell::new(vec![(binder, owned)]),
            generative_nonce: None,
            sealed: RefCell::new(None),
        }
    }

    /// A generative window: opaque ascription's per-application mint, always one member. `nonce`
    /// (the minted module's `scope_id`) folds into that member's component digest, so two `:|`
    /// applications of one signature member over one representation stay distinct types.
    pub fn generative(name: TypeSymbol, kind: KKind, nonce: ScopeId) -> Self {
        Self {
            members: RefCell::new(vec![PendingMember::new(name, None, kind)]),
            binders: RefCell::new(Vec::new()),
            generative_nonce: Some(nonce),
            sealed: RefCell::new(None),
        }
    }

    /// The generativity nonce folded into this window's component digest, if any.
    pub fn generative_nonce(&self) -> Option<ScopeId> {
        self.generative_nonce
    }

    /// Index of the standalone member named `name`. Owned members — a `UNION`'s variants — never
    /// answer here: they are reached through their binder or the qualified sigil.
    pub fn member_index(&self, name: TypeSymbol) -> Option<usize> {
        self.members
            .borrow()
            .iter()
            .position(|m| m.owner.is_none() && m.name == name)
    }

    /// Index of the member `binder` owns under the bare tag `tag` — the qualified-sigil lookup,
    /// scoped by the binder's own member list so the same tag under two binders never collides.
    ///
    /// `tag` probes by bare symbol bits: a variant tag arriving from a record-literal field name
    /// carries no class, and the member list it is matched against is keyed by the `TypeSymbol` the
    /// declaration minted. Symbol equality is text equality, so a hit witnesses the class rather
    /// than asserting it ([design/label-interning.md](../../../../design/label-interning.md)).
    pub fn variant_index(&self, binder: TypeSymbol, tag: Symbol) -> Option<usize> {
        let owned = self.binder_members(binder)?;
        let members = self.members.borrow();
        owned
            .into_iter()
            .find(|index| members[*index].name.symbol() == tag)
    }

    /// Whether `name` is a declaring binder of this window.
    pub fn binds(&self, name: TypeSymbol) -> bool {
        self.binders
            .borrow()
            .iter()
            .any(|(binder, _)| *binder == name)
    }

    /// The member indices `binder` owns, in announcement order.
    fn binder_members(&self, binder: TypeSymbol) -> Option<Vec<usize>> {
        self.binders
            .borrow()
            .iter()
            .find(|(name, _)| *name == binder)
            .map(|(_, owned)| owned.clone())
    }

    /// What the seal minted, or `None` while the window is still open.
    pub fn sealed(&self) -> Option<SealedGroup> {
        self.sealed.borrow().clone()
    }

    /// Whether the window has sealed — a cheap probe that clones nothing. Once sealed, a member
    /// name resolves to its bound absolute handle, not the relative `Sibling` back-edge.
    pub fn is_sealed(&self) -> bool {
        self.sealed.borrow().is_some()
    }

    /// Number of announced members.
    pub fn len(&self) -> usize {
        self.members.borrow().len()
    }

    pub fn is_empty(&self) -> bool {
        self.members.borrow().is_empty()
    }

    /// The announced member names in announcement order.
    pub fn member_names(&self) -> Vec<TypeSymbol> {
        self.members.borrow().iter().map(|m| m.name).collect()
    }

    /// The names of every member whose finalize has not run — empty once the window can seal. A
    /// name here after the declarator finished is a reference to a type the group never declared.
    pub fn unfilled_member_names(&self) -> Vec<TypeSymbol> {
        self.members
            .borrow()
            .iter()
            .filter(|m| !m.is_filled())
            .map(|m| m.name)
            .collect()
    }

    /// The names a reference may reach bare: the standalone members and the declaring binders.
    /// An owned member — a `UNION`'s variant — is absent, because it is reached only through its
    /// binder or the qualified sigil.
    pub fn bare_reachable_names(&self) -> Vec<TypeSymbol> {
        self.members
            .borrow()
            .iter()
            .filter(|m| m.owner.is_none())
            .map(|m| m.name)
            .chain(self.binders.borrow().iter().map(|(name, _)| *name))
            .collect()
    }

    /// Every still-unfilled member as `(name, owner)` — the owner is who carries the member's
    /// declaration placeholder, since a variant stamps none of its own.
    pub fn unfilled_members(&self) -> Vec<(TypeSymbol, Option<TypeSymbol>)> {
        self.members
            .borrow()
            .iter()
            .filter(|m| !m.is_filled())
            .map(|m| (m.name, m.owner))
            .collect()
    }

    /// Every `(name, handle)` this window's seal installs: the standalone members and the binders.
    /// Empty while the window is open. Variants are absent — a variant is reached through its
    /// binder's union node, never by name.
    pub fn installable(&self) -> Vec<(TypeSymbol, KType)> {
        let Some(sealed) = self.sealed() else {
            return Vec::new();
        };
        self.members
            .borrow()
            .iter()
            .enumerate()
            .filter(|(_, member)| member.owner.is_none())
            .map(|(index, member)| (member.name, sealed.members[index]))
            .chain(sealed.binder_types.iter().copied())
            .collect()
    }

    /// Whether the member at `index` has had its finalize run — the by-index half of
    /// [`Self::unfilled_member_names`], for a consumer holding a relative handle rather than a name.
    pub fn member_is_filled(&self, index: usize) -> bool {
        self.members
            .borrow()
            .get(index)
            .is_some_and(|m| m.is_filled())
    }

    /// The relative handle naming standalone member `name`. Announces the member first if the
    /// window has not seen the name — the forward-reference case inside a declarator whose own
    /// member list is discovered as its schema is walked. `kind` is the family to announce it
    /// with, ignored when the name is already announced.
    pub fn sibling(&self, name: TypeSymbol, kind: KKind, types: &TypeRegistry) -> KType {
        let index = match self.member_index(name) {
            Some(index) => index,
            None => {
                let mut members = self.members.borrow_mut();
                let index = members.len();
                members.push(PendingMember::new(name, None, kind));
                index
            }
        };
        types.intern(TypeNode::Sibling(index))
    }

    /// The relative type binder `name` denotes: the union of the members it owns, each as its
    /// relative sibling handle. A `UNION`'s variant payload naming the union itself resolves
    /// through here.
    pub fn binder_union(&self, name: TypeSymbol, types: &TypeRegistry) -> Option<KType> {
        let owned = self.binder_members(name)?;
        let siblings: Vec<KType> = owned
            .into_iter()
            .map(|index| types.intern(TypeNode::Sibling(index)))
            .collect();
        Some(types.union_of(&siblings))
    }

    /// Fill member `index`'s schema and, if that was the last unfilled member, seal the window.
    /// Returns what the seal minted on the fill that seals (and on any later call once sealed),
    /// `None` while members remain open.
    ///
    /// The single sealing seam: [`PendingMember`]'s fill slot is private, so no site can install a
    /// schema without reaching the identity computation below.
    pub fn fill_member(
        &self,
        index: usize,
        schema: RelativeSchema,
        types: &TypeRegistry,
    ) -> Option<SealedGroup> {
        *self.members.borrow()[index].fill.borrow_mut() = Some(schema);
        if let Some(sealed) = self.sealed.borrow().clone() {
            return Some(sealed);
        }
        let complete = self.members.borrow().iter().all(PendingMember::is_filled);
        if !complete {
            return None;
        }
        let members = self.members.borrow();
        let binders = self.binders.borrow();
        let inputs: Vec<SealMemberInput> = members
            .iter()
            .map(|m| SealMemberInput {
                name: m.name,
                owner: m.owner,
                kind: m.kind,
                schema: m
                    .fill
                    .borrow()
                    .clone()
                    .expect("the window seals only once every member is filled"),
            })
            .collect();
        let binder_inputs: Vec<SealBinderInput<'_>> = binders
            .iter()
            .map(|(name, owned)| SealBinderInput {
                name: *name,
                members: owned.as_slice(),
            })
            .collect();
        let sealed = seal_group(&inputs, &binder_inputs, self.generative_nonce, types);
        drop(inputs);
        drop(binder_inputs);
        drop(members);
        drop(binders);
        *self.sealed.borrow_mut() = Some(sealed.clone());
        Some(sealed)
    }

    /// Seal a one-member window in place — the standalone declarators' path, where announcement,
    /// fill and seal all happen at one site. `nonce` makes it a generative mint. The member's own
    /// self-reference is `Sibling(0)`, so a self-recursive standalone type needs no other setup.
    pub fn seal_singleton(
        name: TypeSymbol,
        schema: RelativeSchema,
        nonce: Option<ScopeId>,
        types: &TypeRegistry,
    ) -> KType {
        let kind = schema.kind();
        let window = match nonce {
            Some(nonce) => Self::generative(name, kind, nonce),
            None => Self::new(vec![(name, kind)]),
        };
        window
            .fill_member(0, schema, types)
            .expect("a one-member window seals on its only fill")
            .members[0]
    }
}

/// One filled member handed to [`seal_group`] — the pure boundary into the identity computation.
pub struct SealMemberInput {
    /// The declared name: the bare tag for a variant. Digested, and the primary canonical sort key.
    pub name: TypeSymbol,
    /// The binder that owns this member, if any. A **sort tiebreak only** — never folded into
    /// [`component_digest`], so a module-hosted variant digests identically to its standalone twin
    /// and two same-tag variants under different binders take distinct fold positions.
    pub owner: Option<TypeSymbol>,
    pub kind: KKind,
    pub schema: RelativeSchema,
}

/// One declaring binder handed to [`seal_group`]: its name and the indices of the members it owns.
pub struct SealBinderInput<'m> {
    pub name: TypeSymbol,
    pub members: &'m [usize],
}

/// Turn a filled group into interned content: one absolute handle per member in announcement
/// order, plus each binder's union over the members it owns. Implements the per-component identity
/// described in this module's header.
pub fn seal_group(
    members: &[SealMemberInput],
    binders: &[SealBinderInput<'_>],
    generative_nonce: Option<ScopeId>,
    types: &TypeRegistry,
) -> SealedGroup {
    let count = members.len();

    // Edges: `member → sibling it references`. A referent must be digested first, so the
    // condensation is processed successor-first — which is exactly Tarjan's emission order.
    let mut edges: Vec<Vec<usize>> = Vec::with_capacity(count);
    for member in members {
        let mut references = Vec::new();
        member.schema.sibling_references(types, &mut references);
        references.sort_unstable();
        references.dedup();
        edges.push(references);
    }

    // `member index → its finished handle`, filled component by component.
    let mut handles: Vec<Option<KType>> = vec![None; count];
    // `member index → (its component's digest, its position in that component, size)`.
    let mut placement: Vec<Option<(TypeDigest, usize, usize)>> = vec![None; count];

    for component in tarjan_components(&edges) {
        // Canonical presentation order is the numeric order of the members' name symbols, with the
        // owning binder as tiebreak so two same-tag variants of different binders take stable
        // distinct positions. It is the order the digest feed folds in, so index and feed agree.
        // The owner orders but does not digest.
        let mut order = component.clone();
        order.sort_by(|a, b| {
            (members[*a].name, members[*a].owner).cmp(&(members[*b].name, members[*b].owner))
        });
        let position_of: HashMap<usize, usize> = order
            .iter()
            .enumerate()
            .map(|(position, member)| (*member, position))
            .collect();

        // Re-encode each member's schema for the fold: an intra-component reference becomes a
        // relative index into *this component's* canonical order, a cross-component one folds
        // the referent's already-finished handle as ordinary external content.
        let presented: Vec<NodeSchema> = {
            let resolve = |sibling: usize| match position_of.get(&sibling) {
                Some(position) => types.intern(TypeNode::Sibling(*position)),
                None => handles[sibling].expect(
                    "a cross-component sibling is upstream, so its component sealed already",
                ),
            };
            order
                .iter()
                .map(|member| {
                    members[*member]
                        .schema
                        .map_handles(types, &resolve)
                        .into_node_schema()
                })
                .collect()
        };
        let component_members: Vec<ComponentMember<'_>> = order
            .iter()
            .zip(presented.iter())
            .map(|(member, schema)| ComponentMember {
                name: members[*member].name,
                kind: members[*member].kind,
                schema,
            })
            .collect();
        // A generative window has exactly one member, so its nonce belongs to the one
        // component the loop ever visits.
        let digest = component_digest(generative_nonce, &component_members);
        drop(component_members);

        for (position, member) in order.iter().enumerate() {
            handles[*member] = Some(KType::from_digest(member_ref_digest(digest, position)));
            placement[*member] = Some((digest, position, order.len()));
        }
    }

    // Every handle is minted, so a member's schema can now be rewritten absolute — including
    // the cyclic edges, which are just handles into content the registry already keys.
    let absolute = |sibling: usize| {
        handles[sibling].expect("every member is placed before any schema is made absolute")
    };
    let mut sealed: Vec<KType> = Vec::with_capacity(count);
    for index in 0..count {
        let (scc_digest, position, scc_size) =
            placement[index].expect("Tarjan covers every member");
        let schema = members[index]
            .schema
            .map_handles(types, &absolute)
            .into_node_schema();
        let handle = types.intern(TypeNode::SetMember {
            scc_digest,
            index: position,
            scc_size,
            name: members[index].name,
            kind: members[index].kind,
            schema,
        });
        debug_assert_eq!(
            handle,
            handles[index].expect("placed"),
            "the interned member node must key at the handle its component derived",
        );
        sealed.push(handle);
    }
    let binder_types = binders
        .iter()
        .map(|binder| {
            let owned: Vec<KType> = binder.members.iter().map(|index| sealed[*index]).collect();
            (binder.name, types.union_of(&owned))
        })
        .collect();
    SealedGroup {
        members: sealed,
        binder_types,
    }
}

/// Deep-rewrite every [`TypeNode::Sibling`] in `kt` through `resolve`, re-interning each composite
/// on the way out. Recurses through exactly the composite shapes a schema can nest a sibling
/// inside; a sealed member handle is a leaf, so a cyclic edge into already-sealed content
/// terminates here rather than descending forever.
fn rewrite_siblings(types: &TypeRegistry, kt: KType, resolve: &impl Fn(usize) -> KType) -> KType {
    match types.node(kt) {
        TypeNode::Sibling(index) => resolve(index),
        TypeNode::List { element } => {
            let element = rewrite_siblings(types, element, resolve);
            types.list(element)
        }
        TypeNode::Dict { key, value } => {
            let key = rewrite_siblings(types, key, resolve);
            let value = rewrite_siblings(types, value, resolve);
            types.dict(key, value)
        }
        TypeNode::Record { fields } => {
            let fields = fields.map(|t| rewrite_siblings(types, *t, resolve));
            types.record(fields)
        }
        TypeNode::KFunction { params, ret } => {
            let params = params.map(|t| rewrite_siblings(types, *t, resolve));
            let ret = rewrite_siblings(types, ret, resolve);
            types.function_type(params, ret)
        }
        TypeNode::ConstructorApply {
            constructor,
            arguments,
        } => {
            let constructor = rewrite_siblings(types, constructor, resolve);
            let arguments = arguments.map(|t| rewrite_siblings(types, *t, resolve));
            types.constructor_apply(constructor, arguments)
        }
        // A union rewrites member-wise, so a self / sibling reference among its members binds like
        // any other. A rewritten sibling names a still-uninterned member of this group, so the
        // rebuild dedups and collapses without reading member nodes ([`intern_union_flat`]) — the
        // members are already flat.
        TypeNode::Union { members } => {
            let members: Vec<KType> = members
                .into_iter()
                .map(|m| rewrite_siblings(types, m, resolve))
                .collect();
            types.intern_union_flat(&members)
        }
        // Leaves and already-absolute handles pass through.
        _ => kt,
    }
}

/// Collect every sibling index `kt` references, at any depth. Mirrors [`rewrite_siblings`]'s walk.
fn collect_siblings(types: &TypeRegistry, kt: KType, out: &mut Vec<usize>) {
    match types.node(kt) {
        TypeNode::Sibling(index) => out.push(index),
        TypeNode::List { element } => collect_siblings(types, element, out),
        TypeNode::Dict { key, value } => {
            collect_siblings(types, key, out);
            collect_siblings(types, value, out);
        }
        TypeNode::Record { fields } => {
            for t in fields.values() {
                collect_siblings(types, *t, out);
            }
        }
        TypeNode::KFunction { params, ret } => {
            for t in params.values() {
                collect_siblings(types, *t, out);
            }
            collect_siblings(types, ret, out);
        }
        TypeNode::ConstructorApply {
            constructor,
            arguments,
        } => {
            collect_siblings(types, constructor, out);
            for t in arguments.values() {
                collect_siblings(types, *t, out);
            }
        }
        TypeNode::Union { members } => {
            for m in members {
                collect_siblings(types, m, out);
            }
        }
        _ => {}
    }
}

/// Tarjan's strongly-connected components over `edges` (`edges[i]` = the members `i` references).
///
/// Components come back in the algorithm's natural emission order, which is a reverse topological
/// order of the condensation: a component is emitted only after every component it references. The
/// seal depends on exactly that — a cross-component reference must already have a finished handle
/// when the referring component is digested.
fn tarjan_components(edges: &[Vec<usize>]) -> Vec<Vec<usize>> {
    struct State<'e> {
        edges: &'e [Vec<usize>],
        index: usize,
        indices: Vec<Option<usize>>,
        lowlink: Vec<usize>,
        on_stack: Vec<bool>,
        stack: Vec<usize>,
        components: Vec<Vec<usize>>,
    }

    fn strong_connect(state: &mut State<'_>, v: usize) {
        state.indices[v] = Some(state.index);
        state.lowlink[v] = state.index;
        state.index += 1;
        state.stack.push(v);
        state.on_stack[v] = true;
        for w in state.edges[v].clone() {
            match state.indices[w] {
                None => {
                    strong_connect(state, w);
                    state.lowlink[v] = state.lowlink[v].min(state.lowlink[w]);
                }
                Some(w_index) if state.on_stack[w] => {
                    state.lowlink[v] = state.lowlink[v].min(w_index);
                }
                Some(_) => {}
            }
        }
        if state.lowlink[v] == state.indices[v].expect("v was just indexed") {
            let mut component = Vec::new();
            loop {
                let w = state.stack.pop().expect("the stack holds v");
                state.on_stack[w] = false;
                component.push(w);
                if w == v {
                    break;
                }
            }
            state.components.push(component);
        }
    }

    let count = edges.len();
    let mut state = State {
        edges,
        index: 0,
        indices: vec![None; count],
        lowlink: vec![0; count],
        on_stack: vec![false; count],
        stack: Vec::new(),
        components: Vec::new(),
    };
    for v in 0..count {
        if state.indices[v].is_none() {
            strong_connect(&mut state, v);
        }
    }
    state.components
}

#[cfg(test)]
mod tests;
