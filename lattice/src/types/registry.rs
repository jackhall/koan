//! The run's type registry: the single owner of every type's content, plus the table of relation
//! verdicts ([`verdicts`](super::verdicts)).
//!
//! Each node is a [`StoredNode`](super::stored::StoredNode) laid alone in the run region the
//! registry is constructed over ([`TypeRegistry::in_region`]), holding its own digest beside its
//! content. `nodes` is a hash table of references to those nodes, its bucket array in the same
//! region. A [`Handle`] *is* the digest of its node, and the digest is already a uniformly
//! distributed hash, so the table hashes a digest by its low word and finds a node by comparing
//! against the digest the node holds — it stores no second copy. Interning is insert-if-absent,
//! so building the same content twice in a run yields one node and two equal handles. Nothing ever
//! leaves the table and nothing in it carries drop glue, so the region releases the table and
//! every node with it, whole.
//!
//! [`TypeRegistry::node`] reads a stored node back as a [`TypeNode`] view. A read takes a
//! reference into the region and releases the table borrow before the reader runs, so reads nest
//! and a reader may intern. A stored node carries four flags computed off its children at intern
//! — whether a free quantifier, whether any rigid variable, whether any variable or scheme outside
//! sealed content, and whether an opaque carrier outside sealed content is reachable — so each
//! probe is one read of the node.
//!
//! Verdicts are a fixed cache laid in the same region, described in [`verdicts`](super::verdicts).
//! The registry therefore owns nothing on the global heap, and can itself rest in a bump.
//!
//! Every door that sorts, flattens or canonicalizes takes a scratch [`BumpAllocator`] for its
//! transient buffers, and every door computes its digest off the caller's own slices first, so
//! content is bumped into the region only on a miss.
//!
//! See [README.md](README.md) § Storage: one region.

use std::cell::RefCell;

use crate::bump::{BumpAllocator, BumpHashTable, BumpVec, bump_hash_table};
use crate::symbols::{BinderSymbol, Symbol, TypeSymbol};

use super::digest::{self, TypeDigest, schema_content_digest};
use super::handle::{DeclaredType, Handle, KType, Parametric, TypeHandle, wrap};
use super::kind::KKind;
use super::node::{ContentKey, Group, NodeSchema, TypeNode};
use super::order::{Dropped, is_subtype_of, unsubsumed};
use super::record::Record;
use super::run::{Elements, Run};
use super::schema::{
    DeclaredGroup, Members, SchemaDraft, SigOrigin, SigSchema, canonical_groups,
    canonical_overloads, member,
};
use super::shape::{
    DeferredReturnSurface, DispatchTokenElement, StoredElement, class_of, written_order,
};
use super::signatures::canonical_applications;
use super::stored::{Content, Flags, Linked, MemberBody, SchemaSlot, StoredNode};
use super::substitute::substitute_quantified;
use super::verdicts::{Relation, VERDICT_SLOTS, VerdictTable};
use super::walk::Variance;
use super::walk::unary::{Visit, children, visit, visit_free_quantified};

/// The node table: a reference to each stored node, bucket array in the run region. A node holds
/// its own digest, so the table holds no second copy of it.
type NodeTable<'run> = BumpHashTable<'run, Linked<'run>>;

/// The table's hash of `digest`: its low word. A digest is already uniformly distributed.
fn slot_hash(digest: TypeDigest) -> u64 {
    digest.0 as u64
}

/// What a scheme door produced: its declared type — a scheme for a non-empty group — and how the
/// caller's declaration-order quantifier indices map onto the interned group's order.
#[derive(Clone, Copy, Debug)]
pub struct GroupIntern<'s> {
    pub handle: DeclaredType<Parametric>,
    /// Declaration index → index in the interned group: a permutation, since the group keeps every
    /// variable. Lives in the scratch the caller handed the door.
    pub quantifier_map: &'s [usize],
}

/// The store of type content and relation verdicts. Interior mutability in independent cells: a
/// read of `nodes` takes a reference to its node and releases the cell before the reader runs, so
/// reads nest freely and a reader may intern, while a verdict slot is copied in and out whole.
pub struct TypeRegistry<'run> {
    /// The run region's bump: every node slice an intern miss keeps is copied in here.
    bump: BumpAllocator<'run>,
    nodes: RefCell<NodeTable<'run>>,
    verdicts: VerdictTable<'run>,
}

impl<'run> TypeRegistry<'run> {
    /// A registry whose content lives in the region `bump` allocates into, pre-seeded with the
    /// fixed handles — the leaves, the `OfKind` values, `List<Any>`, `Dict<Any, Any>` and the empty
    /// signature — so the constants those names lower to are dereferenceable in a registry that has
    /// interned nothing else.
    pub fn in_region(bump: BumpAllocator<'run>) -> Self {
        Self::with_verdict_slots(bump, VERDICT_SLOTS)
    }

    /// How many nodes the registry has interned. Tests read it — koan's among them, which is why it
    /// is not `cfg(test)`: that does not reach a dependency.
    pub fn node_count(&self) -> usize {
        self.nodes.borrow().len()
    }

    /// Every interned type's digest, in no particular order. `each` runs while the table is read, so
    /// it may not intern. Tests read it, koan's among them, as they read `node_count`.
    pub fn each_digest(&self, mut each: impl FnMut(TypeDigest)) {
        for node in self.nodes.borrow().iter() {
            each(node.digest());
        }
    }

    /// `test`-only: a registry whose verdict table holds `slots` slots, so a test can fill a
    /// bucket.
    #[cfg(test)]
    pub(super) fn in_region_with_verdict_slots(bump: BumpAllocator<'run>, slots: usize) -> Self {
        Self::with_verdict_slots(bump, slots)
    }

    fn with_verdict_slots(bump: BumpAllocator<'run>, verdict_slots: usize) -> Self {
        let registry = Self {
            bump,
            nodes: RefCell::new(bump_hash_table(bump)),
            verdicts: VerdictTable::with_slots(verdict_slots),
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
            TypeNode::Symbol,
            TypeNode::TypeNameToken,
            TypeNode::Expression,
            TypeNode::SigiledTypeExpr,
            TypeNode::RecordType,
            TypeNode::Literal,
            TypeNode::Block,
            TypeNode::Declaration,
            TypeNode::Binder,
            TypeNode::Name,
            TypeNode::Keyword,
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
        self.list(Handle::ANY);
        self.dict(Handle::ANY, Handle::ANY);
        self.intern_schema(SigSchema::EMPTY);
        let type_code = self.union_of(
            self.bump,
            &[
                Handle::TYPE_NAME_TOKEN,
                Handle::SIGILED_TYPE_EXPR,
                Handle::RECORD_TYPE,
            ],
        );
        self.list(Handle::NAME);
        self.list(Handle::EXPRESSION);
        self.list(Handle::DECLARATION);
        self.dict(Handle::NAME, Handle::BLOCK);
        self.dict(Handle::NAME, type_code);
        self.dict(type_code, Handle::BLOCK);
        self.union_of(
            self.bump,
            &[Handle::LIST_OF_NAME, Handle::DICT_NAME_TYPE_CODE],
        );
        self.record::<Handle>(self.bump, &[]);
    }

    /// `test`-only: how many verdicts were recorded, and how many of those evicted another.
    #[cfg(test)]
    pub(super) fn verdict_tally(&self) -> (u64, u64) {
        self.verdicts.tally()
    }

    // --- Content: interning and node reads ---

    /// Intern `node` and return its handle. Interning the same content twice yields one node and
    /// two equal handles. The generic door: its slices are stored as handed in, so they must
    /// already live at least as long as the region.
    pub(super) fn intern(&self, scratch: BumpAllocator<'_>, node: TypeNode<'run>) -> Handle {
        self.intern_digested(digest::node_digest(scratch, &node), || node)
    }

    /// Insert the node `build` produces under `digest` if the digest is not already present — the
    /// one insert-if-absent path every interning door takes.
    ///
    /// A hit — the steady state, since a repeated spelling of a type names an already-interned
    /// node — never calls `build`, so a door that computes its digest off the caller's slices bumps
    /// nothing into the region. On a miss the view `build` returns has its flags folded from its
    /// children and is laid as a stored node.
    ///
    /// No table borrow is held across `build`, but `build` only copies the caller's slices into the
    /// region and never interns.
    fn intern_digested(
        &self,
        digest: TypeDigest,
        build: impl FnOnce() -> TypeNode<'run>,
    ) -> Handle {
        self.intern_laid(digest, || {
            let node = build();
            let folded = self.flags_over(&node);
            let content = Content::lay(node, folded, self.bump);
            debug_assert_eq!(
                content.flags(),
                content.flags() | folded,
                "a node's flags hold its children's",
            );
            content
        })
    }

    /// The insert-if-absent core under every intern: on a miss, `lay` builds the stored content,
    /// and the node is laid alone in the region and entered in the table under `digest`.
    fn intern_laid(&self, digest: TypeDigest, lay: impl FnOnce() -> Content<'run>) -> Handle {
        if self.stored(digest).is_none() {
            let content = lay();
            debug_assert!(self.stored(digest).is_none(), "`lay` interns nothing");
            let node = self.bump.alloc(StoredNode::new(digest, content));
            self.nodes
                .borrow_mut()
                .insert_unique(slot_hash(digest), node, |node| slot_hash(node.digest()));
        }
        Handle::from_digest(digest)
    }

    /// `node`'s flags folded from its children's own — one probe per child, since every child was
    /// interned first. A child not in the table reads as no flags, which is exact: the only such
    /// child is the handle of a group member mid-seal, which the seal's rebuilt schemas name before
    /// its own node is interned, and a sealed member is a leaf for every probe. A variable's or a
    /// carrier's own flags are [`Content::flags`]'s constants.
    fn flags_over(&self, node: &TypeNode<'run>) -> Flags {
        let mut folded = Flags::NONE;
        children(node, &mut |child, _| {
            if let Some(stored) = self.stored(child.digest()) {
                folded = folded | stored.flags();
            }
        });
        // A scheme binds its own variables, so nothing under one is free here, and the scheme
        // itself is a quantified callable's type.
        if node.binds_quantifiers() {
            folded.without(Flags::QUANTIFIED) | Flags::PARAMETRIC
        } else {
            folded
        }
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

    /// A caller's field run copied into the region raw.
    fn rehome_fields<H: TypeHandle>(
        &self,
        fields: &[(BinderSymbol, H)],
    ) -> &'run [(BinderSymbol, Handle)] {
        if fields.is_empty() {
            &[]
        } else {
            self.bump
                .alloc_slice_fill_iter(fields.iter().map(|(name, kt)| (*name, kt.raw())))
        }
    }

    /// The content `handle` names, by value, its children read as `handle`'s children: a concrete
    /// type's are concrete, a parametric type's parametric.
    ///
    /// The node lives in the region, and the `RefCell` borrow ends before the caller reads it. That
    /// is what lets a reader intern, and what makes reads nest freely. The node's slices live in
    /// the region too, so a reader may hand one back as `&'run`.
    ///
    /// A miss is a bug, not a state: a handle is only ever produced by an interning door, and the
    /// table is insert-only.
    pub fn node<H: TypeHandle>(&self, handle: H) -> TypeNode<'run, H::Child> {
        self.enter(handle.raw()).view()
    }

    /// `handle`'s own quantifier group and the type under it: a scheme's group and body, or the
    /// empty group over `handle` itself. How the lattice's raw-handle relations read a callable
    /// whose handle may name a scheme.
    pub(super) fn split(&self, handle: Handle) -> (Group<'run>, Handle) {
        match self.node(handle) {
            TypeNode::Scheme { group, body } => (*group, body),
            _ => (Group::EMPTY, handle),
        }
    }

    /// What `handle` is declared as: a [`Scheme`](super::handle::Scheme) where it names a scheme node, and a
    /// [`Parametric`] otherwise.
    pub(super) fn declared(&self, handle: Handle) -> DeclaredType<Parametric> {
        if self.enter(handle).view::<Handle>().binds_quantifiers() {
            DeclaredType::Scheme(wrap(handle))
        } else {
            DeclaredType::Type(wrap(handle))
        }
    }

    /// The node stored under `handle`, by reference. A miss panics, as [`node`](Self::node)'s does.
    pub(super) fn enter(&self, handle: Handle) -> Linked<'run> {
        let digest = handle.digest();
        match self.stored(digest) {
            Some(stored) => stored,
            None => panic!("type handle 0x{:032x} names no interned node", digest.0),
        }
    }

    /// The node stored under `digest`, or `None`: found by hashing the digest and comparing
    /// against the digest each candidate node holds. The table borrow ends before the caller reads
    /// the node.
    fn stored(&self, digest: TypeDigest) -> Option<Linked<'run>> {
        self.nodes
            .borrow()
            .find(slot_hash(digest), |node| node.digest() == digest)
            .copied()
    }

    /// Whether `handle` names a union. The construction lane's first probe, so it answers from the
    /// node's shape alone and never reads out the member list.
    pub fn is_union<H: TypeHandle>(&self, handle: H) -> bool {
        matches!(
            self.stored(handle.raw().digest()).map(StoredNode::content),
            Some(Content::Union { .. })
        )
    }

    /// The `union` member named `name`, whatever schema it declares. `name` probes by bare symbol
    /// bits: the token arrives from a reference site with no class attached.
    pub fn union_member_named(&self, union: KType, name: Symbol) -> Option<KType> {
        let Some(Content::Union { members, .. }) =
            self.stored(union.digest()).map(StoredNode::content)
        else {
            return None;
        };
        members
            .iter()
            .copied()
            .find(|m| {
                matches!(
                    self.stored(m.digest()).map(StoredNode::content),
                    Some(Content::SetMember {
                        name: member_name, ..
                    }) if member_name.symbol() == name
                )
            })
            .map(wrap)
    }

    // --- Composite construction ---
    //
    // The single entry point per composite shape. Each takes child handles and returns the
    // parent's handle, so building a type is bottom-up interning and no site can construct a
    // composite the registry has not seen.
    //
    // A door that builds a type from child types is generic over the handle its children are: a
    // `KType` out of `KType` children, a `Parametric` out of parametric ones. A child holds no
    // binder of the door's own, so the composite holds exactly the variables its children do.

    /// `List<element>`.
    ///
    /// A list of a concrete element is concrete, and of a parametric one parametric — a
    /// parametric element yields no `KType`:
    ///
    /// ```compile_fail,E0308
    /// use lattice::bump::Bump;
    /// use lattice::symbols::{SymbolInterner, TypeSymbol};
    /// use lattice::types::{KType, TypeRegistry};
    ///
    /// let region = Bump::new();
    /// let types = TypeRegistry::in_region(&region);
    /// let symbols = SymbolInterner::new();
    /// let name = TypeSymbol::declared("Elt", &symbols).expect("a Type token");
    /// let variable = types.lexical(0, name, KType::ANY);
    /// let _list: KType = types.list(variable);
    /// ```
    pub fn list<H: TypeHandle>(&self, element: H) -> H {
        let element = element.raw();
        wrap(
            self.intern_digested(digest::list_digest(element.digest()), || TypeNode::List {
                element,
            }),
        )
    }

    /// `Dict<key, value>`.
    pub fn dict<H: TypeHandle>(&self, key: H, value: H) -> H {
        let (key, value) = (key.raw(), value.raw());
        wrap(
            self.intern_digested(digest::dict_digest(key.digest(), value.digest()), || {
                TypeNode::Dict { key, value }
            }),
        )
    }

    /// A structural record type over `fields`, in declaration order with unique names.
    pub fn record<H: TypeHandle>(
        &self,
        scratch: BumpAllocator<'_>,
        fields: &[(BinderSymbol, H)],
    ) -> H {
        wrap(
            self.intern_digested(digest::record_digest(scratch, fields), || {
                TypeNode::Record {
                    fields: Record::over(self.rehome_fields(fields)),
                }
            }),
        )
    }

    /// The function type `(params) -> ret`, binding no group: a variable inside reads the enclosing
    /// scheme's group.
    pub fn function_type<H: TypeHandle>(
        &self,
        scratch: BumpAllocator<'_>,
        params: &[(BinderSymbol, H)],
        ret: H,
    ) -> H {
        wrap(self.intern_function(scratch, params, ret.raw()))
    }

    /// The one door that mints a quantified function type, its `FOR ALL` group (`quantifiers` and
    /// their `bounds`, in declaration order) put in group order: it interns the plain function type,
    /// then binds the group over it ([`bind_group`](Self::bind_group)). A [`Scheme`](super::handle::Scheme), or the plain
    /// function type where the group is empty.
    pub fn function_scheme<'s>(
        &self,
        scratch: BumpAllocator<'s>,
        quantifiers: &[TypeSymbol],
        bounds: &[KType],
        params: &[(BinderSymbol, Parametric)],
        ret: Parametric,
    ) -> GroupIntern<'s> {
        let body = self.function_type(scratch, params, ret);
        self.scheme(scratch, quantifiers, bounds, body.raw())
    }

    /// Intern a function type. Probe-first, as [`intern_shape`](Self::intern_shape) is, so the
    /// params run is copied into the region only on a genuine miss.
    fn intern_function<H: TypeHandle>(
        &self,
        scratch: BumpAllocator<'_>,
        params: &[(BinderSymbol, H)],
        ret: Handle,
    ) -> Handle {
        let digest = digest::function_digest(scratch, params, ret.digest());
        self.intern_digested(digest, || TypeNode::KFunction {
            params: Record::over(self.rehome_fields(params)),
            ret,
        })
    }

    /// A confined FN return slot whose source return is deferred to per-call elaboration, carried
    /// as the surface it is shadowed by. An expression surface's text is copied into the region on
    /// a miss.
    pub fn deferred_return(&self, surface: DeferredReturnSurface<'_>) -> KType {
        wrap(
            self.intern_digested(digest::deferred_return_digest(surface), || {
                TypeNode::DeferredReturn(match surface {
                    DeferredReturnSurface::Type(name) => DeferredReturnSurface::Type(name),
                    DeferredReturnSurface::Expression(text) => {
                        DeferredReturnSurface::Expression(self.bump.alloc_str(text))
                    }
                })
            }),
        )
    }

    /// Application of a higher-kinded type constructor to the parameter-name-keyed `arguments`.
    pub fn constructor_apply<H: TypeHandle>(
        &self,
        scratch: BumpAllocator<'_>,
        constructor: KType,
        arguments: &[(BinderSymbol, H)],
    ) -> H {
        let digest = digest::constructor_apply_digest(scratch, constructor.digest(), arguments);
        wrap(self.intern_digested(digest, || TypeNode::ConstructorApply {
            constructor: constructor.raw(),
            arguments: Record::over(self.rehome_fields(arguments)),
        }))
    }

    /// A signature's head parameter: a named variable its members read and `WITH` pins, bounded by
    /// `bound` — [`KType::ANY`] where the declaration constrains nothing.
    pub fn head_parameter(&self, name: TypeSymbol, bound: KType) -> Parametric {
        wrap(self.parameter(name, bound))
    }

    /// The carrier an opaque view hides the head parameter `name` behind, keyed on `key`, the
    /// content the view hides: two views of equal content share it, and two of different content
    /// never unify. A value carries it and dispatches on it, so it is concrete, and the order reads
    /// it as an atom under `Any` alone. It records `met`, the bound its source met, for a
    /// signature's fit alone.
    pub fn carrier(&self, name: TypeSymbol, met: KType, key: ContentKey) -> KType {
        self.assert_bound(met);
        wrap(
            self.intern_digested(digest::carrier_digest(name, key, met), || {
                TypeNode::Carrier { name, key, met }
            }),
        )
    }

    /// Whether `kt` is an opaque view's carrier.
    pub fn is_carrier(&self, kt: KType) -> bool {
        matches!(self.node(kt.raw()), TypeNode::Carrier { .. })
    }

    /// A head parameter named `name`, bounded by `bound`.
    pub(super) fn parameter(&self, name: TypeSymbol, bound: KType) -> Handle {
        self.assert_bound(bound);
        self.intern_digested(digest::parameter_digest(name, bound), || {
            TypeNode::Parameter { name, bound }
        })
    }

    /// A bound is concrete by its type. An opaque carrier is concrete too, but a bound holds none:
    /// the elaborator refuses one there, and this checks that it did.
    fn assert_bound(&self, bound: KType) {
        debug_assert!(
            !self.contains_carrier(bound.raw()),
            "a bound holds no opaque carrier",
        );
    }

    /// A code kind needing `names` where its code is built. `kind` is a code kind below `Code`;
    /// `names` may arrive in any order and repeat, since identity is their set, and needing none
    /// is the bare `kind` itself.
    pub fn code_needing(
        &self,
        scratch: BumpAllocator<'_>,
        kind: KType,
        names: &[BinderSymbol],
    ) -> KType {
        debug_assert!(
            kind.code_parent().is_some(),
            "a code kind needing names is a kind below `Code`",
        );
        if names.is_empty() {
            return kind;
        }
        let mut sorted = BumpVec::with_capacity_in(names.len(), scratch);
        sorted.extend_from_slice(names);
        sorted.sort_unstable();
        sorted.dedup();
        let digest = digest::code_needing_digest(kind, &sorted);
        wrap(self.intern_digested(digest, || TypeNode::CodeNeeding {
            kind,
            names: self.rehome(&sorted),
        }))
    }

    /// The `index`-th rigid variable of the enclosing scheme's group, bounded by `bound`.
    pub fn quantified(&self, index: usize, bound: KType) -> Parametric {
        self.assert_bound(bound);
        wrap(
            self.intern_digested(digest::quantified_digest(index, bound), || {
                TypeNode::Quantified { index, bound }
            }),
        )
    }

    /// The lexical variable at `level`, named `name`, bounded by `bound` and above `Never`.
    pub fn lexical(&self, level: usize, name: TypeSymbol, bound: KType) -> Parametric {
        self.intern_lexical(level, name, KType::NEVER, bound)
    }

    /// A lexical variable between `lower` and `bound`. A `lower` other than `Never` lies strictly
    /// under `bound` — a converged pair is its point, never a variable.
    pub fn lexical_between(
        &self,
        scratch: BumpAllocator<'_>,
        level: usize,
        name: TypeSymbol,
        lower: KType,
        bound: KType,
    ) -> Parametric {
        self.assert_bound(lower);
        debug_assert!(
            lower == KType::NEVER
                || (lower != bound && is_subtype_of(self, scratch, lower.raw(), bound.raw())),
            "a lexical variable's lower end lies strictly under its bound",
        );
        self.intern_lexical(level, name, lower, bound)
    }

    fn intern_lexical(
        &self,
        level: usize,
        name: TypeSymbol,
        lower: KType,
        bound: KType,
    ) -> Parametric {
        self.assert_bound(bound);
        wrap(
            self.intern_digested(digest::lexical_digest(level, name, lower, bound), || {
                TypeNode::Lexical {
                    level,
                    name,
                    lower,
                    bound,
                }
            }),
        )
    }

    /// A relative sibling reference against an open recursive-group window.
    pub fn sibling(&self, index: usize) -> KType {
        wrap(self.intern_digested(digest::sibling_digest(index), || TypeNode::Sibling(index)))
    }

    /// One sealed member of a recursive group. Built only by the seal, which derives the handle
    /// from the component digest first and interns the node under it; `schema` is copied into the
    /// region on a miss, so the seal may hand one staged in scratch.
    pub(super) fn set_member(
        &self,
        scc_digest: TypeDigest,
        index: usize,
        name: TypeSymbol,
        kind: KKind,
        schema: NodeSchema<'_>,
    ) -> Handle {
        self.intern_laid(digest::member_ref_digest(scc_digest, index), || {
            let schema = match schema {
                NodeSchema::NewType(repr) => NodeSchema::NewType(repr),
                NodeSchema::TypeConstructor {
                    representation,
                    param_names,
                } => NodeSchema::TypeConstructor {
                    representation,
                    param_names: self.rehome(param_names),
                },
            };
            Content::SetMember {
                kind,
                index: u32::try_from(index).expect("a component has fewer than 2^32 members"),
                body: self.bump.alloc(MemberBody {
                    scc_digest,
                    schema: SchemaSlot::filled(schema),
                }),
                name,
            }
        })
    }

    /// A module-signature type over `draft`, in canonical form — the one door a schema enters the
    /// lattice through. It makes each named table a [`Members`], canonicalizes the keyworded and
    /// operator channels, digests the result, and copies it into the region on a miss, so no
    /// interned schema is ever uncanonical. A schema with no parameter and no member is the empty
    /// signature whatever its origin, so `SIG E = #[]` and `Module` are one handle.
    pub fn signature(&self, scratch: BumpAllocator<'_>, draft: SchemaDraft<'_>) -> KType {
        let SchemaDraft {
            origin,
            parameters,
            manifest_members,
            value_slots,
            mut keyworded,
            mut operators,
            ..
        } = draft;
        canonical_overloads(self, scratch, &mut keyworded);
        canonical_groups(&mut operators);
        let schema = SigSchema {
            origin,
            parameters: Members::from_table(parameters),
            manifest_members: Members::from_table(manifest_members),
            value_slots: Members::from_table(value_slots),
            keyworded: &keyworded,
            operators: &operators,
        };
        if schema.is_empty() {
            return KType::EMPTY_SIGNATURE;
        }
        wrap(self.intern_schema(schema))
    }

    /// Intern a schema that is already canonical, wherever its slices live: the seed's door for
    /// [`SigSchema::EMPTY`]. Computes the schema's content digest once, here, so the node carries
    /// it and identity is one compare.
    pub(super) fn intern_schema(&self, schema: SigSchema<'_>) -> Handle {
        let schema_digest = schema_content_digest(schema);
        self.intern_digested(digest::signature_digest(schema_digest), || {
            TypeNode::Signature {
                schema: SigSchema {
                    origin: schema.origin,
                    parameters: schema.parameters.copied_into(self.bump),
                    manifest_members: schema.manifest_members.copied_into(self.bump),
                    value_slots: schema.value_slots.copied_into(self.bump),
                    keyworded: self.rehome(schema.keyworded),
                    operators: self.rehome_groups(schema.operators),
                },
                schema_digest,
            }
        })
    }

    /// `signature` with `pins` fixed — the application `Stack WITH {Elt = Number}` spells —
    /// or `signature` itself for no pins. `signature` is a declared signature, and each pin is
    /// keyed by one of its parameters.
    pub fn signature_apply<H: TypeHandle>(
        &self,
        scratch: BumpAllocator<'_>,
        signature: KType,
        pins: &[(BinderSymbol, H)],
    ) -> H {
        if pins.is_empty() {
            return wrap(signature.raw());
        }
        debug_assert!(
            matches!(
                self.node(signature),
                TypeNode::Signature { schema, .. }
                    if schema.origin == SigOrigin::Declared
                        && pins.iter().all(|(name, _)| matches!(
                            name,
                            BinderSymbol::Type(name) if member(schema.parameters, *name).is_some()
                        ))
            ),
            "an application pins a declared signature's own parameters",
        );
        let digest = digest::signature_apply_digest(scratch, signature.digest(), pins);
        wrap(self.intern_digested(digest, || TypeNode::SignatureApply {
            signature,
            pins: Record::over(self.rehome_fields(pins)),
        }))
    }

    /// The meet of the signature types `members` — the set of every application they hold, less
    /// each application lying above another ([`signatures`](super::signatures)). The empty set is
    /// `Module`, and a set of one is that application's own handle.
    pub fn signature_meet<H: TypeHandle>(&self, scratch: BumpAllocator<'_>, members: &[H]) -> H {
        let mut raw = BumpVec::with_capacity_in(members.len(), scratch);
        raw.extend(members.iter().map(|member| member.raw()));
        let set = canonical_applications(self, scratch, &raw);
        wrap(match set[..] {
            [] => Handle::EMPTY_SIGNATURE,
            [only] => only,
            _ => self.intern_digested(digest::signature_meet_digest(&set), || {
                TypeNode::SignatureMeet {
                    members: Run::over(self.rehome(&set)),
                }
            }),
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

    /// The expression shape over `elements` returning `ret`, binding no group: a variable inside
    /// reads the enclosing scheme's group.
    ///
    /// `classes` is each slot's dense priority class, as [`dense_classes`](super::shape::dense_classes)
    /// normalizes a written ranking; empty for written order. Written order spelled out digests
    /// as the empty ranking, so the two spellings are one handle. The node stores each slot's
    /// class in its element, written order spelled out.
    pub fn shape_type<H: TypeHandle>(
        &self,
        elements: &[DispatchTokenElement<H>],
        classes: &[u16],
        ret: H,
    ) -> H {
        debug_assert!(
            classes.is_empty()
                || classes.len()
                    == elements
                        .iter()
                        .filter(|e| matches!(e, DispatchTokenElement::Slot(_)))
                        .count(),
            "a ranking names one class per slot",
        );
        let classes = if written_order(classes) { &[] } else { classes };
        wrap(self.intern_shape(elements, classes, ret.raw()))
    }

    /// The one door that mints a quantified expression shape, its `FOR ALL` group (`quantifiers`
    /// and their `bounds`, in declaration order) put in group order: it interns the plain shape,
    /// then binds the group over it ([`bind_group`](Self::bind_group)). A [`Scheme`](super::handle::Scheme), or the plain
    /// shape where the group is empty.
    pub fn shape_scheme<'s>(
        &self,
        scratch: BumpAllocator<'s>,
        quantifiers: &[TypeSymbol],
        bounds: &[KType],
        elements: &[DispatchTokenElement<Parametric>],
        classes: &[u16],
        ret: Parametric,
    ) -> GroupIntern<'s> {
        let body = self.shape_type(elements, classes, ret);
        self.scheme(scratch, quantifiers, bounds, body.raw())
    }

    /// The declared type both scheme doors answer: [`bind_group`](Self::bind_group) over `body`.
    fn scheme<'s>(
        &self,
        scratch: BumpAllocator<'s>,
        quantifiers: &[TypeSymbol],
        bounds: &[KType],
        body: Handle,
    ) -> GroupIntern<'s> {
        let (handle, quantifier_map) = self.bind_group(scratch, quantifiers, bounds, body);
        GroupIntern {
            handle: self.declared(handle),
            quantifier_map,
        }
    }

    /// `body`, a function type or a shape whose free `Quantified` read `quantifiers` by declaration
    /// index, bound under that group put in group order ([`group_order`](Self::group_order)): a
    /// `Scheme` node, or `body` itself for an empty group. The two scheme doors' shared core, and
    /// how a rebuilt scheme is put back together. The body is substituted into group order, and the
    /// group is copied into the region only on a miss. The returned map takes a declaration index
    /// to a group index.
    ///
    /// Argument names never reach here — they are binder-side — and the quantifier names are
    /// render-only, so two schemes alpha-equivalent under a renaming intern to the node whichever
    /// spelling built first.
    pub(super) fn bind_group<'s>(
        &self,
        scratch: BumpAllocator<'s>,
        quantifiers: &[TypeSymbol],
        bounds: &[KType],
        body: Handle,
    ) -> (Handle, &'s [usize]) {
        debug_assert_eq!(quantifiers.len(), bounds.len(), "one bound per quantifier");
        if quantifiers.is_empty() {
            return (body, &[]);
        }
        let positions = self.binder_positions(scratch, body);
        let group = self.group_order(scratch, quantifiers, bounds, positions.iter().copied());
        let body = substitute_quantified(self, scratch, body, &group.bindings);
        debug_assert!(
            self.quantifier_indices_in_range(scratch, body, group.names.len()),
            "every quantified position names an index of the scheme's own group",
        );
        let digest = digest::scheme_digest(&group.bounds, body.digest());
        let handle = self.intern_digested(digest, || TypeNode::Scheme {
            group: self.bump.alloc(Group {
                names: self.rehome(&group.names),
                bounds: self.rehome(&group.bounds),
            }),
            body,
        });
        (handle, group.quantifier_map)
    }

    /// The positions a scheme's body binds, each with its polarity, in the order its group is
    /// numbered over: a shape's slots in element order, or a function type's parameters in
    /// symbol-sorted key order (a record's identity is order-blind, so the numbering must be too),
    /// then the return.
    fn binder_positions<'s>(
        &self,
        scratch: BumpAllocator<'s>,
        body: Handle,
    ) -> BumpVec<'s, (Handle, Variance)> {
        let mut positions = BumpVec::new_in(scratch);
        let ret = match self.node(body) {
            TypeNode::KFunction { params, ret } => {
                let mut sorted = BumpVec::with_capacity_in(params.len(), scratch);
                sorted.extend(params.iter());
                sorted.sort_by_key(|(name, _)| name.symbol());
                positions.extend(sorted.iter().map(|(_, kt)| (*kt, Variance::Contra)));
                ret
            }
            TypeNode::ExpressionShape { elements, ret, .. } => {
                positions.extend(elements.iter().filter_map(|element| match element {
                    DispatchTokenElement::Slot(kt) => Some((kt, Variance::Contra)),
                    DispatchTokenElement::Keyword(_) => None,
                }));
                ret
            }
            _ => unreachable!("a scheme's body is a function type or a shape"),
        };
        positions.push((ret, Variance::Co));
        positions
    }

    /// Number a scheme's variables over the `(type, variance)` positions it binds, walked in the
    /// order given — the group order the scheme doors share.
    ///
    /// The variables some position names come first, by first occurrence in the walked order; then
    /// every variable no position names, in declaration order. No variable is dropped, so a call
    /// solves each one, and two alpha-variants intern to one handle. The caller substitutes its own
    /// positions through [`bindings`](GroupOrder::bindings) and interns the result —
    /// [`bind_group`](Self::bind_group) is that caller.
    fn group_order<'s>(
        &self,
        scratch: BumpAllocator<'s>,
        quantifiers: &[TypeSymbol],
        bounds: &[KType],
        positions: impl Iterator<Item = (Handle, Variance)>,
    ) -> GroupOrder<'s> {
        let arity = quantifiers.len();
        let census = self.quantifier_census(scratch, positions, arity);

        let mut order = BumpVec::with_capacity_in(arity, scratch);
        order.extend(0..arity);
        // A stable sort, so the variables no position names (`first` is `usize::MAX`) keep their
        // declaration order behind the rest.
        order.sort_by_key(|index| census[*index].first);
        let mut quantifier_map = BumpVec::with_capacity_in(arity, scratch);
        quantifier_map.resize(arity, 0);
        for (interned, declared) in order.iter().enumerate() {
            quantifier_map[*declared] = interned;
        }

        let mut bindings = BumpVec::with_capacity_in(arity, scratch);
        bindings.extend(
            (0..arity).map(|index| self.quantified(quantifier_map[index], bounds[index]).raw()),
        );
        let mut names = BumpVec::with_capacity_in(arity, scratch);
        names.extend(order.iter().map(|index| quantifiers[*index]));
        let mut ordered_bounds = BumpVec::with_capacity_in(arity, scratch);
        ordered_bounds.extend(order.iter().map(|index| bounds[*index]));
        GroupOrder {
            names,
            bounds: ordered_bounds,
            bindings,
            quantifier_map: quantifier_map.leak(),
        }
    }

    /// Intern a shape.
    ///
    /// Probe-first: every definition mints its callable's shape, and re-running one definition —
    /// a lambda inside a loop body — mints a shape already interned. Taking the digest off the
    /// borrowed run means the run is copied into the region only on a genuine miss.
    fn intern_shape<H: TypeHandle>(
        &self,
        elements: &[DispatchTokenElement<H>],
        classes: &[u16],
        ret: Handle,
    ) -> Handle {
        let handle = digest::shape_digest(
            elements.iter().copied(),
            classes.iter().copied(),
            ret.digest(),
        );
        self.intern_digested(handle, || {
            // Each slot's class rides in its element, written order spelled out.
            let mut slot = 0;
            let stored = self
                .bump
                .alloc_slice_fill_iter(elements.iter().map(|element| match *element {
                    DispatchTokenElement::Keyword(keyword) => StoredElement::Keyword(keyword),
                    DispatchTokenElement::Slot(kt) => {
                        let class = class_of(classes, slot);
                        slot += 1;
                        StoredElement::Slot(kt.raw(), class)
                    }
                }));
            let elements = Elements::over(stored);
            TypeNode::ExpressionShape {
                elements,
                classes: elements.classes(),
                ret,
            }
        })
    }

    /// Count each variable's free contravariant occurrences across the positions a scheme binds,
    /// recording the visit order of its first occurrence. A nested scheme's own group shadows this
    /// one, so the census skips it.
    fn quantifier_census<'s>(
        &self,
        scratch: BumpAllocator<'s>,
        positions: impl Iterator<Item = (Handle, Variance)>,
        arity: usize,
    ) -> BumpVec<'s, Occurrences> {
        let mut census = BumpVec::with_capacity_in(arity, scratch);
        census.resize(arity, Occurrences::default());
        let mut seen = 0usize;
        for (kt, position) in positions {
            visit_free_quantified(self, scratch, kt, position, &mut |index, context| {
                if let Some(record) = census.get_mut(index) {
                    if record.first == usize::MAX {
                        record.first = seen;
                    }
                    if context.variance() == Variance::Contra {
                        record.contravariant += 1;
                    }
                    seen += 1;
                }
                Visit::Skip
            });
        }
        census
    }

    // --- Unions ---

    /// The canonicalizing constructor for a union — the single entry point that builds one.
    ///
    /// Flattens any nested union member into its members, drops [`KType::NEVER`] (the identity
    /// element: it admits nothing, so it widens nothing), and deduplicates by handle. A member
    /// [`KType::ANY`] absorbs the rest. The order relates concrete types only, so it reduces the
    /// concrete members among themselves — no concrete member lies under another, nor under the
    /// union of the rest — and keeps every parametric member beside them, even one whose bound lies
    /// under a concrete member. A union holding all three family tops is [`KType::ANY`]. One
    /// survivor collapses to that member; none is `Never`.
    ///
    /// Generic over its members' handle, as every composite door is: a union of concrete types is
    /// concrete.
    pub fn union_of<H: TypeHandle>(&self, scratch: BumpAllocator<'_>, members: &[H]) -> H {
        let width: usize = members
            .iter()
            .map(|member| match self.enter(member.raw()).content() {
                Content::Union { members: inner, .. } => inner.len(),
                _ => 1,
            })
            .sum();
        let mut flat = BumpVec::with_capacity_in(width, scratch);
        let push_unique = |handle: Handle, flat: &mut BumpVec<'_, Handle>| {
            if handle != Handle::NEVER && !flat.contains(&handle) {
                flat.push(handle);
            }
        };
        for member in members {
            match self.enter(member.raw()).content() {
                Content::Union { members: inner, .. } => {
                    for nested in inner.iter() {
                        push_unique(*nested, &mut flat);
                    }
                }
                _ => push_unique(member.raw(), &mut flat),
            }
        }
        wrap(self.canonical_union(scratch, flat))
    }

    /// The canonical union over `flat`, already flattened and deduplicated with `Never` dropped.
    fn canonical_union(&self, scratch: BumpAllocator<'_>, mut flat: BumpVec<'_, Handle>) -> Handle {
        // The top's own definition, not the order: every type lies under `Any`, a variable too.
        if flat.contains(&Handle::ANY) {
            return Handle::ANY;
        }
        if flat.len() > 1 {
            // Subsumption: a concrete member below another contributes nothing the other does not
            // already admit. Mutually ordered members are equal handles, which the dedup above
            // removed, so the surviving set is an antichain and dropping is order-insensitive.
            let keep = unsubsumed(self, scratch, &flat, Dropped::Below);
            let mut keep = keep.iter();
            flat.retain(|_| *keep.next().unwrap_or(&true));
        }
        // The three family tops together hold every type, so their union is `Any` — and must be, or
        // a type variable bounded by `Any` would lie under `Any` but not under the union that equals it.
        if [Handle::ANY_VALUE, Handle::ANY_TYPE, Handle::ANY_CODE]
            .iter()
            .all(|top| flat.contains(top))
        {
            return Handle::ANY;
        }
        match flat.len() {
            0 => Handle::NEVER,
            1 => flat[0],
            _ => self.intern_union_members(scratch, &flat),
        }
    }

    /// Intern a union from members that are already flat and already an antichain — dedup by handle
    /// and collapse a one-member result, but read no member nodes.
    ///
    /// The seal's door, and the seal's alone. A rewritten sibling handle names a still-uninterned
    /// member of the group being sealed, so [`union_of`](Self::union_of)'s flatten pass would fault
    /// on it. It is sound there because the rename `Sibling(i) ↦ member_i` preserves every
    /// subsumption verdict: both are atoms, below only themselves and `Never`, and distinct indices
    /// name distinct members — so a union canonical before the seal is canonical after it.
    pub(super) fn intern_union_flat(
        &self,
        scratch: BumpAllocator<'_>,
        members: &[Handle],
    ) -> Handle {
        let mut flat = BumpVec::with_capacity_in(members.len(), scratch);
        for member in members {
            if !flat.contains(member) {
                flat.push(*member);
            }
        }
        match flat.len() {
            0 => Handle::NEVER,
            1 => flat[0],
            _ => self.intern_union_members(scratch, &flat),
        }
    }

    /// Intern the `Union` node over the already-canonical `flat`, probing the table before copying
    /// the members in. The `Union` arm of `node_digest` *is* `union_digest` over the node's member
    /// slice, so the digest taken here off `flat` equals the digest the node would key at.
    fn intern_union_members(&self, scratch: BumpAllocator<'_>, flat: &[Handle]) -> Handle {
        self.intern_digested(digest::union_digest(scratch, flat), || TypeNode::Union {
            members: Run::over(self.rehome(flat)),
        })
    }

    // --- Quantifier probes ---

    /// Whether any `Quantified` position is reachable from `kt` without crossing a scheme — the
    /// probe that lets a slot type with nothing to solve answer the relations in one
    /// step instead of walking under a unifier. Read off the flags interning stored in the node.
    pub(super) fn contains_quantified(&self, kt: Handle) -> bool {
        self.enter(kt).flags().holds(Flags::QUANTIFIED)
    }

    /// Whether any rigid variable — `Quantified`, `Lexical` or `Parameter` — is reachable from
    /// `kt`. Read off the flags interning stored in the node.
    ///
    /// The invariant a **bound** carries: a bound is a variable-free type. That is what keeps the
    /// order's two rigid clauses consistent, since below a rigid variable are only itself and
    /// `Never` while above it is everything above its bound — and a rigid bound would put a
    /// variable in both sets at once. A bound is a [`KType`], so it holds no variable; the doors
    /// that take a bound assert it holds no opaque carrier either.
    pub(super) fn contains_rigid(&self, kt: Handle) -> bool {
        self.enter(kt).flags().holds(Flags::RIGID)
    }

    /// Whether `kt` is concrete: no free `Quantified`, `Lexical`, head `Parameter` or scheme is
    /// reachable outside sealed content. Read off the flags interning stored in the node.
    pub(super) fn is_concrete(&self, kt: Handle) -> bool {
        !self.enter(kt).flags().holds(Flags::PARAMETRIC)
    }

    /// `kt` as a [`KType`] where it is concrete — the one checked conversion from a parametric type.
    /// `None` where a variable is reachable from it.
    pub fn concrete(&self, kt: Parametric) -> Option<KType> {
        self.is_concrete(kt.raw()).then(|| wrap(kt.raw()))
    }

    /// Whether an opaque carrier is reachable from `kt` outside sealed content. Read off the flag
    /// interning stored in the node.
    pub(super) fn contains_carrier(&self, kt: Handle) -> bool {
        self.enter(kt).flags().holds(Flags::CARRIER)
    }

    /// Whether `kt` holds an opaque carrier outside sealed content. A bound holds none, so the
    /// elaborator refuses one there.
    pub fn holds_carrier(&self, kt: KType) -> bool {
        self.contains_carrier(kt.raw())
    }

    /// Whether `kt` reads the `index`-th quantifier of the enclosing scheme — what a definition asks
    /// of each name its `FOR ALL` group lists.
    pub fn references_quantifier<H: TypeHandle>(
        &self,
        scratch: BumpAllocator<'_>,
        kt: H,
        index: usize,
    ) -> bool {
        let kt = kt.raw();
        self.contains_quantified(kt)
            && visit_free_quantified(self, scratch, kt, Variance::Co, &mut |found, _| {
                if found == index {
                    Visit::Stop
                } else {
                    Visit::Skip
                }
            })
    }

    /// Whether `kt` reads the head parameter `name`: a `Parameter` of that name, which
    /// substituting `name` would replace. A signature is opaque, as it is there. What a member read
    /// asks of each head parameter its application leaves unpinned; it builds nothing.
    pub fn mentions_parameter(
        &self,
        scratch: BumpAllocator<'_>,
        kt: DeclaredType<Parametric>,
        name: TypeSymbol,
    ) -> bool {
        let kt = match kt {
            DeclaredType::Type(kt) => kt.raw(),
            DeclaredType::Scheme(scheme) => scheme.raw(),
        };
        self.contains_rigid(kt)
            && visit(self, scratch, kt, &mut |_, node, _| match *node {
                TypeNode::Parameter { name: found, .. } if found == name => Visit::Stop,
                _ => Visit::Descend,
            })
    }

    /// Whether any of quantifiers `0..arity` occurs free in `kt` at a contravariant position — what
    /// a family's representation may not do, since an application is covariant in its arguments.
    pub fn quantifies_contravariantly<H: TypeHandle>(
        &self,
        scratch: BumpAllocator<'_>,
        kt: H,
        arity: usize,
    ) -> bool {
        let kt = kt.raw();
        self.contains_quantified(kt)
            && self
                .quantifier_census(scratch, std::iter::once((kt, Variance::Co)), arity)
                .iter()
                .any(|occurrences| occurrences.contravariant > 0)
    }

    /// Whether every free `Quantified` position reachable from `kt` names an index below `arity` —
    /// the scheme doors' well-formedness probe.
    pub(super) fn quantifier_indices_in_range(
        &self,
        scratch: BumpAllocator<'_>,
        kt: Handle,
        arity: usize,
    ) -> bool {
        !visit_free_quantified(self, scratch, kt, Variance::Co, &mut |index, _| {
            if index >= arity {
                Visit::Stop
            } else {
                Visit::Skip
            }
        })
    }

    // --- Verdicts ---

    /// Consult the registry for a recorded verdict. A hit marks its slot the one touched last.
    pub(super) fn verdict(
        &self,
        subject: TypeDigest,
        candidate: TypeDigest,
        relation: Relation,
    ) -> Option<bool> {
        self.verdicts.get(subject, candidate, relation)
    }

    /// Record `verdict` for the key, laying the table in the run region on the first record.
    pub(super) fn record_verdict(
        &self,
        subject: TypeDigest,
        candidate: TypeDigest,
        relation: Relation,
        verdict: bool,
    ) {
        self.verdicts
            .record(self.bump, subject, candidate, relation, verdict);
    }
}

/// A quantifier group put in group order over the positions it binds — what both interning doors
/// hand their own positions and names through.
struct GroupOrder<'s> {
    /// The quantifiers' names, in group order.
    names: BumpVec<'s, TypeSymbol>,
    /// Each quantifier's bound, in the same order.
    bounds: BumpVec<'s, KType>,
    /// What each **declared** variable substitutes to: its `Quantified` at its group index.
    bindings: BumpVec<'s, Handle>,
    /// Declaration index → group index.
    quantifier_map: &'s [usize],
}

/// One variable's census entry while a scheme's group is being ordered.
#[derive(Clone, Copy)]
struct Occurrences {
    contravariant: usize,
    /// Visit sequence number of the first occurrence — the ordering key. `usize::MAX` until the
    /// variable is met at all.
    first: usize,
}

impl Default for Occurrences {
    fn default() -> Self {
        Occurrences {
            contravariant: 0,
            first: usize::MAX,
        }
    }
}
