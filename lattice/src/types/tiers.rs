//! The tiers a type lives in: the [`Frozen`] load every agent shares, the [`Agent`] tier one agent
//! lays above it, and a [`Branch`] per habitat half the run builds in. A [`TypeChain`] reads and
//! writes them.
//!
//! - The **frozen load** holds the load's types and outlives every run. While the load runs, its
//!   [`RootBuilder`] grows a hashbrown table and a lossless verdict table in the bump it is laid in;
//!   the load's end ([`RootBuilder::freeze`]) lays both into [`FrozenTable`]s, runs of slots that
//!   are never written. The frozen load has no intern door and no record door, holds no cell, and
//!   so is `Sync`; it is covariant, so a chain holds it at its own lifetime.
//! - The **agent tier** holds what one agent rests above the load: insert-only nodes laid alone in
//!   its bump, which never move, indexed by a hashbrown table there, and a lossless verdict table.
//!   It is released whole at the agent's death.
//! - A **branch** is a [`DigestTable`] of the nodes one habitat half — a cell region or a scratch
//!   habitat — interned, laid in that half at its smallest size by the first door that needs it,
//!   and released with the half. Its verdicts are a second table, laid at its first verdict. A
//!   branch written through shared borrows is invariant in its lifetime.
//!
//! A branch may **link** one other tier, read at a brand of its own through the object-safe
//! [`Tier`]: a spawned child's branch links its spawner's ([`BranchTier`]), which the pin that
//! carried the child's birth keeps alive. A chain over a scratch habitat reads its region's
//! branch the same way ([`RegionTier`]), held by value in the chain's [`Halves`] and re-reading the
//! region's header on every probe so a branch the region lays during the step is seen. A branch
//! never holds a region tier: the region's door is valid for one step, a branch for its half's
//! life. A link never cycles: it points at a spawner, linked
//! before its child existed, or at a region a scratch branch is linked to late, which reaches no
//! scratch branch. A branch records its link depth and its chain's bottom as it is linked, which a
//! merge's reach check reads ([transfer.rs](super::transfer)); so a branch is linked late only
//! where nothing links it, which it records too, and nothing has recorded its stamps.
//!
//! How a chain reaches the tier it writes is the [`Habitat`] it is built over: a [`LaysRuns`]
//! with the half's own tier ([`Own`]) beside it. Koan implements it for a region's writer beside
//! its header door; the root builder implements it for the load; `BumpHalf` is the test suite's.
//!
//! See [identity.md § Storage](identity.md#storage-a-root-and-its-branches).

use std::cell::{Cell, RefCell};
use std::hash::BuildHasher;
use std::marker::PhantomData;

use crate::bump::{
    BumpAllocator, BumpHashTable, DigestTable, FrozenTable, Keyed, LaysRuns, bump_hash_table,
};
use crate::symbols::IdentityBuildHasher;

use super::chain::TypeChain;
use super::digest::{TypeDigest, node_digest};
use super::node::TypeNode;
use super::stored::{Content, Flags, Linked, StoredNode};
use super::verdicts::{VerdictEntry, VerdictKey};

/// The root builder's node table: a reference to each stored node, bucket array in its bump.
/// A node holds its own digest, so the table holds no second copy of it.
type NodeTable<'g> = BumpHashTable<'g, Linked<'g>>;

/// The table's hash of `digest`: the identity hash every digest-keyed map runs.
fn slot_hash(digest: TypeDigest) -> u64 {
    IdentityBuildHasher::default().hash_one(digest.0)
}

/// Lay `value` alone through `lay`.
pub(super) fn lay_one<'r, T>(lay: impl LaysRuns<'r>, value: T) -> &'r T {
    let mut value = Some(value);
    &lay.lay(1, |_| value.take().expect("a run of one fills once"))[0]
}

/// The node under `digest` in `nodes`, or `None`.
fn find_in<'g>(nodes: &NodeTable<'g>, digest: TypeDigest) -> Option<Linked<'g>> {
    nodes
        .find(slot_hash(digest), |node| node.digest() == digest)
        .copied()
}

impl Keyed for StoredNode<'_> {
    type Key = TypeDigest;

    fn key(&self) -> TypeDigest {
        self.digest()
    }

    fn word(key: TypeDigest) -> u64 {
        key.0 as u64
    }
}

/// A tier grown through shared borrows in the bump it is laid in: a hashbrown node table, whose
/// miss is one group probe, and a verdict table laid at the first verdict, which keeps every
/// verdict it is given. The root builder's storage, and the agent tier's.
struct Grown<'g> {
    bump: BumpAllocator<'g>,
    nodes: RefCell<NodeTable<'g>>,
    verdicts: Cell<Option<&'g DigestTable<'g, VerdictEntry>>>,
}

impl<'g> Grown<'g> {
    fn new(bump: BumpAllocator<'g>, nodes: NodeTable<'g>) -> Self {
        Grown {
            bump,
            nodes: RefCell::new(nodes),
            verdicts: Cell::new(None),
        }
    }

    fn find(&self, digest: TypeDigest) -> Option<Linked<'g>> {
        find_in(&self.nodes.borrow(), digest)
    }

    fn insert(&self, node: Linked<'g>) {
        self.nodes
            .borrow_mut()
            .insert_unique(slot_hash(node.digest()), node, |node| {
                slot_hash(node.digest())
            });
    }

    fn len(&self) -> usize {
        self.nodes.borrow().len()
    }

    fn verdict(&self, key: VerdictKey) -> Option<bool> {
        self.verdicts.get()?.get(key).map(VerdictEntry::verdict)
    }

    fn record(&self, key: VerdictKey, verdict: bool) {
        record_in(&self.verdicts, self.bump, key, verdict);
    }

    #[cfg(any(test, feature = "counters"))]
    fn verdict_count(&self) -> usize {
        self.verdicts.get().map_or(0, DigestTable::len)
    }

    fn each_digest(&self, mut each: impl FnMut(TypeDigest)) {
        for node in self.nodes.borrow().iter() {
            each(node.digest());
        }
    }
}

/// Record `verdict` in `verdicts` unless it holds one for `key`, laying the table through `lay` on
/// the first.
fn record_in<'t>(
    verdicts: &Cell<Option<&'t DigestTable<'t, VerdictEntry>>>,
    lay: impl LaysRuns<'t>,
    key: VerdictKey,
    verdict: bool,
) {
    let table = match verdicts.get() {
        Some(table) => table,
        None => {
            let table = lay_one(lay, DigestTable::new(lay));
            verdicts.set(Some(table));
            table
        }
    };
    if table.get(key).is_none() {
        table.insert_unique(lay, lay_one(lay, VerdictEntry::new(key, verdict)));
    }
}

/// The load's tier while the load builds it: a hashbrown node table and a lossless verdict table,
/// grown in the bump it is laid in. The load's chain writes it; [`freeze`](Self::freeze) ends the
/// load.
pub struct RootBuilder<'g> {
    grown: Grown<'g>,
    never: Linked<'g>,
    any: Linked<'g>,
    frozen: Cell<bool>,
}

impl<'g> RootBuilder<'g> {
    /// A builder over `bump`, laid there, seeded with the fixed handles — the leaves, the `OfKind`
    /// values, `List<Any>`, `Dict<Any, Any>`, the empty signature and the code composites — so the
    /// constants those names lower to resolve in a load that holds nothing else.
    pub fn in_region(bump: BumpAllocator<'g>) -> &'g Self {
        let mut nodes = bump_hash_table(bump);
        let mut lay_leaf = |leaf: TypeNode<'g, Linked<'g>>| -> Linked<'g> {
            let digest = node_digest(bump, &leaf);
            let node = bump.alloc(StoredNode::new(
                digest,
                Content::lay(leaf, Flags::NONE, bump),
                true,
            ));
            nodes.insert_unique(slot_hash(digest), &*node, |node| slot_hash(node.digest()));
            node
        };
        let (never, any) = (lay_leaf(TypeNode::Never), lay_leaf(TypeNode::Any));
        let builder = bump.alloc(RootBuilder {
            grown: Grown::new(bump, nodes),
            never,
            any,
            frozen: Cell::new(false),
        });
        TypeChain::building(&*builder).seed_constants(bump);
        builder
    }

    /// End the load: lay its nodes and its verdicts into the [`Frozen`] load's read-only tables,
    /// each slot placed as a digest table places it, and lay the load in the bump. The staging
    /// table the nodes pass through is stranded there. An insert after the freeze panics.
    pub fn freeze(&self) -> &'g Frozen<'g> {
        assert!(!self.frozen.replace(true), "a load freezes once");
        let bump = self.grown.bump;
        let staging = DigestTable::with_capacity(bump, self.grown.len());
        for &node in self.grown.nodes.borrow().iter() {
            staging.insert_unique(bump, node);
        }
        let verdicts = match self.grown.verdicts.get() {
            Some(table) => table.frozen(bump),
            None => DigestTable::new(bump).frozen(bump),
        };
        bump.alloc(Frozen {
            nodes: staging.frozen(bump),
            verdicts,
            never: self.never,
            any: self.any,
        })
    }

    pub(super) fn never(&self) -> Linked<'g> {
        self.never
    }

    pub(super) fn any(&self) -> Linked<'g> {
        self.any
    }

    pub(super) fn find(&self, digest: TypeDigest) -> Option<Linked<'g>> {
        self.grown.find(digest)
    }

    pub(super) fn insert(&self, node: Linked<'g>) {
        assert!(!self.frozen.get(), "a frozen load takes no intern");
        self.grown.insert(node);
    }

    pub(super) fn len(&self) -> usize {
        self.grown.len()
    }

    pub(super) fn verdict(&self, key: VerdictKey) -> Option<bool> {
        self.grown.verdict(key)
    }

    pub(super) fn record(&self, key: VerdictKey, verdict: bool) {
        self.grown.record(key, verdict);
    }

    /// `test`-only: how many verdicts the builder holds.
    #[cfg(test)]
    pub(super) fn verdict_count(&self) -> usize {
        self.grown.verdict_count()
    }
}

impl<'g> LaysRuns<'g> for &'g RootBuilder<'g> {
    fn lay<T>(self, len: usize, fill: impl FnMut(usize) -> T) -> &'g [T] {
        self.grown.bump.lay(len, fill)
    }

    fn text(self, text: &str) -> &'g str {
        self.grown.bump.text(text)
    }
}

impl<'g> Habitat<'g> for &'g RootBuilder<'g> {
    fn own(self) -> Option<Own<'g>> {
        Some(Own::Builder(self))
    }

    fn own_or_lay(self) -> Own<'g> {
        Own::Builder(self)
    }
}

/// The frozen load: its node index and its verdict table, each a run of (digest word, entry) slots
/// laid at the freeze and never written. It has no intern door and no record door, so no run writes
/// it. It holds no cell, so every agent may read one load from its own thread; and it is
/// covariant in `'g`, so a chain holds it at its own lifetime.
pub struct Frozen<'g> {
    nodes: FrozenTable<'g, StoredNode<'g>>,
    verdicts: FrozenTable<'g, VerdictEntry>,
    never: Linked<'g>,
    any: Linked<'g>,
}

const _: () = assert!(!std::mem::needs_drop::<Frozen<'static>>());
const _: fn() = || {
    fn sync<T: Sync>() {}
    sync::<Frozen<'static>>();
};

impl<'g> Frozen<'g> {
    /// A load over `bump` holding the fixed handles and nothing else: a builder seeded and frozen
    /// at once. What a test that needs no load builds its chains over.
    pub fn seeded(bump: BumpAllocator<'g>) -> &'g Self {
        RootBuilder::in_region(bump).freeze()
    }

    /// How many nodes the load holds. Tests read it — koan's among them, which is why it is not
    /// `cfg(test)`: that does not reach a dependency.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Every digest the load holds, in no particular order. Tests read it — koan's among them.
    pub fn each_digest(&self, mut each: impl FnMut(TypeDigest)) {
        self.nodes.each(|node| each(node.digest()));
    }

    /// Whether the load holds no node; never, since `Never` and `Any` are laid before it exists.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub(super) fn never(&self) -> Linked<'g> {
        self.never
    }

    pub(super) fn any(&self) -> Linked<'g> {
        self.any
    }

    pub(super) fn find(&self, digest: TypeDigest) -> Option<Linked<'g>> {
        self.nodes.get(digest)
    }

    pub(super) fn verdict(&self, key: VerdictKey) -> Option<bool> {
        self.verdicts.get(key).map(VerdictEntry::verdict)
    }

    /// `test`-only: how many verdicts the load holds.
    #[cfg(test)]
    pub(super) fn verdict_count(&self) -> usize {
        self.verdicts.len()
    }
}

/// The agent tier: the nodes and verdicts one agent lays above the frozen load, insert-only, in a
/// bump released whole at the agent's death. A node is laid alone and entered in a hashbrown index,
/// whose miss is one group probe, since every intern that misses the load probes here next; it
/// never moves, and nothing here is released before the tier. Its verdicts are a [`DigestTable`],
/// as a branch's are. A chain reads it through the object-safe [`AgentTier`], at the chain's own
/// brand, since a tier written through shared borrows is invariant in its lifetime.
pub struct Agent<'a> {
    grown: Grown<'a>,
}

impl<'a> Agent<'a> {
    /// An empty agent tier over `bump`, laid there.
    pub fn in_region(bump: BumpAllocator<'a>) -> &'a Self {
        bump.alloc(Agent {
            grown: Grown::new(bump, bump_hash_table(bump)),
        })
    }

    /// How many nodes the tier holds. Tests read it — koan's among them.
    pub fn len(&self) -> usize {
        self.grown.len()
    }

    /// Whether the tier holds no node.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Every digest the tier holds, in no particular order. Tests read it — koan's among them.
    pub fn each_digest(&self, each: impl FnMut(TypeDigest)) {
        self.grown.each_digest(each);
    }

    /// How many verdicts the tier holds. Tests read it — koan's among them, through the `counters`
    /// feature, since `cfg(test)` does not reach a dependency.
    #[cfg(any(test, feature = "counters"))]
    pub fn verdict_count(&self) -> usize {
        self.grown.verdict_count()
    }
}

impl<'a> LaysRuns<'a> for &'a Agent<'a> {
    fn lay<T>(self, len: usize, fill: impl FnMut(usize) -> T) -> &'a [T] {
        self.grown.bump.lay(len, fill)
    }

    fn text(self, text: &str) -> &'a str {
        self.grown.bump.text(text)
    }
}

impl<'a> Habitat<'a> for &'a Agent<'a> {
    fn own(self) -> Option<Own<'a>> {
        Some(Own::Agent(self))
    }

    fn own_or_lay(self) -> Own<'a> {
        Own::Agent(self)
    }
}

/// The agent tier as a chain reads it, at the chain's brand.
pub(super) trait AgentTier<'t> {
    /// The node under `digest` in the tier.
    fn find(&self, digest: TypeDigest) -> Option<Linked<'t>>;

    /// The tier's recorded verdict for `key`.
    fn verdict(&self, key: VerdictKey) -> Option<bool>;

    /// Record `verdict` for `key` in the tier.
    fn record(&self, key: VerdictKey, verdict: bool);
}

impl<'a: 't, 't> AgentTier<'t> for Agent<'a> {
    fn find(&self, digest: TypeDigest) -> Option<Linked<'t>> {
        self.grown.find(digest)
    }

    fn verdict(&self, key: VerdictKey) -> Option<bool> {
        self.grown.verdict(key)
    }

    fn record(&self, key: VerdictKey, verdict: bool) {
        self.grown.record(key, verdict);
    }
}

/// A branch's position on its link chain, which is what a reach check compares: its address, how
/// many links its chain holds above it, and its chain's bottom's [tag](self::bottom_tag).
#[derive(Clone, Copy)]
pub(super) struct ChainPosition {
    pub address: *const (),
    pub depth: u32,
    pub bottom: u32,
}

/// A chain's bottom as a branch records it: 32 bits of its address, so both stamps fit in a word.
/// Two chains whose tags differ share no branch; two whose tags match may, and the check walks
/// down to compare addresses, so a collision costs steps, never a wrong answer.
fn bottom_tag(address: *const ()) -> u32 {
    // A branch is word-aligned, so its low bits name nothing.
    (address.addr() >> 3) as u32
}

/// The types one habitat half interned at the run: a [`DigestTable`] of its nodes laid in the
/// half, the verdicts recorded over them, laid at the first, and the tier it links, if its cell was
/// spawned by one whose habitat held a branch. Laid by the first door that interns into the half,
/// released with the half, and never dropped.
pub struct Branch<'t> {
    nodes: DigestTable<'t, StoredNode<'t>>,
    verdicts: Cell<Option<&'t DigestTable<'t, VerdictEntry>>>,
    link: Cell<Option<&'t dyn Tier<'t>>>,
    /// How many links the branch's chain holds above it: 0 where it links nothing, one more than
    /// its link's otherwise.
    depth: Cell<u32>,
    /// The [tag](self::bottom_tag) of the chain's last branch, the one that links nothing; unread at
    /// depth 0, where the branch is its own bottom, since a branch moves after [`Branch::new`].
    bottom: Cell<u32>,
    /// Whether another branch links this one, and so has recorded its depth and bottom: what keeps
    /// a late link from changing stamps another branch holds.
    linked: Cell<bool>,
    /// How many reach steps the merges into the branch have taken.
    #[cfg(any(test, feature = "counters"))]
    reach_steps: Cell<usize>,
}

const _: () = assert!(!std::mem::needs_drop::<Branch<'static>>());

impl<'t> Branch<'t> {
    /// An empty branch on the smallest table, laid through `lay`.
    pub fn new(lay: impl LaysRuns<'t>) -> Self {
        Branch {
            nodes: DigestTable::new(lay),
            verdicts: Cell::new(None),
            link: Cell::new(None),
            depth: Cell::new(0),
            bottom: Cell::new(0),
            linked: Cell::new(false),
            #[cfg(any(test, feature = "counters"))]
            reach_steps: Cell::new(0),
        }
    }

    /// How many nodes the branch holds — its own and the ones it cached through its link.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Whether the branch holds no node.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// The node table's length in slots.
    pub fn slots(&self) -> usize {
        self.nodes.slots()
    }

    /// How many verdicts the branch holds. Tests read it — koan's among them, through the
    /// `counters` feature, since `cfg(test)` does not reach a dependency.
    #[cfg(any(test, feature = "counters"))]
    pub fn verdict_count(&self) -> usize {
        self.verdicts.get().map_or(0, |table| table.len())
    }

    /// Every digest the branch holds, in slot order. Tests read it — koan's among them.
    pub fn each_digest(&self, mut each: impl FnMut(TypeDigest)) {
        self.nodes.each(|node| each(node.digest()));
    }

    /// The branch's own node under `digest`, its link unread.
    pub(super) fn find(&self, digest: TypeDigest) -> Option<Linked<'t>> {
        self.nodes.get(digest)
    }

    /// The node under `digest` in the branch, else through its link, recursively.
    pub(super) fn reach(&self, digest: TypeDigest) -> Option<Linked<'t>> {
        self.find(digest).or_else(|| self.link_reach(digest))
    }

    /// The node under `digest` through the branch's link alone.
    pub(super) fn link_reach(&self, digest: TypeDigest) -> Option<Linked<'t>> {
        self.link.get()?.reach(digest)
    }

    /// Insert `node`, whose digest the branch does not hold, growing through `lay`.
    pub(super) fn insert(&self, lay: impl LaysRuns<'t>, node: Linked<'t>) {
        self.nodes.insert_unique(lay, node);
    }

    /// The branch's address, which names it whatever its brand.
    pub(super) fn address(&self) -> *const () {
        (self as *const Self).cast()
    }

    /// How many reach steps the merges into the branch have taken, summed; a merge into a half that
    /// ends it with no branch counts nowhere. Tests read it — koan's among them, through the
    /// `counters` feature, since `cfg(test)` does not reach a dependency.
    #[cfg(any(test, feature = "counters"))]
    pub fn reach_steps(&self) -> usize {
        self.reach_steps.get()
    }

    /// Add a merge's reach steps to the branch's count.
    #[cfg(any(test, feature = "counters"))]
    pub(super) fn count_reach_steps(&self, steps: usize) {
        self.reach_steps.set(self.reach_steps.get() + steps);
    }

    /// The branch's position on its link chain.
    pub(super) fn position(&self) -> ChainPosition {
        ChainPosition {
            address: self.address(),
            depth: self.depth.get(),
            bottom: self.bottom(),
        }
    }

    /// The tag of the chain's bottom.
    fn bottom(&self) -> u32 {
        match self.depth.get() {
            0 => bottom_tag(self.address()),
            _ => self.bottom.get(),
        }
    }

    /// Whether the branch is the one at `at`, or reaches it through its links, walking its chain
    /// only as far as `at`'s depth, and not at all where `at` lies on another chain. Counts one
    /// step in `steps` per branch it reads.
    pub(super) fn reaches_at(&self, at: ChainPosition, steps: &Cell<usize>) -> bool {
        steps.set(steps.get() + 1);
        let depth = self.depth.get();
        if self.bottom() != at.bottom || depth < at.depth {
            false
        } else if depth == at.depth {
            self.address() == at.address
        } else {
            self.link
                .get()
                .is_some_and(|link| link.reaches_at(at, steps))
        }
    }

    /// Whether the branch is the one at `address`, or reaches it through its links: the full walk
    /// a merge's debug check compares [`reaches_at`](Self::reaches_at) against.
    pub(super) fn reaches_branch(&self, address: *const ()) -> bool {
        self.address() == address
            || self
                .link
                .get()
                .is_some_and(|link| link.reaches_branch(address))
    }

    /// Visit the nodes of the branch, then of each branch its links reach, nearest first, stopping
    /// at the first branch `reached` answers true for.
    pub(super) fn each_until(
        &self,
        reached: &dyn Fn(ChainPosition) -> bool,
        visit: &mut dyn FnMut(Linked<'t>),
    ) {
        if reached(self.position()) {
            return;
        }
        self.nodes.each(&mut *visit);
        if let Some(link) = self.link.get() {
            link.each_until(reached, visit);
        }
    }

    /// Link `tier`, the branch at `home`, taking the depth and bottom it gives. Called once: as the
    /// branch is laid, or late, before any branch links it, so a branch's depth never changes once
    /// another links it.
    pub(super) fn set_link(&self, tier: &'t dyn Tier<'t>, home: ChainPosition) {
        assert!(
            self.link.replace(Some(tier)).is_none(),
            "a branch links once"
        );
        self.depth.set(home.depth + 1);
        self.bottom.set(home.bottom);
    }

    /// Record that another branch links this one.
    pub(super) fn mark_linked(&self) {
        self.linked.set(true);
    }

    /// Whether another branch links this one.
    pub(super) fn is_linked(&self) -> bool {
        self.linked.get()
    }

    /// Whether the branch links anything.
    pub(super) fn links_any(&self) -> bool {
        self.link.get().is_some()
    }

    /// Whether the branch links the branch at `home` itself: one link above it, on its chain.
    pub(super) fn links(&self, home: ChainPosition) -> bool {
        self.depth.get() == home.depth + 1
            && (self.link.get()).is_some_and(|link| link.reaches_at(home, &Cell::new(0)))
    }

    pub(super) fn verdict(&self, key: VerdictKey) -> Option<bool> {
        self.verdicts.get()?.get(key).map(VerdictEntry::verdict)
    }

    /// Record `verdict` unless the branch holds one for `key`, laying the verdict table through
    /// `lay` on the first.
    pub(super) fn record(&self, lay: impl LaysRuns<'t>, key: VerdictKey, verdict: bool) {
        record_in(&self.verdicts, lay, key, verdict);
    }
}

/// The tier a chain writes: the root builder during the load, a habitat half's branch at the run,
/// or the agent tier on the chain that lays into it. A chain reaches it through its [`Habitat`] and
/// never matches on it.
#[derive(Clone, Copy)]
pub enum Own<'t> {
    Builder(&'t RootBuilder<'t>),
    Branch(&'t Branch<'t>),
    Agent(&'t Agent<'t>),
}

impl<'t> Own<'t> {
    pub(super) fn find(self, digest: TypeDigest) -> Option<Linked<'t>> {
        match self {
            Own::Builder(builder) => builder.find(digest),
            Own::Branch(branch) => branch.find(digest),
            Own::Agent(agent) => agent.grown.find(digest),
        }
    }

    pub(super) fn reach(self, digest: TypeDigest) -> Option<Linked<'t>> {
        match self {
            Own::Builder(builder) => builder.find(digest),
            Own::Branch(branch) => branch.reach(digest),
            Own::Agent(agent) => agent.grown.find(digest),
        }
    }

    pub(super) fn link_reach(self, digest: TypeDigest) -> Option<Linked<'t>> {
        match self {
            Own::Builder(_) | Own::Agent(_) => None,
            Own::Branch(branch) => branch.link_reach(digest),
        }
    }

    pub(super) fn insert(self, lay: impl LaysRuns<'t>, node: Linked<'t>) {
        match self {
            Own::Builder(builder) => builder.insert(node),
            Own::Branch(branch) => branch.insert(lay, node),
            Own::Agent(agent) => agent.grown.insert(node),
        }
    }

    pub(super) fn len(self) -> usize {
        match self {
            Own::Builder(builder) => builder.len(),
            Own::Branch(branch) => branch.len(),
            Own::Agent(agent) => agent.len(),
        }
    }

    pub(super) fn verdict(self, key: VerdictKey) -> Option<bool> {
        match self {
            Own::Builder(builder) => builder.verdict(key),
            Own::Branch(branch) => branch.verdict(key),
            Own::Agent(agent) => agent.grown.verdict(key),
        }
    }

    pub(super) fn record(self, lay: impl LaysRuns<'t>, key: VerdictKey, verdict: bool) {
        match self {
            Own::Builder(builder) => builder.record(key, verdict),
            Own::Branch(branch) => branch.record(lay, key, verdict),
            Own::Agent(agent) => agent.grown.record(key, verdict),
        }
    }

    /// [`Branch::reaches_at`] over the tier; the root builder and the agent tier reach no branch.
    pub(super) fn reaches_at(self, at: ChainPosition, steps: &Cell<usize>) -> bool {
        match self {
            Own::Builder(_) | Own::Agent(_) => false,
            Own::Branch(branch) => branch.reaches_at(at, steps),
        }
    }

    /// [`Branch::reaches_branch`] over the tier; the root builder and the agent tier reach no
    /// branch.
    pub(super) fn reaches_branch(self, address: *const ()) -> bool {
        match self {
            Own::Builder(_) | Own::Agent(_) => false,
            Own::Branch(branch) => branch.reaches_branch(address),
        }
    }

    /// The branch, for a door only a branch serves.
    pub(super) fn branch(self) -> &'t Branch<'t> {
        match self {
            Own::Builder(_) => panic!("the root builder is not a branch"),
            Own::Agent(_) => panic!("the agent tier is not a branch"),
            Own::Branch(branch) => branch,
        }
    }
}

/// What a chain is built over: the runs it lays its content through, and the half's own tier
/// beside them, re-read on every call so a tier laid since is seen.
pub trait Habitat<'t>: LaysRuns<'t> {
    /// The half's own tier, or `None` while nothing has laid its branch.
    fn own(self) -> Option<Own<'t>>;

    /// The half's own tier, laying its branch ([`Branch::new`]) if none is laid.
    fn own_or_lay(self) -> Own<'t>;

    /// The region tier a scratch chain reads between the root and its own: `None` but for
    /// [`Halves`] over a region.
    fn region(&self) -> Option<RegionRead<'_, 't>> {
        None
    }
}

/// A region tier as [`Habitat::region`] hands it to the chain.
#[derive(Clone, Copy)]
pub struct RegionRead<'a, 't>(&'a dyn Tier<'t>);

impl<'a, 't> RegionRead<'a, 't> {
    pub(super) fn tier(self) -> &'a dyn Tier<'t> {
        self.0
    }
}

/// What a scratch habitat's chain is built over: the half it writes, and the region whose tier it
/// reads between the root and its own, held by value since a region's door lives one step. `None`
/// for a half read alone, so a caller may hold one type for both. `'r: 't` is declared, so a
/// function handed a chain over one may read the region at the chain's brand.
#[derive(Clone, Copy)]
pub struct Halves<'r, 't, H, R>
where
    'r: 't,
{
    half: H,
    region: Option<RegionTier<'r, R>>,
    brand: PhantomData<&'t ()>,
}

impl<'t, H> Halves<'t, 't, H, H> {
    /// `half` alone. The region's type is the half's own, so no brand is left free.
    pub fn alone(half: H) -> Self {
        Halves {
            half,
            region: None,
            brand: PhantomData,
        }
    }
}

impl<'r: 't, 't, H, R> Halves<'r, 't, H, R> {
    /// `half`, reading `region` between the root and its own tier.
    pub fn over(half: H, region: R) -> Self {
        Halves {
            half,
            region: Some(RegionTier(region, PhantomData)),
            brand: PhantomData,
        }
    }

    /// The half the chain writes.
    pub fn half(self) -> H {
        self.half
    }
}

impl<'r: 't, 't, H: Habitat<'t>, R: Habitat<'r>> LaysRuns<'t> for Halves<'r, 't, H, R> {
    fn lay<T>(self, len: usize, fill: impl FnMut(usize) -> T) -> &'t [T] {
        self.half.lay(len, fill)
    }

    fn text(self, text: &str) -> &'t str {
        self.half.text(text)
    }
}

impl<'r: 't, 't, H: Habitat<'t>, R: Habitat<'r>> Habitat<'t> for Halves<'r, 't, H, R> {
    fn own(self) -> Option<Own<'t>> {
        self.half.own()
    }

    fn own_or_lay(self) -> Own<'t> {
        self.half.own_or_lay()
    }

    fn region(&self) -> Option<RegionRead<'_, 't>> {
        Some(RegionRead(self.region.as_ref()?))
    }
}

/// A tier a chain reads at a brand other than its own: a scratch chain's region, or the spawner's
/// branch a branch links. Object-safe, so a chain names neither brand.
pub(super) trait Tier<'t> {
    /// The node under `digest` in the tier's own table.
    fn find(&self, digest: TypeDigest) -> Option<Linked<'t>>;

    /// The node under `digest` in the tier's own table, else through its link, recursively.
    fn reach(&self, digest: TypeDigest) -> Option<Linked<'t>>;

    /// The tier's recorded verdict for `key`.
    fn verdict(&self, key: VerdictKey) -> Option<bool>;

    /// Record `verdict` for `key` in the tier.
    fn record(&self, key: VerdictKey, verdict: bool);

    /// [`Branch::reaches_at`] over the tier's branch.
    fn reaches_at(&self, at: ChainPosition, steps: &Cell<usize>) -> bool;

    /// [`Branch::reaches_branch`] over the tier's branch.
    fn reaches_branch(&self, address: *const ()) -> bool;

    /// [`Branch::each_until`] over the tier's branch, which a link always is.
    fn each_until(
        &self,
        reached: &dyn Fn(ChainPosition) -> bool,
        visit: &mut dyn FnMut(Linked<'t>),
    );
}

/// A scratch chain's region tier: the region half itself, so every call re-reads the region's
/// tier and a branch the region lays during the step is seen. Held by value in [`Halves`].
#[derive(Clone, Copy)]
pub(super) struct RegionTier<'r, H>(pub H, pub PhantomData<&'r ()>);

impl<'r: 't, 't, H: Habitat<'r>> Tier<'t> for RegionTier<'r, H> {
    fn find(&self, digest: TypeDigest) -> Option<Linked<'t>> {
        self.0.own()?.find(digest)
    }

    fn reach(&self, digest: TypeDigest) -> Option<Linked<'t>> {
        self.0.own()?.reach(digest)
    }

    fn verdict(&self, key: VerdictKey) -> Option<bool> {
        self.0.own()?.verdict(key)
    }

    fn record(&self, key: VerdictKey, verdict: bool) {
        if let Some(own) = self.0.own() {
            own.record(self.0, key, verdict);
        }
    }

    fn reaches_at(&self, at: ChainPosition, steps: &Cell<usize>) -> bool {
        self.0.own().is_some_and(|own| own.reaches_at(at, steps))
    }

    fn reaches_branch(&self, address: *const ()) -> bool {
        self.0.own().is_some_and(|own| own.reaches_branch(address))
    }

    fn each_until(
        &self,
        reached: &dyn Fn(ChainPosition) -> bool,
        visit: &mut dyn FnMut(Linked<'t>),
    ) {
        if let Some(Own::Branch(branch)) = self.0.own() {
            branch.each_until(reached, &mut |node| visit(node));
        }
    }
}

/// A branch's link to its spawner's branch, which existed when the link was laid. A chain reads
/// a link's nodes only: verdicts are read from the chain's own tiers, and none is placed in a
/// linked one.
#[derive(Clone, Copy)]
pub(super) struct BranchTier<'h>(pub &'h Branch<'h>);

impl<'h: 't, 't> Tier<'t> for BranchTier<'h> {
    fn find(&self, digest: TypeDigest) -> Option<Linked<'t>> {
        self.0.find(digest)
    }

    fn reach(&self, digest: TypeDigest) -> Option<Linked<'t>> {
        self.0.reach(digest)
    }

    fn verdict(&self, _: VerdictKey) -> Option<bool> {
        None
    }

    fn record(&self, _: VerdictKey, _: bool) {}

    fn reaches_at(&self, at: ChainPosition, steps: &Cell<usize>) -> bool {
        self.0.reaches_at(at, steps)
    }

    fn reaches_branch(&self, address: *const ()) -> bool {
        self.0.reaches_branch(address)
    }

    fn each_until(
        &self,
        reached: &dyn Fn(ChainPosition) -> bool,
        visit: &mut dyn FnMut(Linked<'t>),
    ) {
        self.0.each_until(reached, &mut |node| visit(node));
    }
}

/// `test`-only: a habitat half over a bump, its branch in a slot beside it — how the suite builds
/// branch chains without a cell graph.
#[cfg(test)]
#[derive(Clone, Copy)]
pub(crate) struct BumpHalf<'t> {
    bump: BumpAllocator<'t>,
    slot: &'t Cell<Option<&'t Branch<'t>>>,
}

#[cfg(test)]
impl<'t> BumpHalf<'t> {
    /// A half over `bump`, its slot laid there, holding no branch yet.
    pub(crate) fn new(bump: BumpAllocator<'t>) -> Self {
        BumpHalf {
            bump,
            slot: bump.alloc(Cell::new(None)),
        }
    }

    /// The half's branch, if one is laid.
    pub(crate) fn branch(self) -> Option<&'t Branch<'t>> {
        self.slot.get()
    }

    /// The branch `chain` writes, which must be laid.
    pub(crate) fn branch_of(chain: &TypeChain<'t, Self>) -> &'t Branch<'t> {
        chain.half().branch().expect("the chain's branch is laid")
    }
}

#[cfg(test)]
impl<'t> LaysRuns<'t> for BumpHalf<'t> {
    fn lay<T>(self, len: usize, fill: impl FnMut(usize) -> T) -> &'t [T] {
        self.bump.lay(len, fill)
    }

    fn text(self, text: &str) -> &'t str {
        self.bump.text(text)
    }
}

#[cfg(test)]
impl<'t> Habitat<'t> for BumpHalf<'t> {
    fn own(self) -> Option<Own<'t>> {
        self.slot.get().map(Own::Branch)
    }

    fn own_or_lay(self) -> Own<'t> {
        Own::Branch(match self.slot.get() {
            Some(branch) => branch,
            None => {
                let branch = &*self.bump.alloc(Branch::new(self.bump));
                self.slot.set(Some(branch));
                branch
            }
        })
    }
}
