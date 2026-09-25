//! The run's type registry: the single owner of every type's content, plus a flat map of relation
//! verdicts.
//!
//! Content lives in `nodes`, a hash map built over the run region the registry is constructed
//! over ([`TypeRegistry::in_region`]): its bucket array and every node's slices are bumped into that
//! region. A [`KType`] handle *is* the digest of its node, so the handle is also its own lookup
//! key, and the digest is already a uniformly distributed hash — the map hashes it with
//! `IdentityHasher`, making a lookup cost about what an array index would. Interning is
//! insert-if-absent, so building the same content twice in a run yields one node and two equal
//! handles. Nothing ever leaves the map and nothing in it carries drop glue, so the region releases
//! the table and every node with it, whole.
//!
//! A node is `Copy`: a read copies the entry out and releases the table borrow before the reader
//! runs, so reads nest and a reader may intern. Beside each node the entry stores two flags
//! computed off its children at intern — whether a free quantifier, and whether any rigid variable,
//! is reachable — so both probes are one table read.
//!
//! Verdicts are a separate table keyed by `(subject digest, candidate digest, relation)`: a fixed
//! run of two-slot buckets laid in the region the first time a verdict is recorded, and never
//! resized, so it strands nothing in a bump that releases nothing before the run ends. A full
//! bucket evicts the slot not touched last. A verdict over a digest pair is a pure function — once
//! computed it never changes — so verdicts are never load-bearing: a forgotten one costs a re-walk
//! of the relation, never a wrong answer. The registry therefore owns nothing on the global heap,
//! and can itself rest in a bump.
//!
//! Every door that sorts, flattens or canonicalizes takes a scratch [`BumpAllocator`] for its
//! transient buffers, and every door computes its digest off the caller's own slices first, so
//! content is bumped into the region only on a miss.
//!
//! See [README.md](README.md) § Storage: one region.

use std::cell::{Cell, RefCell};

use crate::memory::{BumpAllocator, BumpBackedMap, BumpVec, ScopeId, bump_table};
use crate::symbols::{BinderSymbol, IdentityBuildHasher, Symbol, TypeSymbol};

use super::digest::{self, TypeDigest, schema_content_digest};
use super::handle::KType;
use super::kind::KKind;
use super::node::{NodeSchema, TypeNode};
use super::order::{Dropped, is_subtype_of, unsubsumed};
use super::record::Record;
use super::schema::{
    DeclaredGroup, Members, SchemaDraft, SigSchema, canonical_groups, canonical_overloads,
};
use super::shape::{DeferredReturnSurface, DispatchTokenElement};
use super::substitute::substitute_quantified;
use super::walk::Variance;
use super::walk::unary::{LEAF, Step, Visit, children, visit, visit_in};

/// One interned node, and the two probe answers computed off its children when it was interned.
#[derive(Clone, Copy)]
struct Entry<'run> {
    node: TypeNode<'run>,
    /// Whether a `Quantified` position is reachable without crossing a shape's own binder.
    quantified: bool,
    /// Whether any rigid variable — `Quantified` or `AbstractType` — is reachable.
    rigid: bool,
}

impl<'run> Entry<'run> {
    /// `node`'s entry, its flags folded from its children's own entries — one probe per child, since
    /// every child was interned first. A child not in the table reads as `false`, which is exact: the
    /// only such child is the handle of a group member mid-seal, which the seal's rebuilt schemas
    /// name before its own node is interned, and a sealed member is a leaf for both probes.
    fn over(node: TypeNode<'run>, nodes: &NodeTable<'run>) -> Self {
        let (mut quantified, mut rigid) = (false, false);
        children(&node, Step::Leaf, Step::Leaf, &mut |child, _| {
            if let Some(entry) = nodes.get(&child.digest()) {
                quantified |= entry.quantified;
                rigid |= entry.rigid;
            }
        });
        // A binder — a shape, or a function carrying a group — binds its own variables, so
        // nothing under one is free here.
        if node.binds_quantifiers() {
            return Entry {
                node,
                quantified: false,
                rigid,
            };
        }
        match node {
            TypeNode::Quantified { .. } => Entry {
                node,
                quantified: true,
                rigid: true,
            },
            TypeNode::AbstractType { .. } => Entry {
                node,
                quantified,
                rigid: true,
            },
            _ => Entry {
                node,
                quantified,
                rigid,
            },
        }
    }
}

/// The node table: keyed by digest under the identity hasher, bucket array in the run region.
type NodeTable<'run> = BumpBackedMap<'run, TypeDigest, Entry<'run>, IdentityBuildHasher>;

/// Which question a recorded verdict answers. The two never alias — each digest domain is disjoint
/// by construction — but the enum still keys the table explicitly.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Relation {
    /// [`is_subtype_of`](super::order::is_subtype_of), the one order.
    Subtype,
    /// [`sig_subtype`](super::sig_relations::sig_subtype) over the two schemas.
    SigSatisfies,
}

/// Slots in the verdict table. A power of two, two slots to a bucket.
const VERDICT_SLOTS: usize = 1024;
const _: () = assert!(VERDICT_SLOTS.is_power_of_two() && VERDICT_SLOTS >= 2);

/// One slot of the verdict table: a whole key, its verdict, and the bucket's recency bit.
#[derive(Clone, Copy)]
struct VerdictSlot {
    subject: TypeDigest,
    candidate: TypeDigest,
    relation: Relation,
    verdict: bool,
    occupied: bool,
    /// Set on the slot of its bucket touched last; the other slot is the one an insert evicts.
    recent: bool,
}

impl VerdictSlot {
    const EMPTY: Self = Self {
        subject: TypeDigest(0),
        candidate: TypeDigest(0),
        relation: Relation::Subtype,
        verdict: false,
        occupied: false,
        recent: false,
    };

    fn holds(self, subject: TypeDigest, candidate: TypeDigest, relation: Relation) -> bool {
        self.occupied
            && self.subject == subject
            && self.candidate == candidate
            && self.relation == relation
    }
}

/// What interning a binder produced — a shape or a function type: the canonical handle, and how
/// the caller's declaration-order quantifier indices map onto the canonical group.
#[derive(Clone, Copy, Debug)]
pub struct GroupIntern<'s> {
    pub handle: KType,
    /// Declaration index → canonical index, `None` for a variable canonical form dropped. Lives in
    /// the scratch the caller handed the door.
    pub quantifier_map: &'s [Option<usize>],
}

/// The store of type content and relation verdicts. Interior mutability in independent cells: a
/// read of `nodes` copies its entry out and releases the cell before the reader runs, so reads
/// nest freely and a reader may intern, while a verdict slot is copied in and out whole.
pub struct TypeRegistry<'run> {
    /// The run region's bump: every node slice an intern miss keeps is copied in here.
    bump: BumpAllocator<'run>,
    nodes: RefCell<NodeTable<'run>>,
    /// The verdict table: empty until the first verdict is recorded, then `verdict_slots` long for
    /// good.
    verdicts: Cell<&'run [Cell<VerdictSlot>]>,
    verdict_slots: usize,
    #[cfg(test)]
    verdict_evictions: Cell<u64>,
    #[cfg(test)]
    verdict_inserts: Cell<u64>,
}

impl<'run> TypeRegistry<'run> {
    /// A registry whose content lives in the region `bump` allocates into, pre-seeded with the
    /// fixed handles — the leaves, the `OfKind` values, `List<Any>`, `Dict<Any, Any>` and the empty
    /// signature — so the constants those names lower to are dereferenceable in a registry that has
    /// interned nothing else.
    pub fn in_region(bump: BumpAllocator<'run>) -> Self {
        Self::with_verdict_slots(bump, VERDICT_SLOTS)
    }

    /// `test`-only: a registry whose verdict table holds `slots` slots, so a test can fill a
    /// bucket.
    #[cfg(test)]
    pub(super) fn in_region_with_verdict_slots(bump: BumpAllocator<'run>, slots: usize) -> Self {
        assert!(slots.is_power_of_two() && slots >= 2);
        Self::with_verdict_slots(bump, slots)
    }

    fn with_verdict_slots(bump: BumpAllocator<'run>, verdict_slots: usize) -> Self {
        let registry = Self {
            bump,
            nodes: RefCell::new(bump_table(bump)),
            verdicts: Cell::new(&[]),
            verdict_slots,
            #[cfg(test)]
            verdict_evictions: Cell::new(0),
            #[cfg(test)]
            verdict_inserts: Cell::new(0),
        };
        registry.seed_constants();
        registry
    }

    /// Intern every constant node, so a fixed handle always resolves. The region is its own scratch
    /// here: no seed sorts or flattens anything, so nothing is stranded in it.
    fn seed_constants(&self) {
        for leaf in [
            TypeNode::Number,
            TypeNode::Str,
            TypeNode::Bool,
            TypeNode::Null,
            TypeNode::Identifier,
            TypeNode::NameToken,
            TypeNode::TypeNameToken,
            TypeNode::KExpression,
            TypeNode::SigiledTypeExpr,
            TypeNode::RecordType,
            TypeNode::Any,
            TypeNode::AnyValue,
            TypeNode::AnyCode,
            TypeNode::Never,
        ] {
            self.intern(self.bump, leaf);
        }
        for kind in [
            KKind::ProperType,
            KKind::Signature,
            KKind::AnyType,
            KKind::NewType,
            KKind::TypeConstructor,
        ] {
            self.intern(self.bump, TypeNode::OfKind(kind));
        }
        self.list(KType::ANY);
        self.dict(KType::ANY, KType::ANY);
        self.intern_schema(SigSchema::EMPTY);
    }

    /// `test`-only: how many verdicts were recorded, and how many of those evicted another.
    #[cfg(test)]
    pub(super) fn verdict_tally(&self) -> (u64, u64) {
        (self.verdict_inserts.get(), self.verdict_evictions.get())
    }

    // --- Content: interning and node reads ---

    /// Intern `node` and return its handle. Interning the same content twice yields one node and
    /// two equal handles. The generic door: its slices are stored as handed in, so they must
    /// already live at least as long as the region.
    pub(super) fn intern(&self, scratch: BumpAllocator<'_>, node: TypeNode<'run>) -> KType {
        self.intern_digested(digest::node_digest(scratch, &node), || node)
    }

    /// Insert the node `build` produces under `digest` if the digest is not already present — the
    /// one insert-if-absent path every interning door takes.
    ///
    /// One borrow around one probe: a hit — the steady state, since a repeated spelling of a type
    /// names an already-interned node — never calls `build`, so a door that computes its digest off
    /// the caller's slices bumps nothing into the region.
    ///
    /// `build` runs under the write borrow, so unlike a [`with_node`](Self::with_node) closure it
    /// may not read or intern; it only copies the caller's slices into the region.
    fn intern_digested(&self, digest: TypeDigest, build: impl FnOnce() -> TypeNode<'run>) -> KType {
        let mut nodes = self.nodes.borrow_mut();
        if !nodes.contains_key(&digest) {
            let entry = Entry::over(build(), &nodes);
            nodes.insert(digest, entry);
        }
        KType::from_digest(digest)
    }

    /// `items` copied into the region — the one copy an intern miss makes of a caller's slice. An
    /// empty run needs no bytes.
    fn rehome<T: Copy>(&self, items: &[T]) -> &'run [T] {
        if items.is_empty() {
            &[]
        } else {
            self.bump.alloc_slice_copy(items)
        }
    }

    /// Read the content `handle` names and hand back whatever `read` derives.
    ///
    /// The node is copied out of the table and the `RefCell` borrow ends before `read` runs. That is
    /// what lets a reader intern, and what makes reads nest freely. The node's slices live in the
    /// region, so `read` may hand one back as `&'run`.
    ///
    /// A miss is a bug, not a state: a handle is only ever produced by an interning door, and the
    /// table is insert-only.
    pub fn with_node<R>(&self, handle: KType, read: impl FnOnce(&TypeNode<'run>) -> R) -> R {
        read(&self.node(handle))
    }

    /// The content `handle` names, by value.
    pub fn node(&self, handle: KType) -> TypeNode<'run> {
        self.entry(handle).node
    }

    fn entry(&self, handle: KType) -> Entry<'run> {
        let digest = handle.digest();
        match self.nodes.borrow().get(&digest) {
            Some(entry) => *entry,
            None => panic!("type handle 0x{:032x} names no interned node", digest.0),
        }
    }

    /// Whether `handle` names a union. The construction lane's first probe, so it answers from the
    /// node's shape alone and never reads out the member list.
    pub fn is_union(&self, handle: KType) -> bool {
        matches!(
            self.nodes
                .borrow()
                .get(&handle.digest())
                .map(|entry| entry.node),
            Some(TypeNode::Union { .. })
        )
    }

    /// The `union` member named `name`, whatever schema it declares. `name` probes by bare symbol
    /// bits: the token arrives from a reference site with no class attached.
    pub fn union_member_named(&self, union: KType, name: Symbol) -> Option<KType> {
        let nodes = self.nodes.borrow();
        let Some(TypeNode::Union { members }) = nodes.get(&union.digest()).map(|entry| entry.node)
        else {
            return None;
        };
        members.iter().copied().find(|m| {
            matches!(
                nodes.get(&m.digest()).map(|entry| entry.node),
                Some(TypeNode::SetMember {
                    name: member_name, ..
                }) if member_name.symbol() == name
            )
        })
    }

    // --- Composite construction ---
    //
    // The single entry point per composite shape. Each takes child handles and returns the
    // parent's handle, so building a type is bottom-up interning and no site can construct a
    // composite the registry has not seen.

    /// `List<element>`.
    pub fn list(&self, element: KType) -> KType {
        self.intern_digested(digest::list_digest(element.digest()), || TypeNode::List {
            element,
        })
    }

    /// `Dict<key, value>`.
    pub fn dict(&self, key: KType, value: KType) -> KType {
        self.intern_digested(digest::dict_digest(key.digest(), value.digest()), || {
            TypeNode::Dict { key, value }
        })
    }

    /// A structural record type over `fields`, in declaration order with unique names.
    pub fn record(&self, scratch: BumpAllocator<'_>, fields: &[(BinderSymbol, KType)]) -> KType {
        self.intern_digested(digest::record_digest(scratch, fields), || {
            TypeNode::Record {
                fields: Record::over(self.rehome(fields)),
            }
        })
    }

    /// The one door that mints a function type `(params) -> ret`, in **canonical form** where it
    /// binds a `FOR ALL` group.
    ///
    /// Canonicalization is [`shape_type`](Self::shape_type)'s, over the params record and the
    /// return instead of over an element run: see [`canonical_group`](Self::canonical_group). The
    /// census walks the params in **symbol-sorted key order**, because a record's identity is
    /// order-blind and the renumbering must be too; the stored record keeps declaration order for
    /// rendering. An empty group interns straight away and binds nothing.
    pub fn function_type<'s>(
        &self,
        scratch: BumpAllocator<'s>,
        quantifiers: &[TypeSymbol],
        params: &[(BinderSymbol, KType)],
        ret: KType,
    ) -> GroupIntern<'s> {
        if quantifiers.is_empty() {
            return GroupIntern {
                handle: self.intern_function(scratch, &[], &[], params, ret),
                quantifier_map: &[],
            };
        }
        let mut sorted = BumpVec::with_capacity_in(params.len(), scratch);
        sorted.extend(params.iter().copied());
        sorted.sort_by_key(|(name, _)| name.symbol());
        let group = self.canonical_group(
            scratch,
            quantifiers,
            sorted
                .iter()
                .map(|(_, kt)| (*kt, Variance::Contra))
                .chain(std::iter::once((ret, Variance::Co))),
        );
        let mut canonical_params = BumpVec::with_capacity_in(params.len(), scratch);
        canonical_params.extend(params.iter().map(|(name, kt)| {
            (
                *name,
                substitute_quantified(self, scratch, *kt, &group.bindings),
            )
        }));
        let ret = substitute_quantified(self, scratch, ret, &group.bindings);
        debug_assert!(
            group.names.is_empty()
                || self.quantifier_indices_in_range(scratch, ret, group.names.len()),
            "every quantified position names an index of the function's own canonical group",
        );
        GroupIntern {
            handle: self.intern_function(
                scratch,
                &group.names,
                &group.bounds,
                &canonical_params,
                ret,
            ),
            quantifier_map: group.quantifier_map,
        }
    }

    /// Intern a function type whose form is already canonical. Probe-first, as
    /// [`intern_shape`](Self::intern_shape) is, so the params run and the group are copied into
    /// the region only on a genuine miss.
    fn intern_function(
        &self,
        scratch: BumpAllocator<'_>,
        quantifiers: &[TypeSymbol],
        bounds: &[KType],
        params: &[(BinderSymbol, KType)],
        ret: KType,
    ) -> KType {
        let digest = digest::function_digest(scratch, quantifiers.len(), params, ret.digest());
        self.intern_digested(digest, || TypeNode::KFunction {
            quantifiers: self.rehome(quantifiers),
            bounds: self.rehome(bounds),
            params: Record::over(self.rehome(params)),
            ret,
        })
    }

    /// A confined FN return slot whose source return is deferred to per-call elaboration, carried
    /// as the surface it is shadowed by. An expression surface's text is copied into the region on
    /// a miss.
    pub fn deferred_return(&self, surface: DeferredReturnSurface<'_>) -> KType {
        self.intern_digested(digest::deferred_return_digest(surface), || {
            TypeNode::DeferredReturn(match surface {
                DeferredReturnSurface::Type(name) => DeferredReturnSurface::Type(name),
                DeferredReturnSurface::Expression(text) => {
                    DeferredReturnSurface::Expression(self.bump.alloc_str(text))
                }
            })
        })
    }

    /// Application of a higher-kinded type constructor to the parameter-name-keyed `arguments`.
    pub fn constructor_apply(
        &self,
        scratch: BumpAllocator<'_>,
        constructor: KType,
        arguments: &[(BinderSymbol, KType)],
    ) -> KType {
        let digest = digest::constructor_apply_digest(scratch, constructor.digest(), arguments);
        self.intern_digested(digest, || TypeNode::ConstructorApply {
            constructor,
            arguments: Record::over(self.rehome(arguments)),
        })
    }

    /// A named rigid variable — a signature's abstract member, or an opaque ascription's
    /// per-application mint when `nonce` is set. `bound` is what bounds it, [`KType::ANY`]
    /// where the declaration constrains nothing. `param_names` may arrive in any order: identity is
    /// their set, so they are stored symbol-sorted.
    ///
    /// A bound holds no rigid variable of its own — see [`contains_rigid`](Self::contains_rigid).
    pub fn abstract_type(
        &self,
        scratch: BumpAllocator<'_>,
        source: ScopeId,
        name: TypeSymbol,
        param_names: &[TypeSymbol],
        nonce: Option<ScopeId>,
        bound: KType,
    ) -> KType {
        debug_assert!(
            !self.contains_rigid(bound),
            "a rigid variable's bound holds no rigid variable of its own",
        );
        let mut sorted = BumpVec::with_capacity_in(param_names.len(), scratch);
        sorted.extend_from_slice(param_names);
        sorted.sort_unstable();
        sorted.dedup();
        let digest = digest::abstract_type_digest(source, name, &sorted, nonce, bound);
        self.intern_digested(digest, || TypeNode::AbstractType {
            source,
            name,
            param_names: self.rehome(&sorted),
            nonce,
            bound,
        })
    }

    /// The `index`-th rigid variable of the enclosing shape's group, bounded by `bound`.
    ///
    /// A bound holds no rigid variable of its own — see [`contains_rigid`](Self::contains_rigid).
    pub fn quantified(&self, index: usize, bound: KType) -> KType {
        debug_assert!(
            !self.contains_rigid(bound),
            "a rigid variable's bound holds no rigid variable of its own",
        );
        self.intern_digested(digest::quantified_digest(index, bound), || {
            TypeNode::Quantified { index, bound }
        })
    }

    /// A relative sibling reference against an open recursive-group window.
    pub fn sibling(&self, index: usize) -> KType {
        self.intern_digested(digest::sibling_digest(index), || TypeNode::Sibling(index))
    }

    /// One sealed member of a recursive group. Built only by the seal, which derives the handle
    /// from the component digest first and interns the node under it; `schema` is copied into the
    /// region on a miss, so the seal may hand one staged in scratch.
    pub(super) fn set_member(
        &self,
        scc_digest: TypeDigest,
        index: usize,
        scc_size: usize,
        name: TypeSymbol,
        kind: KKind,
        schema: NodeSchema<'_>,
    ) -> KType {
        self.intern_digested(digest::member_ref_digest(scc_digest, index), || {
            TypeNode::SetMember {
                scc_digest,
                index,
                scc_size,
                name,
                kind,
                schema: match schema {
                    NodeSchema::NewType(repr) => NodeSchema::NewType(repr),
                    NodeSchema::TypeConstructor {
                        representation,
                        param_names,
                    } => NodeSchema::TypeConstructor {
                        representation,
                        param_names: self.rehome(param_names),
                    },
                },
            }
        })
    }

    /// A module-signature type over `draft`, in canonical form — the one door a schema enters the
    /// lattice through. It makes each named table a [`Members`], canonicalizes the keyworded and
    /// operator channels, digests the result, and copies it into the region on a miss, so no
    /// interned schema is ever uncanonical.
    pub fn signature(&self, scratch: BumpAllocator<'_>, draft: SchemaDraft<'_>) -> KType {
        let SchemaDraft {
            sig_id,
            abstract_members,
            manifest_members,
            value_slots,
            mut keyworded,
            mut operators,
            ..
        } = draft;
        canonical_overloads(self, scratch, &mut keyworded);
        canonical_groups(&mut operators);
        self.intern_schema(SigSchema {
            sig_id,
            abstract_members: Members::from_table(abstract_members),
            manifest_members: Members::from_table(manifest_members),
            value_slots: Members::from_table(value_slots),
            keyworded: &keyworded,
            operators: &operators,
        })
    }

    /// Intern a schema that is already canonical, wherever its slices live: the seed's door for
    /// [`SigSchema::EMPTY`], and a walk's for a schema it rebuilt member for member. Computes the
    /// schema's content digest once, here, so the node carries it and identity is one compare.
    pub(super) fn intern_schema(&self, schema: SigSchema<'_>) -> KType {
        let schema_digest = schema_content_digest(schema, self);
        self.intern_digested(digest::signature_digest(schema_digest), || {
            TypeNode::Signature {
                schema: SigSchema {
                    sig_id: schema.sig_id,
                    abstract_members: schema.abstract_members.copied_into(self.bump),
                    manifest_members: schema.manifest_members.copied_into(self.bump),
                    value_slots: schema.value_slots.copied_into(self.bump),
                    keyworded: self.rehome(schema.keyworded),
                    operators: self.rehome_groups(schema.operators),
                },
                schema_digest,
            }
        })
    }

    /// An operator channel copied into the region, each record's member run with it.
    fn rehome_groups(&self, groups: &[DeclaredGroup<'_>]) -> &'run [DeclaredGroup<'run>] {
        if groups.is_empty() {
            return &[];
        }
        self.bump
            .alloc_slice_fill_iter(groups.iter().map(|group| DeclaredGroup {
                members: self.rehome(group.members),
                mode: group.mode,
            }))
    }

    // --- Shapes ---

    /// The one door that mints an expression shape, in **canonical form** — see
    /// [`canonical_group`](Self::canonical_group), which is run here over each argument slot in
    /// element order and then the return. Canonical form is what makes the order antisymmetric
    /// once quantified shapes relate by instantiation: two shapes each below the other must be
    /// one handle.
    ///
    /// Argument names never reach here — they are binder-side — and the surviving quantifier names
    /// are render-only, so two shapes alpha-equivalent under a renaming intern to the node
    /// whichever spelling built first. The returned map translates a caller's declaration-order
    /// bindings into the canonical group's order.
    pub fn shape_type<'s>(
        &self,
        scratch: BumpAllocator<'s>,
        quantifiers: &[TypeSymbol],
        elements: &[DispatchTokenElement],
        ret: KType,
    ) -> GroupIntern<'s> {
        if quantifiers.is_empty() {
            return GroupIntern {
                handle: self.intern_shape(&[], &[], elements, ret),
                quantifier_map: &[],
            };
        }
        let group = self.canonical_group(
            scratch,
            quantifiers,
            elements
                .iter()
                .filter_map(|element| match element {
                    DispatchTokenElement::Slot(kt) => Some((*kt, Variance::Contra)),
                    DispatchTokenElement::Keyword(_) => None,
                })
                .chain(std::iter::once((ret, Variance::Co))),
        );
        let mut canonical_elements = BumpVec::with_capacity_in(elements.len(), scratch);
        canonical_elements.extend(elements.iter().map(|element| match element {
            DispatchTokenElement::Slot(kt) => DispatchTokenElement::Slot(substitute_quantified(
                self,
                scratch,
                *kt,
                &group.bindings,
            )),
            keyword => *keyword,
        }));
        let ret = substitute_quantified(self, scratch, ret, &group.bindings);
        debug_assert!(
            group.names.is_empty()
                || self.quantifier_indices_in_range(scratch, ret, group.names.len()),
            "every quantified position names an index of the shape's own canonical group",
        );
        GroupIntern {
            handle: self.intern_shape(&group.names, &group.bounds, &canonical_elements, ret),
            quantifier_map: group.quantifier_map,
        }
    }

    /// Census, survivor selection and renumbering over the `(type, variance)` positions a binder
    /// binds, walked in the order given — the canonical form both interning doors share.
    ///
    /// A variable with no occurrence is dropped; one that occurs exactly once is replaced by its
    /// bound where the occurrence is contravariant and by [`KType::NEVER`] where it is covariant,
    /// since a single occurrence is equivalent to that replacement in every admission and every
    /// subtype question. Survivors are renumbered by first occurrence in the walked order. The
    /// caller substitutes its own positions through [`bindings`](CanonicalGroup::bindings) and
    /// interns the result.
    fn canonical_group<'s>(
        &self,
        scratch: BumpAllocator<'s>,
        quantifiers: &[TypeSymbol],
        positions: impl Iterator<Item = (KType, Variance)>,
    ) -> CanonicalGroup<'s> {
        let arity = quantifiers.len();
        let census = self.quantifier_census(scratch, positions, arity);

        let mut survivors = BumpVec::with_capacity_in(arity, scratch);
        survivors.extend((0..arity).filter(|index| census[*index].occurrences() >= 2));
        survivors.sort_by_key(|index| census[*index].first);
        let mut quantifier_map = BumpVec::with_capacity_in(arity, scratch);
        quantifier_map.resize(arity, None);
        for (canonical, declared) in survivors.iter().enumerate() {
            quantifier_map[*declared] = Some(canonical);
        }

        let mut bindings = BumpVec::with_capacity_in(arity, scratch);
        bindings.extend((0..arity).map(|index| {
            let seen = &census[index];
            match quantifier_map[index] {
                Some(canonical) => self.quantified(canonical, seen.bound),
                // A lone covariant occurrence must hold at every instantiation, which only the
                // bottom does; a lone contravariant one is free for the caller to pick, which is
                // exactly its bound.
                None if seen.covariant > 0 => KType::NEVER,
                None => seen.bound,
            }
        }));

        let mut names = BumpVec::with_capacity_in(survivors.len(), scratch);
        names.extend(survivors.iter().map(|index| quantifiers[*index]));
        let mut bounds = BumpVec::with_capacity_in(survivors.len(), scratch);
        bounds.extend(survivors.iter().map(|index| census[*index].bound));
        CanonicalGroup {
            names,
            bounds,
            bindings,
            quantifier_map: quantifier_map.leak(),
        }
    }

    /// Intern a shape whose form is already canonical.
    ///
    /// Probe-first: every definition mints its callable's shape, and re-running one definition —
    /// a lambda inside a loop body — mints a shape already interned. Taking the digest off the
    /// borrowed run means the run and the quantifier group are copied into the region only on a
    /// genuine miss.
    fn intern_shape(
        &self,
        quantifiers: &[TypeSymbol],
        bounds: &[KType],
        elements: &[DispatchTokenElement],
        ret: KType,
    ) -> KType {
        let handle = digest::shape_digest(quantifiers.len(), elements, ret.digest());
        self.intern_digested(handle, || TypeNode::ExpressionShape {
            quantifiers: self.rehome(quantifiers),
            bounds: self.rehome(bounds),
            elements: self.rehome(elements),
            ret,
        })
    }

    /// Count each variable's free occurrences across the positions a binder binds, under the
    /// polarity each carries, recording its bound and the visit order of its first occurrence. A
    /// nested binder's own group shadows this one, so the census skips it.
    fn quantifier_census<'s>(
        &self,
        scratch: BumpAllocator<'s>,
        positions: impl Iterator<Item = (KType, Variance)>,
        arity: usize,
    ) -> BumpVec<'s, Occurrences> {
        let mut census = BumpVec::with_capacity_in(arity, scratch);
        census.resize(arity, Occurrences::default());
        let mut seen = 0usize;
        for (kt, position) in positions {
            visit_in(
                self,
                scratch,
                kt,
                LEAF,
                position,
                &mut |_, node, context| {
                    if node.binds_quantifiers() {
                        return Visit::Skip;
                    }
                    match *node {
                        TypeNode::Quantified { index, bound } => {
                            if let Some(record) = census.get_mut(index) {
                                if record.first == usize::MAX {
                                    record.first = seen;
                                    record.bound = bound;
                                }
                                match context.variance() {
                                    Variance::Co => record.covariant += 1,
                                    Variance::Contra => record.contravariant += 1,
                                }
                                seen += 1;
                            }
                            Visit::Skip
                        }
                        _ => Visit::Descend,
                    }
                },
            );
        }
        census
    }

    // --- Unions ---

    /// The canonicalizing constructor for a union — the single entry point that builds one.
    ///
    /// Flattens any nested union member into its members, drops [`KType::NEVER`] (the identity
    /// element: it admits nothing, so it widens nothing), deduplicates by handle, then drops every
    /// member that is a subtype of the rest, so [`KType::ANY`] absorbs, a union holding all three
    /// family tops is [`KType::ANY`], and no member lies under the union of the others. One survivor
    /// collapses to that member; none is `Never`.
    pub fn union_of(&self, scratch: BumpAllocator<'_>, members: &[KType]) -> KType {
        let width: usize = members
            .iter()
            .map(|member| match self.node(*member) {
                TypeNode::Union { members: inner } => inner.len(),
                _ => 1,
            })
            .sum();
        let mut flat = BumpVec::with_capacity_in(width, scratch);
        let push_unique = |handle: KType, flat: &mut BumpVec<'_, KType>| {
            if handle != KType::NEVER && !flat.contains(&handle) {
                flat.push(handle);
            }
        };
        for member in members {
            match self.node(*member) {
                TypeNode::Union { members: inner } => {
                    for nested in inner {
                        push_unique(*nested, &mut flat);
                    }
                }
                _ => push_unique(*member, &mut flat),
            }
        }
        if flat.len() > 1 {
            // Subsumption: a member below another contributes nothing the other does not already
            // admit. Mutually ordered members are equal handles, which the dedup above removed, so
            // the surviving set is an antichain and dropping is order-insensitive.
            let keep = unsubsumed(self, scratch, &flat, Dropped::Below);
            let mut keep = keep.iter();
            flat.retain(|_| *keep.next().unwrap_or(&true));
            self.drop_variables_under_the_rest(scratch, &mut flat);
        }
        // The three family tops together hold every type, so their union is `Any` — and must be, or
        // a type variable bounded by `Any` would lie under `Any` but not under the union that equals it.
        if [KType::ANY_VALUE, KType::ANY_TYPE, KType::ANY_CODE]
            .iter()
            .all(|top| flat.contains(top))
        {
            return KType::ANY;
        }
        match flat.len() {
            0 => KType::NEVER,
            1 => flat[0],
            _ => self.intern_union_members(scratch, &flat),
        }
    }

    /// Drop from the antichain `flat` every rigid variable whose bound lies under the union of the
    /// other members — a bound spanning several members, which the pairwise pass cannot see. Only
    /// the non-rigid members are read: a bound is variable-free, so no rigid member holds one up.
    fn drop_variables_under_the_rest(
        &self,
        scratch: BumpAllocator<'_>,
        flat: &mut BumpVec<'_, KType>,
    ) {
        let bounded = |member: KType| {
            self.node(member)
                .rigid_bound()
                .filter(|bound| *bound != KType::ANY)
        };
        if !flat.iter().any(|member| bounded(*member).is_some()) {
            return;
        }
        let mut concrete = BumpVec::with_capacity_in(flat.len(), scratch);
        concrete.extend(
            flat.iter()
                .copied()
                .filter(|member| self.node(*member).rigid_bound().is_none()),
        );
        // With one concrete member the pairwise pass already decided.
        if concrete.len() < 2 {
            return;
        }
        let rest = self.intern_union_members(scratch, &concrete);
        flat.retain(|member| {
            bounded(*member).is_none_or(|bound| !is_subtype_of(self, scratch, bound, rest))
        });
    }

    /// Intern a union from members that are already flat and already an antichain — dedup by handle
    /// and collapse a one-member result, but read no member nodes.
    ///
    /// The seal's door, and the seal's alone. A rewritten sibling handle names a still-uninterned
    /// member of the group being sealed, so [`union_of`](Self::union_of)'s flatten pass would fault
    /// on it. It is sound there because the rename `Sibling(i) ↦ member_i` preserves every
    /// subsumption verdict: both are atoms, below only themselves and `Never`, and distinct indices
    /// name distinct members — so a union canonical before the seal is canonical after it.
    pub(super) fn intern_union_flat(&self, scratch: BumpAllocator<'_>, members: &[KType]) -> KType {
        let mut flat = BumpVec::with_capacity_in(members.len(), scratch);
        for member in members {
            if !flat.contains(member) {
                flat.push(*member);
            }
        }
        match flat.len() {
            0 => KType::NEVER,
            1 => flat[0],
            _ => self.intern_union_members(scratch, &flat),
        }
    }

    /// Intern the `Union` node over the already-canonical `flat`, probing the table before copying
    /// the members in. The `Union` arm of `node_digest` *is* `union_digest` over the node's member
    /// slice, so the digest taken here off `flat` equals the digest the node would key at.
    fn intern_union_members(&self, scratch: BumpAllocator<'_>, flat: &[KType]) -> KType {
        self.intern_digested(digest::union_digest(scratch, flat), || TypeNode::Union {
            members: self.rehome(flat),
        })
    }

    // --- Quantifier probes ---

    /// Whether any `Quantified` position is reachable from `kt` without crossing a shape's own
    /// binder — the probe that lets a slot type with nothing to solve answer the relations in one
    /// step instead of walking under a unifier. Read off the flag interning stored beside the node.
    pub fn contains_quantified(&self, kt: KType) -> bool {
        self.entry(kt).quantified
    }

    /// Whether any rigid variable — `Quantified` or `AbstractType` — is reachable from `kt`. Read
    /// off the flag interning stored beside the node.
    ///
    /// The invariant a **bound** carries: a bound is a variable-free type. That is what keeps the
    /// order's two rigid clauses consistent, since below a rigid variable are only itself and
    /// `Never` while above it is everything above its bound — and a rigid bound would put a
    /// variable in both sets at once. The two doors that mint a rigid variable assert it, so a
    /// caller that reaches for a rigid bound fails a test rather than producing a wrong verdict.
    pub fn contains_rigid(&self, kt: KType) -> bool {
        self.entry(kt).rigid
    }

    /// Whether `kt` reads the `index`-th quantifier of the enclosing shape — what a definition asks
    /// of each name its `FOR ALL` group lists.
    pub fn references_quantifier(
        &self,
        scratch: BumpAllocator<'_>,
        kt: KType,
        index: usize,
    ) -> bool {
        self.contains_quantified(kt)
            && visit(self, scratch, kt, LEAF, &mut |_, node, _| {
                if node.binds_quantifiers() {
                    return Visit::Skip;
                }
                match *node {
                    TypeNode::Quantified { index: found, .. } if found == index => Visit::Stop,
                    TypeNode::Quantified { .. } => Visit::Skip,
                    _ => Visit::Descend,
                }
            })
    }

    /// Whether any of quantifiers `0..arity` occurs free in `kt` at a contravariant position — what
    /// a family's representation may not do, since an application is covariant in its arguments.
    pub fn quantifies_contravariantly(
        &self,
        scratch: BumpAllocator<'_>,
        kt: KType,
        arity: usize,
    ) -> bool {
        self.contains_quantified(kt)
            && self
                .quantifier_census(scratch, std::iter::once((kt, Variance::Co)), arity)
                .iter()
                .any(|occurrences| occurrences.contravariant > 0)
    }

    /// Whether every free `Quantified` position reachable from `kt` names an index below `arity` —
    /// the [`shape_type`](Self::shape_type) well-formedness probe.
    pub(super) fn quantifier_indices_in_range(
        &self,
        scratch: BumpAllocator<'_>,
        kt: KType,
        arity: usize,
    ) -> bool {
        !visit(self, scratch, kt, LEAF, &mut |_, node, _| {
            if node.binds_quantifiers() {
                return Visit::Skip;
            }
            match *node {
                TypeNode::Quantified { index, .. } if index >= arity => Visit::Stop,
                TypeNode::Quantified { .. } => Visit::Skip,
                _ => Visit::Descend,
            }
        })
    }

    // --- Verdicts ---

    /// The first slot of the bucket a key lands in. A digest is already a uniformly distributed
    /// hash, so the two digests' low words fold together — rotated apart, so `(a, b)` and `(b, a)`
    /// land in different buckets — with the relation mixed in. A slot compares the whole key, so a
    /// fold collision costs a slot and never a wrong verdict.
    fn bucket(&self, subject: TypeDigest, candidate: TypeDigest, relation: Relation) -> usize {
        let fold = (subject.0 as u64).rotate_left(32) ^ (candidate.0 as u64) ^ (relation as u64);
        2 * ((fold as usize) & (self.verdict_slots / 2 - 1))
    }

    /// Consult the registry for a recorded verdict. A hit marks its slot the one touched last.
    pub(super) fn verdict(
        &self,
        subject: TypeDigest,
        candidate: TypeDigest,
        relation: Relation,
    ) -> Option<bool> {
        let table = self.verdicts.get();
        if table.is_empty() {
            return None;
        }
        let first = self.bucket(subject, candidate, relation);
        let pair = &table[first..first + 2];
        let hit = pair
            .iter()
            .position(|slot| slot.get().holds(subject, candidate, relation))?;
        let verdict = touch(pair, hit);
        Some(verdict)
    }

    /// Record `verdict` for the key. Negative verdicts are recorded exactly as positive ones. The
    /// table is laid on the first record; after that a full bucket evicts the slot not touched last.
    pub(super) fn record_verdict(
        &self,
        subject: TypeDigest,
        candidate: TypeDigest,
        relation: Relation,
        verdict: bool,
    ) {
        let mut table = self.verdicts.get();
        if table.is_empty() {
            table = self
                .bump
                .alloc_slice_fill_with(self.verdict_slots, |_| Cell::new(VerdictSlot::EMPTY));
            self.verdicts.set(table);
        }
        let first = self.bucket(subject, candidate, relation);
        let pair = &table[first..first + 2];
        let held = pair
            .iter()
            .position(|slot| slot.get().holds(subject, candidate, relation));
        let free = || pair.iter().position(|slot| !slot.get().occupied);
        let slot = held.or_else(free).unwrap_or_else(|| {
            #[cfg(test)]
            self.verdict_evictions.set(self.verdict_evictions.get() + 1);
            usize::from(pair[0].get().recent)
        });
        #[cfg(test)]
        self.verdict_inserts.set(self.verdict_inserts.get() + 1);
        pair[slot].set(VerdictSlot {
            subject,
            candidate,
            relation,
            verdict,
            occupied: true,
            recent: false,
        });
        touch(pair, slot);
    }
}

/// Mark slot `hit` of a bucket the one touched last, and return its verdict.
fn touch(pair: &[Cell<VerdictSlot>], hit: usize) -> bool {
    let (mut this, mut other) = (pair[hit].get(), pair[1 - hit].get());
    this.recent = true;
    other.recent = false;
    pair[hit].set(this);
    pair[1 - hit].set(other);
    this.verdict
}

/// A quantifier group canonicalized over the positions it binds — what both interning doors
/// hand their own positions and names through.
struct CanonicalGroup<'s> {
    /// The surviving quantifiers' names, in canonical index order.
    names: BumpVec<'s, TypeSymbol>,
    /// Each survivor's bound, in the same order.
    bounds: BumpVec<'s, KType>,
    /// What each **declared** variable substitutes to: its canonical `Quantified`, or `Never` /
    /// its bound where canonical form dropped it.
    bindings: BumpVec<'s, KType>,
    /// Declaration index → canonical index, `None` for a variable canonical form dropped.
    quantifier_map: &'s [Option<usize>],
}

/// One variable's census entry while a binder is being canonicalized.
#[derive(Clone, Copy)]
struct Occurrences {
    covariant: usize,
    contravariant: usize,
    /// Visit sequence number of the first occurrence — the renumbering key. `usize::MAX` until the
    /// variable is met at all.
    first: usize,
    bound: KType,
}

impl Occurrences {
    fn occurrences(&self) -> usize {
        self.covariant + self.contravariant
    }
}

impl Default for Occurrences {
    fn default() -> Self {
        Occurrences {
            covariant: 0,
            contravariant: 0,
            first: usize::MAX,
            bound: KType::ANY,
        }
    }
}
