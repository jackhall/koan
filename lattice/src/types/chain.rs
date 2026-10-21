//! The type chain: the tiers one habitat half reads and writes types through, and every door that
//! interns or reads a type.
//!
//! A [`TypeChain`] reads its tiers longest-lived first, under `'load: 'agent: 'here: 'scratch`: the
//! [`Frozen`] load, the [`Agent`] tier, a scratch chain's region branch, then the tier it writes,
//! reached through the [`Habitat`] it is built over — a cell region's or a scratch habitat's branch
//! at the run, the [`RootBuilder`](super::tiers::RootBuilder) during the load, the one chain with
//! no frozen load and no agent tier, or the agent tier itself on the resting chain
//! ([`TypeChain::resting`]), which reads no agent tier above its own. A scratch habitat's chain is
//! built over [`Halves`], which holds the region beside the half it writes. A node references
//! nodes of its own tier or a longer-lived one, by covariance alone. See [tiers.rs](super::tiers)
//! for the tiers themselves.
//!
//! Each node is a [`StoredNode`](super::stored::StoredNode) laid alone in the half the chain
//! writes, holding its own handle beside its content and a reference to each child's node. A
//! [`Handle`] is the digest of its node under two flags, and the digest is already a uniformly
//! distributed hash, so each tier's table finds a node by its digest's low word and compares
//! against the digest the node holds — it stores no second copy. Interning is insert-if-absent:
//! [`find`](TypeChain::find), which has a digest and no flags, probes the load, the agent tier, the
//! region tier, then the own tier, and a miss lays the node in the own tier. A lookup by handle
//! ([`locate`](TypeChain::locate)) is routed by the handle's in-root bit: a load handle probes the
//! load alone, and any other skips it. Building the same content twice in a chain
//! yields one node and two equal handles. Nothing ever leaves a tier and nothing in one carries
//! drop glue, so a half releases its branch and every node in it whole.
//!
//! A handle is looked up once, where it enters the lattice ([`TypeChain::enter`]); every walk and
//! relation inside follows references from there. A handle the chain's tiers lack is read through
//! the own branch's link, and the region tier's, and cached in the own tier, so the walk is paid
//! once per handle per half; an intern probes no link, and lays again content a linked tier holds,
//! which identity by digest allows. `Never` and `Any` are held on the chain, so producing either
//! costs no lookup. [`TypeChain::node`] reads a stored node back as a [`TypeNode`] view. A read
//! takes a reference into its tier and releases any table borrow before the reader runs, so reads
//! nest and a reader may intern. A stored node carries six flags computed off its children at
//! intern — whether a free quantifier, whether any rigid variable, whether any variable or scheme
//! outside sealed content, whether an opaque carrier outside sealed content is reachable, and
//! whether the type is porous or exposing — so each probe is one read of the node.
//!
//! A verdict over two load types is read from the load's table, then the agent's, and recorded in
//! the agent's, since no run writes the load's; one whose types are load or agent types is the
//! agent's; any other is read from the own tier's, the region tier's, then the agent's, and
//! recorded in the shortest-lived tier holding one of its two types ([`verdicts`](super::verdicts)).
//!
//! [`adopt`](TypeChain::adopt), [`merge`](TypeChain::merge) and [`link`](TypeChain::link) are how a
//! crossing makes another half's handles resolve here ([transfer.rs](super::transfer)).
//!
//! Every door that sorts, flattens or canonicalizes takes a scratch [`BumpAllocator`] for its
//! transient buffers, and every door computes its digest off the caller's own slices first, so
//! content is laid in the half only on a miss.
//!
//! See [identity.md § Storage](identity.md#storage-a-root-and-its-branches).

use std::cell::Cell;

use crate::bump::{Bump, BumpAllocator, BumpVec};
use crate::symbols::{BinderSymbol, Symbol, TypeSymbol};

use super::component::Component;
use super::digest::{self, ComponentMember, TypeDigest, schema_content_digest};
use super::handle::{DeclaredType, Handle, KType, Naming, Parametric, TypeHandle, wrap};
use super::kind::KKind;
use super::node::{ContentKey, Group, NodeSchema, TypeNode};
use super::order::{Dropped, is_subtype_of, unsubsumed};
use super::record::Record;
use super::run::{Elements, Run};
use super::schema::{
    Members, SchemaDraft, SigOrigin, SigSchema, canonical_groups, canonical_overloads, member,
};
use super::shape::{
    DeferredReturnSurface, DispatchTokenElement, StoredElement, class_of, written_order,
};
use super::signatures::canonical_applications;
use super::stored::{Content, Flags, Linked, StoredNode, copied, mapped_fields};
use super::substitute::{rewrite_siblings, substitute_quantified};
use super::tiers::{Agent, AgentTier, ChainPosition, Frozen, Habitat, Halves, Own, lay_one};
use super::verdicts::{Relation, VerdictKey};
use super::walk::Variance;
use super::walk::unary::{Visit, children, visit, visit_free_quantified};

/// The porous and exposing flags of a flagged `node`, from its children's
/// ([type tiers](../../../design/type-tiers.md#porosity)). A child at a flipped position — a
/// parameter or a slot — makes the node porous unless it is `Any`, since every other type there
/// admits a union with a minted one; a child at the node's own variance hands up its porous flag,
/// an application's family aside. A child's **part** exposes where it is exposing, or where it is
/// a porous function type, whose value keeps its own memo. Every other variant's flags are
/// [`Content::flags`]'s constants.
fn porosity_over<'t>(node: &TypeNode<'t, Linked<'t>>) -> Flags {
    let (mut porous, mut parts) = (false, false);
    // An application lies under its bare family, never over it: what lies under an application
    // differs from it only in its arguments, so its constructor — its first child — hands up no
    // porous flag.
    let mut constructor = matches!(node, TypeNode::ConstructorApply { .. });
    children(node, &mut |child, flips| {
        porous |= if flips {
            child != Handle::ANY
        } else {
            !std::mem::take(&mut constructor) && child.is_porous()
        };
        parts |= child.is_exposing() || (child.is_function_type() && child.is_porous());
    });
    let (porous, exposing) = match node {
        TypeNode::Record { .. } => (true, parts),
        TypeNode::List { .. } | TypeNode::Dict { .. } | TypeNode::ConstructorApply { .. } => {
            (porous, parts)
        }
        // Under a union a part keeps its own memo.
        TypeNode::Union { .. } => (porous, porous || parts),
        // What a function reaches it reaches through its captures.
        TypeNode::KFunction { .. } | TypeNode::ExpressionShape { .. } | TypeNode::Scheme { .. } => {
            (porous, false)
        }
        TypeNode::SignatureApply { .. } | TypeNode::SignatureMeet { .. } => (true, true),
        _ => (false, false),
    };
    let mut flags = Flags::NONE;
    if porous {
        flags = flags | Flags::POROUS;
    }
    if exposing {
        flags = flags | Flags::EXPOSING;
    }
    flags
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

/// The tiers one habitat half reads and writes types through: the frozen load, the agent tier, a
/// scratch habitat's region tier, and the half's own tier, reached through `half`. Interior mutability
/// lives in the tiers: a read takes a reference to its node and releases any table borrow before
/// the reader runs, so reads nest freely and a reader may intern.
///
/// Invariant in `'t`, as the half's writer is: a value door pairs a writer with a chain at one
/// brand, so a type is interned into the branch of the habitat its value is laid in.
pub struct TypeChain<'t, H> {
    /// The frozen load; `None` only on the load's chain, whose own tier is the root builder.
    frozen: Option<&'t Frozen<'t>>,
    /// The agent tier, read between the load and the region tier; `None` on the load's chain and
    /// on the chain whose own tier it is.
    agent: Option<&'t dyn AgentTier<'t>>,
    /// The bottom and the top, held so producing either costs no lookup.
    never: Linked<'t>,
    any: Linked<'t>,
    /// The half the chain lays into, its own tier, and a scratch chain's region tier.
    half: H,
    /// `test`-only: how many handles have been entered, so a test can count a walk's lookups.
    #[cfg(test)]
    lookups: Cell<usize>,
    /// `test`-only: how many probes the frozen load's node or verdict table has taken.
    #[cfg(test)]
    load_probes: Cell<usize>,
    /// `test`-only: how many probes the own or region tier's verdict table has taken.
    #[cfg(test)]
    branch_verdict_probes: Cell<usize>,
}

impl<'r: 't, 't, H: Habitat<'t>, R: Habitat<'r>> TypeChain<'t, Halves<'r, 't, H, R>> {
    /// A scratch habitat's chain over `half`, reading the frozen load, the agent tier, then
    /// `region`'s tier before its own. The region is held by value, so building the chain lays
    /// nothing, and it is re-read on every probe, so a branch the region lays during the step is
    /// seen.
    pub fn scratch<'a: 't>(
        frozen: &'t Frozen<'t>,
        agent: &'t Agent<'a>,
        region: R,
        half: H,
    ) -> Self {
        debug_assert!(
            matches!(region.own(), None | Some(Own::Branch(_))),
            "a region tier is a branch: the builder's nodes are the load's, and the agent tier is \
             read above it"
        );
        Self::new(frozen, agent, Halves::over(half, region))
    }
}

/// Which tier holds a type, longest-lived first: where a verdict over it is placed.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Placed {
    Load,
    Agent,
    Region,
    Own,
}

impl<'t, H: Habitat<'t>> TypeChain<'t, H> {
    /// The load's chain, over `half`, whose own tier is the root builder.
    pub fn building(half: H) -> Self {
        let Some(Own::Builder(builder)) = half.own() else {
            panic!("the load's chain is built over the root builder");
        };
        Self::over(None, None, builder.never(), builder.any(), half)
    }

    /// A chain over `half`, reading `frozen`, then `agent`, before its own tier.
    pub fn new<'a: 't>(frozen: &'t Frozen<'t>, agent: &'t Agent<'a>, half: H) -> Self {
        Self::over(
            Some(frozen),
            Some(agent),
            frozen.never(),
            frozen.any(),
            half,
        )
    }

    /// The chain that lays into the agent tier `half` writes, reading `frozen` first.
    pub fn resting(frozen: &'t Frozen<'t>, half: H) -> Self {
        debug_assert!(
            matches!(half.own(), Some(Own::Agent(_))),
            "a resting chain writes the agent tier"
        );
        Self::over(Some(frozen), None, frozen.never(), frozen.any(), half)
    }

    fn over(
        frozen: Option<&'t Frozen<'t>>,
        agent: Option<&'t dyn AgentTier<'t>>,
        never: Linked<'t>,
        any: Linked<'t>,
        half: H,
    ) -> Self {
        TypeChain {
            frozen,
            agent,
            never,
            any,
            half,
            #[cfg(test)]
            lookups: Cell::new(0),
            #[cfg(test)]
            load_probes: Cell::new(0),
            #[cfg(test)]
            branch_verdict_probes: Cell::new(0),
        }
    }

    /// How many nodes the chain's own tier holds: 0 for a half with no branch yet. Tests read it —
    /// koan's among them, which is why it is not `cfg(test)`: that does not reach a dependency.
    pub fn own_len(&self) -> usize {
        self.half.own().map_or(0, Own::len)
    }

    /// The bottom's node.
    pub(super) fn never(&self) -> Linked<'t> {
        self.never
    }

    /// The top's node.
    pub(super) fn any(&self) -> Linked<'t> {
        self.any
    }

    /// The half the chain lays into. A caller reads it to see where a type the chain interns
    /// lands: in the root, or in this half's own tier or the region tier it reads.
    pub fn half(&self) -> H {
        self.half
    }

    /// `test`-only: how many handles have been entered into the lattice.
    #[cfg(test)]
    pub(super) fn lookups(&self) -> usize {
        self.lookups.get()
    }

    /// `test`-only: how many probes the frozen load's node or verdict table has taken.
    #[cfg(test)]
    pub(super) fn load_probes(&self) -> usize {
        self.load_probes.get()
    }

    /// `test`-only: how many probes the own or region tier's verdict table has taken.
    #[cfg(test)]
    pub(super) fn branch_verdict_probes(&self) -> usize {
        self.branch_verdict_probes.get()
    }

    /// `test`-only: count one probe of the frozen load's tables.
    fn count_load_probe(&self) {
        #[cfg(test)]
        self.load_probes.set(self.load_probes.get() + 1);
    }

    /// `test`-only: count one probe of a branch tier's verdict table.
    fn count_branch_verdict_probe(&self) {
        #[cfg(test)]
        self.branch_verdict_probes
            .set(self.branch_verdict_probes.get() + 1);
    }

    /// Intern every constant node, so a fixed handle always resolves. `scratch` serves the doors
    /// that sort or flatten.
    pub(super) fn seed_constants(&self, scratch: BumpAllocator<'_>) {
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
            self.intern(scratch, leaf);
        }
        for kind in [
            KKind::ProperType,
            KKind::Signature,
            KKind::AnyType,
            KKind::NewType,
            KKind::TypeConstructor,
        ] {
            self.intern(scratch, TypeNode::OfKind(kind));
        }
        self.list(Handle::ANY);
        self.dict(Handle::ANY, Handle::ANY);
        self.intern_schema(SigSchema::EMPTY);
        let type_code = self.union_of(
            scratch,
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
            scratch,
            &[Handle::LIST_OF_NAME, Handle::DICT_NAME_TYPE_CODE],
        );
        self.record::<Handle>(scratch, &[]);
    }

    // --- Content: interning and node reads ---

    /// Intern `node` and return its node. Interning the same content twice yields one node. The
    /// generic door: its slices are stored as handed in, so they must already live at least as
    /// long as the half.
    pub(super) fn intern(
        &self,
        scratch: BumpAllocator<'_>,
        node: TypeNode<'t, Linked<'t>>,
    ) -> Linked<'t> {
        self.intern_digested(digest::node_digest(scratch, &node), || node)
    }

    /// Insert the node `build` produces under `digest` if the digest is not already present — the
    /// one insert-if-absent path every interning door takes.
    ///
    /// A hit — the steady state, since a repeated spelling of a type names an already-interned
    /// node — never calls `build`, so a door that computes its digest off the caller's slices lays
    /// nothing in the half. On a miss the view `build` returns has its flags folded from its
    /// children and is laid as a stored node.
    ///
    /// No table borrow is held across `build`, but `build` only copies the caller's slices into the
    /// half and never interns.
    fn intern_digested(
        &self,
        digest: TypeDigest,
        build: impl FnOnce() -> TypeNode<'t, Linked<'t>>,
    ) -> Linked<'t> {
        self.intern_laid(digest, || {
            let node = build();
            let folded = self.flags_over(&node);
            let content = Content::lay(node, folded, self.half);
            debug_assert_eq!(
                content.flags(),
                content.flags() | folded,
                "a node's flags hold its children's",
            );
            content
        })
    }

    /// The insert-if-absent core under every intern: on a miss, `lay` builds the stored content,
    /// and the node is laid alone in the half and entered in its own tier under `digest`. Returns
    /// the node found or laid. The probe reads no link.
    pub(super) fn intern_laid(
        &self,
        digest: TypeDigest,
        lay: impl FnOnce() -> Content<'t>,
    ) -> Linked<'t> {
        if let Some(found) = self.find(digest) {
            return found;
        }
        let content = lay();
        debug_assert!(self.find(digest).is_none(), "`lay` interns nothing");
        // The load's chain, the one with no root, lays into the root builder.
        let node: Linked<'t> = lay_one(
            self.half,
            StoredNode::new(digest, content, self.frozen.is_none()),
        );
        self.half.own_or_lay().insert(self.half, node);
        node
    }

    /// `node`'s flags folded from its children's own: each child is a node, interned first. A
    /// variable's or a carrier's own flags are [`Content::flags`]'s constants.
    pub(super) fn flags_over(&self, node: &TypeNode<'t, Linked<'t>>) -> Flags {
        let mut folded = Flags::NONE;
        children(node, &mut |child, _| {
            folded = folded | child.flags().without(Flags::POROSITY);
        });
        let folded = folded | porosity_over(node);
        // A scheme binds its own variables, so nothing under one is free here, and the scheme
        // itself is a quantified callable's type.
        if node.binds_quantifiers() {
            folded.without(Flags::QUANTIFIED) | Flags::PARAMETRIC
        } else {
            folded
        }
    }

    /// A caller's field run copied into the half, each type by its node: the one lookup per field
    /// an intern miss takes for a typed operand.
    fn rehome_fields<N: Naming<'t>>(
        &self,
        fields: &[(BinderSymbol, N)],
    ) -> &'t [(BinderSymbol, Linked<'t>)] {
        mapped_fields(self.half, fields, &mut |kt| kt.linked(self))
    }

    /// The content `handle` names, by value, its children read as `handle`'s children: a concrete
    /// type's are concrete, a parametric type's parametric.
    ///
    /// The node lives in a tier, and any table borrow ends before the caller reads it. That is what
    /// lets a reader intern, and what makes reads nest freely. The node's slices live as long as
    /// its tier, so a reader may hand one back as `&'t`.
    ///
    /// A miss is a bug, not a state: a handle is only ever produced by an interning door, and a
    /// crossing makes every handle its value carries resolve in the destination.
    pub fn node<N: TypeHandle>(&self, handle: N) -> TypeNode<'t, N::Child> {
        self.enter(handle.raw()).view_as()
    }

    /// The node stored under `handle`, by reference — the one lookup a handle takes into the
    /// lattice, where it enters: every walk and relation inside follows references from there. A
    /// handle the chain's tiers lack is read through the own branch's link, then the region tier's,
    /// and cached in the own tier. A miss panics, as [`node`](Self::node)'s does.
    pub(super) fn enter(&self, handle: Handle) -> Linked<'t> {
        #[cfg(test)]
        self.lookups.set(self.lookups.get() + 1);
        match self.resolve(handle) {
            Some(found) => found,
            None => panic!(
                "type handle 0x{:032x} names no interned node",
                handle.digest().0
            ),
        }
    }

    /// The node under `handle` in a tier the chain probes, else — for a handle not the root's —
    /// through a link, cached in the own tier so the next read probes no link.
    pub(super) fn resolve(&self, handle: Handle) -> Option<Linked<'t>> {
        if let Some(found) = self.locate(handle) {
            return Some(found);
        }
        if handle.in_root() {
            return None;
        }
        let found = self.linked_reach(handle.digest())?;
        self.half.own_or_lay().insert(self.half, found);
        Some(found)
    }

    /// Whether `handle` names a node this chain can read: in a tier it probes, or through a link.
    /// Caches nothing, so a check that reads it leaves the tiers as they were.
    pub fn resolves(&self, kt: impl Into<DeclaredType<Parametric>>) -> bool {
        let handle = kt.into().raw();
        self.locate(handle).is_some()
            || (!handle.in_root() && self.linked_reach(handle.digest()).is_some())
    }

    /// Whether `kt`'s node lies in the frozen load, or, on the load's chain, in the root builder:
    /// a type every chain over the load resolves. Reads the handle's in-root bit: looks nothing
    /// up.
    pub fn in_root(&self, kt: impl Into<DeclaredType<Parametric>>) -> bool {
        kt.into().raw().in_root()
    }

    /// The node under `digest` in the frozen load, the agent tier, the region tier or the own
    /// tier, longest-lived first — the intern's probe, which knows no bits and reads no link.
    pub(super) fn find(&self, digest: TypeDigest) -> Option<Linked<'t>> {
        if let Some(frozen) = self.frozen {
            self.count_load_probe();
            if let Some(found) = frozen.find(digest) {
                return Some(found);
            }
        }
        self.find_past_load(digest)
    }

    /// The node under `digest` in the agent tier, the region tier or the own tier, longest-lived
    /// first: where a handle not the load's lies.
    fn find_past_load(&self, digest: TypeDigest) -> Option<Linked<'t>> {
        self.agent
            .and_then(|agent| agent.find(digest))
            .or_else(|| {
                self.half
                    .region()
                    .and_then(|region| region.tier().find(digest))
            })
            .or_else(|| self.half.own()?.find(digest))
    }

    /// The node under `handle` in a tier the chain probes, routed by its in-root bit: a load
    /// handle's in the frozen load alone — on the load's chain, the builder, its own tier — and
    /// any other's in the agent tier, the region tier, then the own tier, never the load. Reads no
    /// link.
    pub(super) fn locate(&self, handle: Handle) -> Option<Linked<'t>> {
        let digest = handle.digest();
        if handle.in_root() {
            return match self.frozen {
                Some(frozen) => {
                    self.count_load_probe();
                    frozen.find(digest)
                }
                None => self.half.own()?.find(digest),
            };
        }
        self.find_past_load(digest)
    }

    /// The tier holding `handle`, longest-lived first, without probing the load: a load handle's
    /// is the load (the own tier on the load's chain); any other's is the agent tier, the region
    /// tier or the own tier, and the own tier where none finds it — a child of a node read through
    /// a link.
    fn tier_of(&self, handle: Handle) -> Placed {
        if handle.in_root() {
            return if self.frozen.is_some() {
                Placed::Load
            } else {
                Placed::Own
            };
        }
        let digest = handle.digest();
        if self.agent.is_some_and(|agent| agent.find(digest).is_some()) {
            Placed::Agent
        } else if (self.half.region()).is_some_and(|region| region.tier().find(digest).is_some()) {
            Placed::Region
        } else {
            Placed::Own
        }
    }

    /// The node under `digest` through the own branch's link, then the region tier's.
    pub(super) fn linked_reach(&self, digest: TypeDigest) -> Option<Linked<'t>> {
        self.half
            .own()
            .and_then(|own| own.link_reach(digest))
            .or_else(|| {
                self.half
                    .region()
                    .and_then(|region| region.tier().reach(digest))
            })
    }

    /// Whether the own tier or the region tier is the branch at `at`, or reaches it through its
    /// links, walking each of their chains only as far as `at`'s depth.
    pub(super) fn reaches_at(&self, at: ChainPosition, steps: &Cell<usize>) -> bool {
        self.half.own().is_some_and(|own| own.reaches_at(at, steps))
            || self
                .half
                .region()
                .is_some_and(|region| region.tier().reaches_at(at, steps))
    }

    /// Whether the own tier or the region tier is the branch at `address`, or reaches it through
    /// its links: a branch whose nodes every read of the chain already finds. The full walk a
    /// merge's debug check compares [`reaches_at`](Self::reaches_at) against.
    pub(super) fn reaches_branch(&self, address: *const ()) -> bool {
        self.half
            .own()
            .is_some_and(|own| own.reaches_branch(address))
            || self
                .half
                .region()
                .is_some_and(|region| region.tier().reaches_branch(address))
    }

    /// Whether `handle` names a union. The construction lane's first probe, so it answers from the
    /// node's shape alone and never reads out the member list.
    pub fn is_union<N: TypeHandle>(&self, handle: N) -> bool {
        matches!(self.enter(handle.raw()).content(), Content::Union { .. })
    }

    /// The `union` member named `name`, whatever schema it declares. `name` probes by bare symbol
    /// bits: the token arrives from a reference site with no class attached.
    pub fn union_member_named(&self, union: KType, name: Symbol) -> Option<KType> {
        let Content::Union { members, .. } = self.enter(union.raw()).content() else {
            return None;
        };
        members
            .iter()
            .find(|m| {
                matches!(
                    m.content(),
                    Content::SetMember {
                        name: member_name, ..
                    } if member_name.symbol() == name
                )
            })
            .map(|m| wrap(m.handle()))
    }

    // --- Composite construction ---
    //
    // The single entry point per composite shape. Each takes child handles and returns the
    // parent's handle, so building a type is bottom-up interning and no site can construct a
    // composite the chain has not seen.
    //
    // A door that builds a type from child types is generic over how its children are named: a
    // `KType` out of `KType` children, a `Parametric` out of parametric ones, and inside the
    // lattice a node out of nodes. A child holds no binder of the door's own, so the composite
    // holds exactly the variables its children do.

    /// `List<element>`.
    ///
    /// A list of a concrete element is concrete, and of a parametric one parametric — a
    /// parametric element yields no `KType`:
    ///
    /// ```compile_fail,E0308
    /// use lattice::bump::Bump;
    /// use lattice::symbols::{SymbolInterner, TypeSymbol};
    /// use lattice::types::{KType, RootBuilder, TypeChain};
    ///
    /// let region = Bump::new();
    /// let types = TypeChain::building(RootBuilder::in_region(&region));
    /// let symbols = SymbolInterner::new();
    /// let name = TypeSymbol::declared("Elt", &symbols).expect("a Type token");
    /// let variable = types.lexical(0, name, KType::ANY);
    /// let _list: KType = types.list(variable);
    /// ```
    pub fn list<N: Naming<'t>>(&self, element: N) -> N {
        N::read(
            self.intern_digested(digest::list_digest(element.digest()), || TypeNode::List {
                element: element.linked(self),
            }),
        )
    }

    /// `Dict<key, value>`.
    pub fn dict<N: Naming<'t>>(&self, key: N, value: N) -> N {
        N::read(
            self.intern_digested(digest::dict_digest(key.digest(), value.digest()), || {
                TypeNode::Dict {
                    key: key.linked(self),
                    value: value.linked(self),
                }
            }),
        )
    }

    /// A structural record type over `fields`, in declaration order with unique names.
    pub fn record<N: Naming<'t>>(
        &self,
        scratch: BumpAllocator<'_>,
        fields: &[(BinderSymbol, N)],
    ) -> N {
        N::read(
            self.intern_digested(digest::record_digest(scratch, fields), || {
                TypeNode::Record {
                    fields: Record::over(self.rehome_fields(fields)),
                }
            }),
        )
    }

    /// The function type `(params) -> ret`, binding no group: a variable inside reads the enclosing
    /// scheme's group.
    pub fn function_type<N: Naming<'t>>(
        &self,
        scratch: BumpAllocator<'_>,
        params: &[(BinderSymbol, N)],
        ret: N,
    ) -> N {
        N::read(self.intern_function(scratch, params, ret.linked(self)))
    }

    /// The one door that mints a quantified function type, its `FOR ALL` group (`quantifiers` and
    /// their `bounds`, in declaration order) put in group order: it interns the plain function
    /// type, then binds the group over it ([`bind_group`](Self::bind_group)). A
    /// [`Scheme`](super::handle::Scheme), or the plain function type where the group is empty.
    pub fn function_scheme<'s>(
        &self,
        scratch: BumpAllocator<'s>,
        quantifiers: &[TypeSymbol],
        bounds: &[KType],
        params: &[(BinderSymbol, Parametric)],
        ret: Parametric,
    ) -> GroupIntern<'s> {
        let body = self.intern_function(scratch, params, ret.linked(self));
        self.scheme(scratch, quantifiers, bounds, body)
    }

    /// Intern a function type. Probe-first, as [`intern_shape`](Self::intern_shape) is, so the
    /// params run is copied into the region only on a genuine miss.
    fn intern_function<N: Naming<'t>>(
        &self,
        scratch: BumpAllocator<'_>,
        params: &[(BinderSymbol, N)],
        ret: Linked<'t>,
    ) -> Linked<'t> {
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
        KType::read(
            self.intern_digested(digest::deferred_return_digest(surface), || {
                TypeNode::DeferredReturn(match surface {
                    DeferredReturnSurface::Type(name) => DeferredReturnSurface::Type(name),
                    DeferredReturnSurface::Expression(text) => {
                        DeferredReturnSurface::Expression(self.half.text(text))
                    }
                })
            }),
        )
    }

    /// Application of a higher-kinded type constructor to the parameter-name-keyed `arguments`.
    pub fn constructor_apply<N: Naming<'t>>(
        &self,
        scratch: BumpAllocator<'_>,
        constructor: N::Bound,
        arguments: &[(BinderSymbol, N)],
    ) -> N {
        let digest = digest::constructor_apply_digest(scratch, constructor.digest(), arguments);
        N::read(self.intern_digested(digest, || TypeNode::ConstructorApply {
            constructor: constructor.linked(self),
            arguments: Record::over(self.rehome_fields(arguments)),
        }))
    }

    /// A signature's head parameter: a named variable its members read and `WITH` pins, bounded by
    /// `bound` — [`KType::ANY`] where the declaration constrains nothing.
    pub fn head_parameter(&self, name: TypeSymbol, bound: KType) -> Parametric {
        Parametric::read(self.parameter(name, bound))
    }

    /// The carrier an opaque view hides the head parameter `name` behind, keyed on `key`, the
    /// content the view hides: two views of equal content share it, and two of different content
    /// never unify. A value carries it and dispatches on it, so it is concrete, and the order reads
    /// it as an atom under `Any` alone. It records `met`, the bound its source met, for a
    /// signature's fit alone.
    pub fn carrier(&self, name: TypeSymbol, met: KType, key: ContentKey) -> KType {
        let met = met.linked(self);
        self.assert_bound(met);
        KType::read(
            self.intern_digested(digest::carrier_digest(name, key, met.digest()), || {
                TypeNode::Carrier { name, key, met }
            }),
        )
    }

    /// Whether `kt` is an opaque view's carrier.
    pub fn is_carrier(&self, kt: KType) -> bool {
        matches!(self.enter(kt.raw()).content(), Content::Carrier { .. })
    }

    /// A head parameter named `name`, bounded by `bound`.
    pub(super) fn parameter(&self, name: TypeSymbol, bound: impl Naming<'t>) -> Linked<'t> {
        let bound = bound.linked(self);
        self.assert_bound(bound);
        self.intern_digested(digest::parameter_digest(name, bound.digest()), || {
            TypeNode::Parameter { name, bound }
        })
    }

    /// A bound is concrete by its type. An opaque carrier is concrete too, but a bound holds none:
    /// the elaborator refuses one there, and this checks that it did.
    fn assert_bound(&self, bound: Linked<'t>) {
        debug_assert!(!bound.contains_carrier(), "a bound holds no opaque carrier");
    }

    /// A code kind needing `names` where its code is built. `kind` is a code kind below `Code`;
    /// `names` may arrive in any order and repeat, since identity is their set, and needing none
    /// is the bare `kind` itself.
    pub fn code_needing<N: Naming<'t>>(
        &self,
        scratch: BumpAllocator<'_>,
        kind: N,
        names: &[BinderSymbol],
    ) -> N {
        debug_assert!(
            kind.linked(self).handle().code_parent().is_some(),
            "a code kind needing names is a kind below `Code`",
        );
        if names.is_empty() {
            return kind;
        }
        let mut sorted = BumpVec::with_capacity_in(names.len(), scratch);
        sorted.extend_from_slice(names);
        sorted.sort_unstable();
        sorted.dedup();
        let digest = digest::code_needing_digest(kind.digest(), &sorted);
        N::read(self.intern_digested(digest, || TypeNode::CodeNeeding {
            kind: kind.linked(self),
            names: copied(self.half, &sorted),
        }))
    }

    /// The `index`-th rigid variable of the enclosing scheme's group, bounded by `bound`.
    pub fn quantified(&self, index: usize, bound: KType) -> Parametric {
        Parametric::read(self.quantified_node(index, bound))
    }

    /// [`quantified`](Self::quantified) as a node.
    pub(super) fn quantified_node(&self, index: usize, bound: impl Naming<'t>) -> Linked<'t> {
        let bound = bound.linked(self);
        self.assert_bound(bound);
        self.intern_digested(digest::quantified_digest(index, bound.digest()), || {
            TypeNode::Quantified { index, bound }
        })
    }

    /// The lexical variable at `level`, named `name`, bounded by `bound` and above `Never`.
    pub fn lexical(&self, level: usize, name: TypeSymbol, bound: KType) -> Parametric {
        let bound = bound.linked(self);
        // An unset lower end skips the check `scratch` serves, so an empty bump never allocates.
        let scratch = Bump::new();
        Parametric::read(self.lexical_node(&scratch, level, name, self.never, bound))
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
        let (lower, bound) = (lower.linked(self), bound.linked(self));
        Parametric::read(self.lexical_node(scratch, level, name, lower, bound))
    }

    /// [`lexical_between`](Self::lexical_between) as a node. `scratch` serves the debug check that
    /// `lower` lies under `bound`, which an unset lower end skips.
    pub(super) fn lexical_node(
        &self,
        scratch: BumpAllocator<'_>,
        level: usize,
        name: TypeSymbol,
        lower: Linked<'t>,
        bound: Linked<'t>,
    ) -> Linked<'t> {
        self.assert_bound(lower);
        self.assert_bound(bound);
        debug_assert!(
            lower == Handle::NEVER
                || (lower != bound && is_subtype_of(self, scratch, lower, bound)),
            "a lexical variable's lower end lies strictly under its bound",
        );
        let digest = digest::lexical_digest(level, name, lower.digest(), bound.digest());
        self.intern_digested(digest, || TypeNode::Lexical {
            level,
            name,
            lower,
            bound,
        })
    }

    /// A relative sibling reference against an open recursive-group window.
    pub fn sibling(&self, index: usize) -> KType {
        KType::read(self.sibling_node(index))
    }

    /// [`sibling`](Self::sibling) as a node.
    pub(super) fn sibling_node(&self, index: usize) -> Linked<'t> {
        self.intern_digested(digest::sibling_digest(index), || TypeNode::Sibling(index))
    }

    /// One sealed component's members, interned in its canonical order: `members` as the
    /// component digest read them, their schemas relative. A component an earlier seal laid keeps
    /// its nodes. Otherwise the component is laid and tied ([`Component::tie`]): its members are
    /// interned, then each schema is rewritten absolute against them, staged in `scratch`.
    pub(super) fn seal_component<'s>(
        &self,
        scratch: BumpAllocator<'s>,
        digest: TypeDigest,
        members: &[ComponentMember<'_>],
    ) -> BumpVec<'s, Linked<'t>> {
        let sealed = self.find(digest::member_ref_digest(digest, 0));
        let component = match sealed.map(StoredNode::content) {
            Some(Content::SetMember { component, .. }) => component,
            _ => {
                let relative = self.half.lay(members.len(), |at| ComponentMember {
                    schema: self.rehome_schema(members[at].schema),
                    ..members[at]
                });
                Component::tie(self.half, digest, relative, |component| {
                    let nodes = self.member_nodes(scratch, component);
                    let resolve = |sibling: usize| nodes[sibling];
                    self.half.lay(relative.len(), |at| ComponentMember {
                        schema: self.absolute_schema(scratch, relative[at].schema, &resolve),
                        ..relative[at]
                    })
                })
            }
        };
        self.member_nodes(scratch, component)
    }

    /// Every member of a sealed `component`, each its one node, in canonical order.
    fn member_nodes<'s>(
        &self,
        scratch: BumpAllocator<'s>,
        component: &'t Component<'t>,
    ) -> BumpVec<'s, Linked<'t>> {
        let count = component.members().len();
        let mut nodes = BumpVec::with_capacity_in(count, scratch);
        for index in 0..count {
            nodes.push(
                self.intern_laid(digest::member_ref_digest(component.digest, index), || {
                    member_content(component, index)
                }),
            );
        }
        nodes
    }

    /// A schema with its parameter names copied into the half.
    pub(super) fn rehome_schema(&self, schema: NodeSchema<'_>) -> NodeSchema<'t> {
        match schema {
            NodeSchema::NewType(repr) => NodeSchema::NewType(repr),
            NodeSchema::TypeConstructor {
                representation,
                param_names,
            } => NodeSchema::TypeConstructor {
                representation,
                param_names: copied(self.half, param_names),
            },
        }
    }

    /// A relative `schema` made absolute: each sibling reference read as the member `resolve`
    /// names, re-interned through the canonical doors.
    fn absolute_schema(
        &self,
        scratch: BumpAllocator<'_>,
        schema: NodeSchema<'t>,
        resolve: &impl Fn(usize) -> Linked<'t>,
    ) -> NodeSchema<'t> {
        let rewrite =
            |relative: Handle| rewrite_siblings(self, scratch, self.enter(relative), resolve);
        match schema {
            NodeSchema::NewType(repr) => NodeSchema::NewType(KType::read(rewrite(repr.raw()))),
            NodeSchema::TypeConstructor {
                representation,
                param_names,
            } => NodeSchema::TypeConstructor {
                representation: representation.map(|kt| Parametric::read(rewrite(kt.raw()))),
                param_names,
            },
        }
    }

    /// `member`'s schema, absolute; `None` for a type that is not a sealed member.
    pub fn member_schema(&self, member: impl TypeHandle) -> Option<NodeSchema<'t>> {
        match self.enter(member.raw()).content() {
            Content::SetMember {
                index, component, ..
            } => Some(component.members()[index as usize].schema),
            _ => None,
        }
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
        KType::read(self.intern_schema(schema))
    }

    /// Intern a schema that is already canonical, wherever its slices live: the seed's door for
    /// [`SigSchema::EMPTY`]. Computes the schema's content digest once, here, so the node carries
    /// it and identity is one compare.
    pub(super) fn intern_schema(&self, schema: SigSchema<'_>) -> Linked<'t> {
        let schema_digest = schema_content_digest(schema);
        self.intern_digested(digest::signature_digest(schema_digest), || {
            TypeNode::Signature {
                schema: schema.copied_into(self.half),
                schema_digest,
            }
        })
    }

    /// `signature` with `pins` fixed — the application `Stack WITH {Elt = Number}` spells —
    /// or `signature` itself for no pins. `signature` is a declared signature, and each pin is
    /// keyed by one of its parameters.
    pub fn signature_apply<N: Naming<'t>>(
        &self,
        scratch: BumpAllocator<'_>,
        signature: N::Bound,
        pins: &[(BinderSymbol, N)],
    ) -> N {
        let signature = signature.linked(self);
        if pins.is_empty() {
            return N::read(signature);
        }
        debug_assert!(
            matches!(
                signature.view(),
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
        N::read(self.intern_digested(digest, || TypeNode::SignatureApply {
            signature,
            pins: Record::over(self.rehome_fields(pins)),
        }))
    }

    /// The meet of the signature types `members` — the set of every application they hold, less
    /// each application lying above another ([`signatures`](super::signatures)). The empty set is
    /// `Module`, and a set of one is that application's own handle.
    pub fn signature_meet<N: Naming<'t>>(&self, scratch: BumpAllocator<'_>, members: &[N]) -> N {
        let mut linked = BumpVec::with_capacity_in(members.len(), scratch);
        linked.extend(members.iter().map(|member| member.linked(self)));
        let set = canonical_applications(self, scratch, &linked);
        N::read(match set[..] {
            [] => self.enter(Handle::EMPTY_SIGNATURE),
            [only] => only,
            _ => self.intern_digested(digest::signature_meet_digest(&set), || {
                TypeNode::SignatureMeet {
                    members: Run::over(copied(self.half, &set)),
                }
            }),
        })
    }

    // --- Shapes ---

    /// The expression shape over `elements` returning `ret`, binding no group: a variable inside
    /// reads the enclosing scheme's group.
    ///
    /// `classes` is each slot's dense priority class, as
    /// [`dense_classes`](super::shape::dense_classes) normalizes a written ranking; empty for
    /// written order. Written order spelled out digests as the empty ranking, so the two spellings
    /// are one handle. The node stores each slot's class in its element, written order spelled out.
    pub fn shape_type<N: Naming<'t>>(
        &self,
        elements: &[DispatchTokenElement<N>],
        classes: &[u16],
        ret: N,
    ) -> N {
        N::read(self.shape_node(elements, classes, ret.linked(self)))
    }

    /// [`shape_type`](Self::shape_type) as a node.
    fn shape_node<N: Naming<'t>>(
        &self,
        elements: &[DispatchTokenElement<N>],
        classes: &[u16],
        ret: Linked<'t>,
    ) -> Linked<'t> {
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
        self.intern_shape(elements, classes, ret)
    }

    /// The one door that mints a quantified expression shape, its `FOR ALL` group (`quantifiers`
    /// and their `bounds`, in declaration order) put in group order: it interns the plain shape,
    /// then binds the group over it ([`bind_group`](Self::bind_group)). A
    /// [`Scheme`](super::handle::Scheme), or the plain shape where the group is empty.
    pub fn shape_scheme<'s>(
        &self,
        scratch: BumpAllocator<'s>,
        quantifiers: &[TypeSymbol],
        bounds: &[KType],
        elements: &[DispatchTokenElement<Parametric>],
        classes: &[u16],
        ret: Parametric,
    ) -> GroupIntern<'s> {
        let body = self.shape_node(elements, classes, ret.linked(self));
        self.scheme(scratch, quantifiers, bounds, body)
    }

    /// The declared type both scheme doors answer: [`bind_group`](Self::bind_group) over `body`.
    fn scheme<'s>(
        &self,
        scratch: BumpAllocator<'s>,
        quantifiers: &[TypeSymbol],
        bounds: &[KType],
        body: Linked<'t>,
    ) -> GroupIntern<'s> {
        let (node, quantifier_map) = self.bind_group(scratch, quantifiers, bounds, body);
        GroupIntern {
            handle: node.declared(),
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
        body: Linked<'t>,
    ) -> (Linked<'t>, &'s [usize]) {
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
            group: lay_one(
                self.half,
                Group {
                    names: copied(self.half, &group.names),
                    bounds: copied(self.half, &group.bounds),
                },
            ),
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
        body: Linked<'t>,
    ) -> BumpVec<'s, (Linked<'t>, Variance)> {
        let mut positions = BumpVec::new_in(scratch);
        let ret = match body.view() {
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
        positions: impl Iterator<Item = (Linked<'t>, Variance)>,
    ) -> GroupOrder<'s, 't> {
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
            (0..arity).map(|index| self.quantified_node(quantifier_map[index], bounds[index])),
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
    fn intern_shape<N: Naming<'t>>(
        &self,
        elements: &[DispatchTokenElement<N>],
        classes: &[u16],
        ret: Linked<'t>,
    ) -> Linked<'t> {
        let handle = digest::shape_digest(
            elements.iter().copied(),
            classes.iter().copied(),
            ret.digest(),
        );
        self.intern_digested(handle, || {
            // Each slot's class rides in its element, written order spelled out.
            let mut slot = 0;
            let stored = self.half.lay(elements.len(), |at| match elements[at] {
                DispatchTokenElement::Keyword(keyword) => StoredElement::Keyword(keyword),
                DispatchTokenElement::Slot(kt) => {
                    let class = class_of(classes, slot);
                    slot += 1;
                    StoredElement::Slot(kt.linked(self), class)
                }
            });
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
        positions: impl Iterator<Item = (Linked<'t>, Variance)>,
        arity: usize,
    ) -> BumpVec<'s, Occurrences> {
        let mut census = BumpVec::with_capacity_in(arity, scratch);
        census.resize(arity, Occurrences::default());
        let mut seen = 0usize;
        for (kt, position) in positions {
            visit_free_quantified(scratch, kt, position, &mut |index, context| {
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
    pub fn union_of<N: Naming<'t>>(&self, scratch: BumpAllocator<'_>, members: &[N]) -> N {
        let mut linked = BumpVec::with_capacity_in(members.len(), scratch);
        linked.extend(members.iter().map(|member| member.linked(self)));
        let width: usize = linked
            .iter()
            .map(|member| match member.content() {
                Content::Union { members: inner, .. } => inner.len(),
                _ => 1,
            })
            .sum();
        let mut flat = BumpVec::with_capacity_in(width, scratch);
        let push_unique = |node: Linked<'t>, flat: &mut BumpVec<'_, Linked<'t>>| {
            if node != Handle::NEVER && !flat.contains(&node) {
                flat.push(node);
            }
        };
        for member in linked.iter().copied() {
            match member.content() {
                Content::Union { members: inner, .. } => {
                    for nested in inner.iter() {
                        push_unique(*nested, &mut flat);
                    }
                }
                _ => push_unique(member, &mut flat),
            }
        }
        N::read(self.canonical_union(scratch, flat))
    }

    /// The canonical union over `flat`, already flattened and deduplicated with `Never` dropped.
    fn canonical_union(
        &self,
        scratch: BumpAllocator<'_>,
        mut flat: BumpVec<'_, Linked<'t>>,
    ) -> Linked<'t> {
        // The top's own definition, not the order: every type lies under `Any`, a variable too.
        if flat.iter().any(|member| *member == Handle::ANY) {
            return self.any;
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
        // a type variable bounded by `Any` would lie under `Any` but not under the union that
        // equals it.
        if [Handle::ANY_VALUE, Handle::ANY_TYPE, Handle::ANY_CODE]
            .iter()
            .all(|top| flat.iter().any(|member| *member == *top))
        {
            return self.any;
        }
        match flat.len() {
            0 => self.never,
            1 => flat[0],
            _ => self.intern_union_members(scratch, &flat),
        }
    }

    /// Intern the `Union` node over the already-canonical `flat`, probing the table before copying
    /// the members in. The `Union` arm of `node_digest` *is* `union_digest` over the node's member
    /// slice, so the digest taken here off `flat` equals the digest the node would key at.
    fn intern_union_members(&self, scratch: BumpAllocator<'_>, flat: &[Linked<'t>]) -> Linked<'t> {
        self.intern_digested(digest::union_digest(scratch, flat), || TypeNode::Union {
            members: Run::over(copied(self.half, flat)),
        })
    }

    // --- Quantifier probes ---
    //
    // The probes themselves read a node's flags ([`StoredNode`]); these are their public doors.

    /// `kt` as a [`KType`] where it is concrete — the one checked conversion from a parametric
    /// type. `None` where a variable is reachable from it. Reads the handle's concrete bit.
    pub fn concrete(&self, kt: Parametric) -> Option<KType> {
        kt.raw().is_concrete().then(|| wrap(kt.raw()))
    }

    /// `declared` as a value's type: a type narrowed to concrete through
    /// [`concrete`](Self::concrete), `None` where a variable is reachable from it, and a scheme as
    /// it is.
    pub fn concrete_declared(
        &self,
        declared: DeclaredType<Parametric>,
    ) -> Option<DeclaredType<KType>> {
        match declared {
            DeclaredType::Type(kt) => self.concrete(kt).map(DeclaredType::Type),
            DeclaredType::Scheme(scheme) => Some(DeclaredType::Scheme(scheme)),
        }
    }

    /// Whether `kt` holds an opaque carrier outside sealed content. A bound holds none, so the
    /// elaborator refuses one there.
    pub fn holds_carrier(&self, kt: KType) -> bool {
        self.enter(kt.raw()).contains_carrier()
    }

    /// Whether a type minted at run, or one built over it, can lie under or fit `kt`: at `Any`,
    /// `Value`, a record or signature type, a variable, a bare type family, a function or shape
    /// type with a parameter or slot not typed `Any`, and over a porous child at the same variance,
    /// an application's family aside.
    pub fn porous(&self, kt: impl Into<Parametric>) -> bool {
        self.enter(kt.into().raw()).is_porous()
    }

    /// Whether a value seen at `kt` can expose a part or payload of a type minted at run: at `Any`,
    /// `Value`, a kind, a signature type, a variable, a carrier, a nominal and a porous union, and
    /// over an exposing child or a porous function type.
    pub fn exposing(&self, kt: impl Into<Parametric>) -> bool {
        self.enter(kt.into().raw()).is_exposing()
    }

    /// Whether `kt` reads the `index`-th quantifier of the enclosing scheme — what a definition
    /// asks of each name its `FOR ALL` group lists.
    pub fn references_quantifier<N: Naming<'t>>(
        &self,
        scratch: BumpAllocator<'_>,
        kt: N,
        index: usize,
    ) -> bool {
        let kt = kt.linked(self);
        kt.contains_quantified()
            && visit_free_quantified(scratch, kt, Variance::Co, &mut |found, _| {
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
        let kt = self.enter(kt.raw());
        kt.contains_rigid()
            && visit(scratch, kt, &mut |_, node, _| match *node {
                TypeNode::Parameter { name: found, .. } if found == name => Visit::Stop,
                _ => Visit::Descend,
            })
    }

    /// Whether any of quantifiers `0..arity` occurs free in `kt` at a contravariant position — what
    /// a family's representation may not do, since an application is covariant in its arguments.
    pub fn quantifies_contravariantly<N: TypeHandle>(
        &self,
        scratch: BumpAllocator<'_>,
        kt: N,
        arity: usize,
    ) -> bool {
        let kt = self.enter(kt.raw());
        kt.contains_quantified()
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
        kt: Linked<'t>,
        arity: usize,
    ) -> bool {
        !visit_free_quantified(scratch, kt, Variance::Co, &mut |index, _| {
            if index >= arity {
                Visit::Stop
            } else {
                Visit::Skip
            }
        })
    }

    // --- Verdicts ---

    /// The verdict recorded for the pair: over two load types, in the frozen load's table, then
    /// the agent's — on the load's chain, the builder's alone — and over any other pair in the own
    /// tier's, the region tier's, then the agent's.
    pub(super) fn verdict(
        &self,
        subject: Handle,
        candidate: Handle,
        relation: Relation,
    ) -> Option<bool> {
        let key = VerdictKey {
            subject: subject.digest(),
            candidate: candidate.digest(),
            relation,
        };
        if subject.in_root() && candidate.in_root() {
            let Some(frozen) = self.frozen else {
                return self.half.own()?.verdict(key);
            };
            self.count_load_probe();
            return frozen.verdict(key).or_else(|| self.agent_verdict(key));
        }
        self.half
            .own()
            .and_then(|own| {
                self.count_branch_verdict_probe();
                own.verdict(key)
            })
            .or_else(|| {
                self.half.region().and_then(|region| {
                    self.count_branch_verdict_probe();
                    region.tier().verdict(key)
                })
            })
            .or_else(|| self.agent?.verdict(key))
    }

    /// The agent's verdict for `key`: its tier's, or the own tier's on the chain that writes it.
    fn agent_verdict(&self, key: VerdictKey) -> Option<bool> {
        match self.agent {
            Some(agent) => agent.verdict(key),
            None => self.half.own()?.verdict(key),
        }
    }

    /// Record `verdict` for the pair in the shortest-lived tier holding one of its two types, as
    /// [`tier_of`](Self::tier_of) places each. A type it finds in no tier — a child of a node read
    /// through a link — counts as the own tier's: the shortest-lived the chain writes. A pair over
    /// two load types at the run is the agent's, since no run writes the load's table.
    pub(super) fn record_verdict(
        &self,
        subject: Handle,
        candidate: Handle,
        relation: Relation,
        verdict: bool,
    ) {
        let key = VerdictKey {
            subject: subject.digest(),
            candidate: candidate.digest(),
            relation,
        };
        match self.tier_of(subject).max(self.tier_of(candidate)) {
            Placed::Load | Placed::Agent => match self.agent {
                Some(agent) => agent.record(key, verdict),
                // The resting chain, whose own tier is the agent's.
                None => self.half.own_or_lay().record(self.half, key, verdict),
            },
            Placed::Region => {
                if let Some(region) = self.half.region() {
                    region.tier().record(key, verdict);
                }
            }
            Placed::Own => self.half.own_or_lay().record(self.half, key, verdict),
        }
    }

    /// `test`-only: how many verdicts the frozen load, or the root builder, holds.
    #[cfg(test)]
    pub(super) fn load_verdict_count(&self) -> usize {
        match (self.frozen, self.half.own()) {
            (Some(frozen), _) => frozen.verdict_count(),
            (None, Some(Own::Builder(builder))) => builder.verdict_count(),
            (None, _) => unreachable!("a chain with no frozen load writes the root builder"),
        }
    }
}

/// A quantifier group put in group order over the positions it binds — what both interning doors
/// hand their own positions and names through.
struct GroupOrder<'s, 't> {
    /// The quantifiers' names, in group order.
    names: BumpVec<'s, TypeSymbol>,
    /// Each quantifier's bound, in the same order.
    bounds: BumpVec<'s, KType>,
    /// What each **declared** variable substitutes to: its `Quantified` at its group index.
    bindings: BumpVec<'s, Linked<'t>>,
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

/// Member `index` of `component` as stored content.
pub(super) fn member_content<'t>(component: &'t Component<'t>, index: usize) -> Content<'t> {
    let member = component.members()[index];
    Content::SetMember {
        kind: member.kind,
        index: u32::try_from(index).expect("a component has fewer than 2^32 members"),
        component,
        name: member.name,
    }
}
