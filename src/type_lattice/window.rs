//! `RecursiveGroupWindow` — the declarator-local pre-seal record a group of co-declared nominal
//! types elaborates against — and [`seal_group`], the pure identity computation it reaches.
//!
//! A window is a held record, not registry state: it holds the group's announced member names, each
//! member's owner and schema slot, the generativity nonce, and the declaring binders. Several can
//! be open at once, which a registry-hosted stack could not express. Nothing on a window is
//! digestible; nothing on it survives the seal. Building a group's relative schemas from the AST is
//! the elaborator's; the lattice only opens and seals the window.
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
//!
//! # Unions across the seal
//!
//! A relative schema builds its unions through the ordinary canonicalizing door while it still
//! holds `Sibling` handles. That is exact: the order relates a `Sibling` as an atom with the same
//! profile as the sealed member it stands for, and distinct indices name distinct members, so a
//! union canonical before the seal is canonical after it. The seal's rewrite therefore re-interns
//! through the flat door, which reads no member nodes — necessary, because a rewritten sibling
//! handle names a member the registry has not interned yet.
//!
//! See [design/typing/type-lattice.md](../../design/typing/type-lattice.md).

use std::cell::{OnceCell, RefCell};
use std::collections::HashMap;

use crate::memory::ScopeId;
use crate::parse::{Symbol, TypeSymbol};

use super::digest::{ComponentMember, TypeDigest, component_digest, member_ref_digest};
use super::handle::KType;
use super::kind::KKind;
use super::node::NodeSchema;
use super::registry::TypeRegistry;
use super::schema::TypeMemberMap;
use super::substitute::{collect_siblings, rewrite_siblings};

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
pub(super) struct PendingMember {
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
    fn is_filled(&self) -> bool {
        self.fill.borrow().is_some()
    }
}

/// The declarator-local record a group of co-declared nominal types elaborates against, from
/// announcement to seal. It carries [`KKind::TypeConstructor`] schemas and grows by threaded
/// discovery ([`Self::sibling`]).
pub struct RecursiveGroupWindow {
    members: RefCell<Vec<PendingMember>>,
    /// Each declaring binder and the indices of the members it owns — a `UNION`'s name over its
    /// variants. The binder is not itself a member: it denotes the union of the members it owns.
    binders: RefCell<Vec<(TypeSymbol, Vec<usize>)>>,
    /// Set when opaque ascription mints this window, so its per-application nonce folds into the
    /// minted member's component digest and two applications never unify. A generative window
    /// always has exactly one member, so the nonce belongs unambiguously to its one component.
    generative_nonce: Option<ScopeId>,
    /// What the seal minted. Empty while the window is open; set exactly once.
    sealed: OnceCell<SealedGroup>,
}

/// What a window's seal produced: one absolute handle per member in announcement order, plus one
/// union handle per binder over exactly the members that binder owns. Every question about a
/// sealed group is answered here; the window itself only answers about an open one.
pub struct SealedGroup {
    members: Vec<SealedMember>,
    binder_types: Vec<(TypeSymbol, KType)>,
}

/// One member's absolute handle with the name and owner it was announced under.
struct SealedMember {
    name: TypeSymbol,
    owner: Option<TypeSymbol>,
    kt: KType,
}

impl SealedGroup {
    /// The absolute handle of the member at announcement index `index`.
    pub fn member(&self, index: usize) -> Option<KType> {
        self.members.get(index).map(|m| m.kt)
    }

    /// The union handle binder `name` denotes, if this seal declared one.
    pub fn binder_type(&self, name: TypeSymbol) -> Option<KType> {
        self.binder_types
            .iter()
            .find(|(binder, _)| *binder == name)
            .map(|(_, kt)| *kt)
    }

    /// Every `(name, handle)` this seal installs: the standalone members, then the binders.
    /// Variants are absent — a variant is reached through its binder's union node, never by name.
    pub fn installable(&self) -> impl Iterator<Item = (TypeSymbol, KType)> + '_ {
        self.members
            .iter()
            .filter(|m| m.owner.is_none())
            .map(|m| (m.name, m.kt))
            .chain(self.binder_types.iter().copied())
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
            sealed: OnceCell::new(),
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
            sealed: OnceCell::new(),
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
            sealed: OnceCell::new(),
        }
    }

    /// Index of the standalone member named `name`. Owned members — a `UNION`'s variants — never
    /// answer here: they are reached through their binder or by member projection off it.
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
    /// than asserting it ([design/label-interning.md](../../design/label-interning.md)).
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

    /// What the seal minted, or `None` while the window is still open. Once sealed, a member name
    /// resolves to its bound absolute handle, not the relative `Sibling` back-edge.
    pub fn sealed(&self) -> Option<&SealedGroup> {
        self.sealed.get()
    }

    /// The names a reference may reach bare: the standalone members and the declaring binders.
    /// An owned member — a `UNION`'s variant — is absent, because it is reached only through its
    /// binder or by member projection off it.
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
    /// declaration placeholder, since a variant stamps none of its own. Empty once the window can
    /// seal; a name here after the declarator finished is a reference to a type the group never
    /// declared.
    pub fn unfilled_members(&self) -> Vec<(TypeSymbol, Option<TypeSymbol>)> {
        self.members
            .borrow()
            .iter()
            .filter(|m| !m.is_filled())
            .map(|m| (m.name, m.owner))
            .collect()
    }

    /// Whether the member at `index` has had its finalize run — the by-index half of
    /// [`Self::unfilled_members`], for a consumer holding a relative handle rather than a name.
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
        types.sibling(index)
    }

    /// The relative type binder `name` denotes: the union of the members it owns, each as its
    /// relative sibling handle. A `UNION`'s variant payload naming the union itself resolves
    /// through here.
    pub fn binder_union(&self, name: TypeSymbol, types: &TypeRegistry) -> Option<KType> {
        let owned = self.binder_members(name)?;
        let siblings: Vec<KType> = owned
            .into_iter()
            .map(|index| types.sibling(index))
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
    ) -> Option<&SealedGroup> {
        *self.members.borrow()[index].fill.borrow_mut() = Some(schema);
        if let Some(sealed) = self.sealed.get() {
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
        Some(self.sealed.get_or_init(|| sealed))
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
            .and_then(|sealed| sealed.member(0))
            .expect("a one-member window seals on its only fill")
    }
}

/// One filled member handed to [`seal_group`] — the pure boundary into the identity computation.
pub(super) struct SealMemberInput {
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
pub(super) struct SealBinderInput<'m> {
    pub name: TypeSymbol,
    pub members: &'m [usize],
}

/// Turn a filled group into interned content: one absolute handle per member in announcement
/// order, plus each binder's union over the members it owns. Implements the per-component identity
/// described in this module's header.
pub(super) fn seal_group(
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
                Some(position) => types.sibling(*position),
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
    let mut sealed: Vec<SealedMember> = Vec::with_capacity(count);
    for index in 0..count {
        let (scc_digest, position, scc_size) =
            placement[index].expect("Tarjan covers every member");
        let schema = members[index]
            .schema
            .map_handles(types, &absolute)
            .into_node_schema();
        let handle = types.set_member(
            scc_digest,
            position,
            scc_size,
            members[index].name,
            members[index].kind,
            schema,
        );
        debug_assert_eq!(
            handle,
            handles[index].expect("placed"),
            "the interned member node must key at the handle its component derived",
        );
        sealed.push(SealedMember {
            name: members[index].name,
            owner: members[index].owner,
            kt: handle,
        });
    }
    let binder_types = binders
        .iter()
        .map(|binder| {
            let owned: Vec<KType> = binder
                .members
                .iter()
                .map(|index| sealed[*index].kt)
                .collect();
            (binder.name, types.union_of(&owned))
        })
        .collect();
    SealedGroup {
        members: sealed,
        binder_types,
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
