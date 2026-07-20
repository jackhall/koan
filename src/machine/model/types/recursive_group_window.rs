//! `RecursiveGroupWindow` — the pre-seal record a group of co-declared nominal types elaborates
//! against, and the seal that turns it into interned registry content.
//!
//! A window is a scope-carried record, not registry state: it holds the group's announced member
//! names, each member's schema slot, the generativity nonce, and the declaring binder's name. It
//! rides the scope chain ([`Scope::nearest_recursive_window`](crate::machine::core::Scope)) so
//! several windows can be open at once under the park-capable scheduler — which a registry-hosted
//! stack could not express. Nothing on a window is digestible; nothing on it survives the seal.
//!
//! Inside the window a reference to a co-declared member is a [`TypeNode::Sibling`] handle: a bare
//! relative index, ordinary interned content, meaningful only against the window that minted it.
//! The seal rewrites every one of them to an absolute member handle.
//!
//! # Member identity is the computed component
//!
//! At the last fill the window seals. Identity is **not** the declared group: it is each member's
//! strongly-connected component under the sibling-reference relation, presented canonically in
//! member-name order. [`seal`](RecursiveGroupWindow::fill_member) extracts the reference edges,
//! runs Tarjan, and digests the condensation in topological order — every component after the
//! components it references, so a cross-component reference folds the referent's already-finished
//! handle as ordinary external content while an intra-component one stays relative.
//!
//! The consequences are the point:
//!
//! - A standalone declaration is a singleton component, and its presentation is byte-identical to
//!   the whole-declaration recipe — so no existing single-type digest moves.
//! - Adding an unreferenced member to a group perturbs nobody else's identity.
//! - A non-recursive member declared inside a group unifies with its standalone twin.
//! - Declaration order is immaterial; only name order and reference structure are.
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
use std::rc::Rc;

use crate::machine::core::ScopeId;

use super::kkind::KKind;
use super::ktype::KType;
use super::node::{NodeSchema, TypeNode};
use super::registry::TypeRegistry;
use super::type_digest::{component_digest, member_ref_digest, ComponentMember, TypeDigest};

/// A member's schema while its window is open: the same shape as [`NodeSchema`], but its handles
/// may name a [`TypeNode::Sibling`] — a relative reference resolved only against this window.
#[derive(Clone)]
pub enum RelativeSchema {
    /// Fresh nominal over a transparent representation.
    NewType(KType),
    /// Higher-kinded constructor: erased-parameter variant schema plus parameter names.
    TypeConstructor {
        schema: HashMap<String, KType>,
        param_names: Vec<String>,
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
                    .map(|(k, v)| (k.clone(), rewrite_siblings(types, *v, resolve)))
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
    /// The declared name. Unique within a window, which is what makes name order a canonical
    /// component presentation with no further refinement.
    pub name: String,
    /// One of the three nominal families `Tagged` / `NewType` / `TypeConstructor`.
    pub kind: KKind,
    fill: RefCell<Option<RelativeSchema>>,
}

impl PendingMember {
    fn new(name: String, kind: KKind) -> Self {
        Self {
            name,
            kind,
            fill: RefCell::new(None),
        }
    }

    /// Whether the member's finalize has run.
    pub fn is_filled(&self) -> bool {
        self.fill.borrow().is_some()
    }
}

/// The record a group of co-declared nominal types elaborates against, from announcement to seal.
pub struct RecursiveGroupWindow {
    members: RefCell<Vec<PendingMember>>,
    /// `name → index`, so a reference by name mints the right relative handle.
    index_of: RefCell<HashMap<String, usize>>,
    /// Set when opaque ascription mints this window, so its per-application nonce folds into the
    /// minted member's component digest and two applications never unify. A generative window
    /// always has exactly one member, so the nonce belongs unambiguously to its one component.
    generative_nonce: Option<ScopeId>,
    /// The declaring name, when it is *not* itself a member — a `UNION`'s own binder, which
    /// denotes the union of every variant rather than any one of them.
    binder: Option<String>,
    /// What the seal minted. `None` while the window is open.
    sealed: RefCell<Option<SealedGroup>>,
}

/// What a window's seal produced: one absolute handle per member in declaration order, plus the
/// `Group` handle over them that a `RECURSIVE TYPES` block's own name binds to.
#[derive(Clone)]
pub struct SealedGroup {
    pub members: Vec<KType>,
    pub group: KType,
}

impl RecursiveGroupWindow {
    /// A window over `members` in declaration order. `binder` names the declaring type when it is
    /// not one of the members (a `UNION`'s own name); `None` when every announced name is a member
    /// (a `NEWTYPE`, a `RECURSIVE TYPES` block).
    pub fn new(members: Vec<(String, KKind)>, binder: Option<String>) -> Rc<Self> {
        let index_of = members
            .iter()
            .enumerate()
            .map(|(index, (name, _))| (name.clone(), index))
            .collect();
        Rc::new(Self {
            members: RefCell::new(
                members
                    .into_iter()
                    .map(|(name, kind)| PendingMember::new(name, kind))
                    .collect(),
            ),
            index_of: RefCell::new(index_of),
            generative_nonce: None,
            binder,
            sealed: RefCell::new(None),
        })
    }

    /// A generative window: opaque ascription's per-application mint, always one member. `nonce`
    /// (the minted module's `scope_id`) folds into that member's component digest, so two `:|`
    /// applications of one signature member over one representation stay distinct types.
    pub fn generative(name: String, kind: KKind, nonce: ScopeId) -> Rc<Self> {
        Rc::new(Self {
            members: RefCell::new(vec![PendingMember::new(name.clone(), kind)]),
            index_of: RefCell::new([(name, 0)].into_iter().collect()),
            generative_nonce: Some(nonce),
            binder: None,
            sealed: RefCell::new(None),
        })
    }

    /// The declaring binder name, when it is not itself a member.
    pub fn binder(&self) -> Option<String> {
        self.binder.clone()
    }

    /// The generativity nonce folded into this window's component digest, if any.
    pub fn generative_nonce(&self) -> Option<ScopeId> {
        self.generative_nonce
    }

    /// Index of the member named `name`, if the window announces it.
    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.index_of.borrow().get(name).copied()
    }

    /// Whether `name` is a member of this window — the in-flight test the finalize gate keys on.
    pub fn holds(&self, name: &str) -> bool {
        self.index_of.borrow().contains_key(name)
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

    /// The announced member names in declaration order.
    pub fn member_names(&self) -> Vec<String> {
        self.members
            .borrow()
            .iter()
            .map(|m| m.name.clone())
            .collect()
    }

    /// The names of every member whose finalize has not run — empty once the window can seal. A
    /// name here after the declarator finished is a reference to a type the group never declared.
    pub fn unfilled_member_names(&self) -> Vec<String> {
        self.members
            .borrow()
            .iter()
            .filter(|m| !m.is_filled())
            .map(|m| m.name.clone())
            .collect()
    }

    /// The relative handle naming member `name`. Announces the member first if the window has not
    /// seen the name — the forward-reference case inside a declarator whose own member list is
    /// discovered as its schema is walked. `kind` is the family to announce it with, ignored when
    /// the name is already announced.
    pub fn sibling(&self, name: &str, kind: KKind, types: &TypeRegistry) -> KType {
        let index = match self.index_of(name) {
            Some(index) => index,
            None => {
                let mut members = self.members.borrow_mut();
                let index = members.len();
                members.push(PendingMember::new(name.to_string(), kind));
                self.index_of.borrow_mut().insert(name.to_string(), index);
                index
            }
        };
        types.intern(TypeNode::Sibling(index))
    }

    /// The type the window's own binder name denotes: the union of every announced member. A
    /// `UNION`'s variant payload naming the union itself resolves through here.
    pub fn binder_union(&self, types: &TypeRegistry) -> KType {
        let siblings: Vec<KType> = (0..self.len())
            .map(|index| types.intern(TypeNode::Sibling(index)))
            .collect();
        types.union_of(siblings)
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
        let sealed = self.seal(types);
        *self.sealed.borrow_mut() = Some(sealed.clone());
        Some(sealed)
    }

    /// Turn the filled window into interned content and hand back one absolute handle per member,
    /// in declaration order. Implements the per-component identity described in this module's
    /// header.
    fn seal(&self, types: &TypeRegistry) -> SealedGroup {
        let count = self.len();
        // Snapshot the fills; nothing may mutate the window from here on.
        let fills: Vec<RelativeSchema> = self
            .members
            .borrow()
            .iter()
            .map(|m| {
                m.fill
                    .borrow()
                    .clone()
                    .expect("the window seals only once every member is filled")
            })
            .collect();
        let (names, kinds): (Vec<String>, Vec<KKind>) = self
            .members
            .borrow()
            .iter()
            .map(|m| (m.name.clone(), m.kind))
            .unzip();

        // Edges: `member → sibling it references`. A referent must be digested first, so the
        // condensation is processed successor-first — which is exactly Tarjan's emission order.
        let mut edges: Vec<Vec<usize>> = Vec::with_capacity(count);
        for fill in &fills {
            let mut references = Vec::new();
            fill.sibling_references(types, &mut references);
            references.sort_unstable();
            references.dedup();
            edges.push(references);
        }

        // `member index → its finished handle`, filled component by component.
        let mut handles: Vec<Option<KType>> = vec![None; count];
        // `member index → (its component's digest, its position in that component, size)`.
        let mut placement: Vec<Option<(TypeDigest, usize, usize)>> = vec![None; count];

        for component in tarjan_components(&edges) {
            // Canonical presentation order is member-name order; names are unique in a window, so
            // no further refinement is needed to make this deterministic.
            let mut order = component.clone();
            order.sort_by(|a, b| names[*a].cmp(&names[*b]));
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
                        fills[*member]
                            .map_handles(types, &resolve)
                            .into_node_schema()
                    })
                    .collect()
            };
            let component_members: Vec<ComponentMember<'_>> = order
                .iter()
                .zip(presented.iter())
                .map(|(member, schema)| ComponentMember {
                    name: names[*member].as_str(),
                    kind: kinds[*member],
                    schema,
                })
                .collect();
            // A generative window has exactly one member, so its nonce belongs to the one
            // component the loop ever visits.
            let digest = component_digest(self.generative_nonce, &component_members);
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
        for member in 0..count {
            let (scc_digest, index, scc_size) =
                placement[member].expect("Tarjan covers every member");
            let schema = fills[member]
                .map_handles(types, &absolute)
                .into_node_schema();
            let handle = types.intern(TypeNode::SetMember {
                scc_digest,
                index,
                scc_size,
                name: names[member].clone(),
                kind: kinds[member],
                schema,
            });
            debug_assert_eq!(
                handle,
                handles[member].expect("placed"),
                "the interned member node must key at the handle its component derived",
            );
            sealed.push(handle);
        }
        let group = types.intern(TypeNode::Group {
            members: sealed.clone(),
        });
        SealedGroup {
            members: sealed,
            group,
        }
    }

    /// Seal a one-member window in place — the standalone declarators' path, where announcement,
    /// fill and seal all happen at one site. `nonce` makes it a generative mint. The member's own
    /// self-reference is `Sibling(0)`, so a self-recursive standalone type needs no other setup.
    pub fn seal_singleton(
        name: String,
        schema: RelativeSchema,
        nonce: Option<ScopeId>,
        types: &TypeRegistry,
    ) -> KType {
        let kind = schema.kind();
        let window = match nonce {
            Some(nonce) => Self::generative(name, kind, nonce),
            None => Self::new(vec![(name, kind)], None),
        };
        window
            .fill_member(0, schema, types)
            .expect("a one-member window seals on its only fill")
            .members[0]
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
            let members = members
                .into_iter()
                .map(|m| rewrite_siblings(types, m, resolve))
                .collect();
            types.intern_union_flat(members)
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
