//! The tiers a type lives in: the frozen [`Root`] the load builds, and a [`Branch`] per habitat
//! half the run builds in. A [`TypeChain`] reads and writes them.
//!
//! - The **root** holds the load's types and outlives every run. While the load runs, its
//!   [`RootBuilder`] grows a hashbrown table in the bump it is laid in; the load's end freezes it
//!   ([`RootBuilder::freeze`]), moving the table out of its `RefCell` into a [`Root`] that has no
//!   intern door, so the run can never intern into it. A frozen root is read-only apart from its
//!   lossy verdict slots, which are cells, so it is covariant and a chain holds it at its own
//!   lifetime.
//! - A **branch** is a [`DigestTable`] of the nodes one habitat half — a cell region or a scratch
//!   habitat — interned, laid in that half at its smallest size by the first door that needs it,
//!   and released with the half. Its verdicts are a second table, laid at its first verdict. A
//!   branch written through shared borrows is invariant in its lifetime.
//!
//! A branch may **link** one other tier, read at a brand of its own through the object-safe
//! [`Tier`]: a spawned child's branch links its spawner's ([`BranchTier`]), which the pin that
//! carried the child's birth keeps alive. A chain over a scratch habitat reads its region's
//! branch the same way ([`RegionTier`]), re-reading the region's header on every probe so a branch
//! the region lays during the step is seen. A link never cycles: it points at a spawner, linked
//! before its child existed.
//!
//! How a chain reaches the tier it writes is the [`Habitat`] it is built over: a [`LaysRuns`]
//! with the half's own tier ([`Own`]) beside it. Koan implements it for a region's writer beside
//! its header door; the root builder implements it for the load; `BumpHalf` is the test suite's.
//!
//! See [identity.md § Storage](identity.md#storage-a-root-and-its-branches).

use std::cell::{Cell, RefCell};
use std::hash::BuildHasher;
use std::marker::PhantomData;

use crate::bump::{BumpAllocator, BumpHashTable, DigestTable, Keyed, LaysRuns, bump_hash_table};
use crate::symbols::IdentityBuildHasher;

use super::chain::TypeChain;
use super::digest::{TypeDigest, node_digest};
use super::node::TypeNode;
use super::stored::{Content, Flags, Linked, StoredNode};
use super::verdicts::{FixedVerdicts, VERDICT_SLOTS, VerdictEntry, VerdictKey, VerdictTable};

/// The root's node table: a reference to each stored node, bucket array in the root's bump.
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

/// The root's node under `digest` in `nodes`, or `None`.
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

/// The root while the load builds it: the node table in its `RefCell`, grown in the bump it is
/// laid in, and the verdict table laid there at the first verdict. The load's chain writes it;
/// [`freeze`](Self::freeze) ends the load.
pub struct RootBuilder<'g> {
    bump: BumpAllocator<'g>,
    nodes: RefCell<NodeTable<'g>>,
    verdicts: VerdictTable<'g>,
    never: Linked<'g>,
    any: Linked<'g>,
    frozen: Cell<bool>,
}

impl<'g> RootBuilder<'g> {
    /// A builder whose content lives in `bump`, laid there, with `Never` and `Any` laid so a chain
    /// can hold them, its verdict table `verdict_slots` long, seeded with the constants through a
    /// chain over it.
    fn seeded(bump: BumpAllocator<'g>, verdict_slots: usize) -> &'g Self {
        let mut nodes = bump_hash_table(bump);
        let mut lay_leaf = |leaf: TypeNode<'g, Linked<'g>>| -> Linked<'g> {
            let digest = node_digest(bump, &leaf);
            let node = bump.alloc(StoredNode::new(
                digest,
                Content::lay(leaf, Flags::NONE, bump),
            ));
            nodes.insert_unique(slot_hash(digest), &*node, |node| slot_hash(node.digest()));
            node
        };
        let (never, any) = (lay_leaf(TypeNode::Never), lay_leaf(TypeNode::Any));
        let builder = bump.alloc(RootBuilder {
            bump,
            nodes: RefCell::new(nodes),
            verdicts: VerdictTable::with_slots(verdict_slots),
            never,
            any,
            frozen: Cell::new(false),
        });
        TypeChain::building(&*builder).seed_constants(bump);
        builder
    }

    /// A builder over `bump`, seeded with the fixed handles — the leaves, the `OfKind` values,
    /// `List<Any>`, `Dict<Any, Any>`, the empty signature and the code composites — so the
    /// constants those names lower to resolve in a root that holds nothing else.
    pub fn in_region(bump: BumpAllocator<'g>) -> &'g Self {
        Self::seeded(bump, VERDICT_SLOTS)
    }

    /// `test`-only: a builder whose verdict table holds `slots` slots, so a test can fill a bucket.
    #[cfg(test)]
    pub(super) fn with_verdict_slots(bump: BumpAllocator<'g>, slots: usize) -> &'g Self {
        Self::seeded(bump, slots)
    }

    /// End the load: move the node table out into a frozen [`Root`] laid in the bump, beside the
    /// verdict table, laid now if nothing recorded a verdict. Nothing is rebuilt. An insert after
    /// the freeze panics.
    pub fn freeze(&self) -> &'g Root<'g> {
        assert!(!self.frozen.replace(true), "a root freezes once");
        let nodes = self.nodes.replace(bump_hash_table(self.bump));
        self.bump.alloc(Root {
            nodes,
            verdicts: self.verdicts.freeze(self.bump),
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
        find_in(&self.nodes.borrow(), digest)
    }

    pub(super) fn insert(&self, node: Linked<'g>) {
        assert!(!self.frozen.get(), "a frozen root takes no intern");
        self.nodes
            .borrow_mut()
            .insert_unique(slot_hash(node.digest()), node, |node| {
                slot_hash(node.digest())
            });
    }

    pub(super) fn len(&self) -> usize {
        self.nodes.borrow().len()
    }

    pub(super) fn verdict(&self, key: VerdictKey) -> Option<bool> {
        self.verdicts.get(key)
    }

    pub(super) fn record(&self, key: VerdictKey, verdict: bool) {
        self.verdicts.record(self.bump, key, verdict);
    }

    /// `test`-only: how many verdicts were recorded, and how many of those evicted another.
    #[cfg(test)]
    pub(super) fn verdict_tally(&self) -> (u64, u64) {
        self.verdicts.tally()
    }
}

impl<'g> LaysRuns<'g> for &'g RootBuilder<'g> {
    fn lay<T>(self, len: usize, fill: impl FnMut(usize) -> T) -> &'g [T] {
        self.bump.lay(len, fill)
    }

    fn text(self, text: &str) -> &'g str {
        self.bump.text(text)
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

/// The frozen root: the load's node table, read-only, and its fixed, lossy verdict table. It has
/// no intern door. Covariant in `'g`, so a chain holds it at its own lifetime.
pub struct Root<'g> {
    nodes: NodeTable<'g>,
    verdicts: FixedVerdicts<'g>,
    never: Linked<'g>,
    any: Linked<'g>,
}

impl<'g> Root<'g> {
    /// A root over `bump` holding the fixed handles and nothing else: a builder seeded and frozen
    /// at once. What a test that needs no load builds its chains over.
    pub fn seeded(bump: BumpAllocator<'g>) -> &'g Self {
        RootBuilder::in_region(bump).freeze()
    }

    /// How many nodes the root holds. Tests read it — koan's among them, which is why it is not
    /// `cfg(test)`: that does not reach a dependency.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Every digest the root holds, in no particular order. Tests read it — koan's among them.
    pub fn each_digest(&self, mut each: impl FnMut(TypeDigest)) {
        for node in self.nodes.iter() {
            each(node.digest());
        }
    }

    /// Whether the root holds no node; never, since `Never` and `Any` are laid before it exists.
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
        find_in(&self.nodes, digest)
    }

    pub(super) fn verdict(&self, key: VerdictKey) -> Option<bool> {
        self.verdicts.get(key)
    }

    pub(super) fn record(&self, key: VerdictKey, verdict: bool) {
        self.verdicts.record(key, verdict);
    }

    /// `test`-only: how many verdicts were recorded, and how many of those evicted another.
    #[cfg(test)]
    pub(super) fn verdict_tally(&self) -> (u64, u64) {
        self.verdicts.tally()
    }
}

/// The types one habitat half interned at the run: a [`DigestTable`] of its nodes laid in the
/// half, the verdicts recorded over them, laid at the first, and the tier it links, if its cell was
/// spawned by one whose habitat held a branch. Laid by the first door that interns into the half,
/// released with the half, and never dropped.
pub struct Branch<'t> {
    nodes: DigestTable<'t, StoredNode<'t>>,
    verdicts: Cell<Option<&'t DigestTable<'t, VerdictEntry>>>,
    link: Cell<Option<&'t dyn Tier<'t>>>,
}

const _: () = assert!(!std::mem::needs_drop::<Branch<'static>>());

impl<'t> Branch<'t> {
    /// An empty branch on the smallest table, laid through `lay`.
    pub fn new(lay: impl LaysRuns<'t>) -> Self {
        Branch {
            nodes: DigestTable::new(lay),
            verdicts: Cell::new(None),
            link: Cell::new(None),
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

    /// Visit every node the branch holds.
    pub(super) fn each(&self, visit: impl FnMut(Linked<'t>)) {
        self.nodes.each(visit);
    }

    /// Link `tier`: one per branch, since a cell has one spawner.
    pub(super) fn set_link(&self, tier: &'t dyn Tier<'t>) {
        assert!(
            self.link.replace(Some(tier)).is_none(),
            "a branch links one spawner"
        );
    }

    pub(super) fn verdict(&self, key: VerdictKey) -> Option<bool> {
        self.verdicts.get()?.get(key).map(VerdictEntry::verdict)
    }

    /// Record `verdict` unless the branch holds one for `key`, laying the verdict table through
    /// `lay` on the first.
    pub(super) fn record(&self, lay: impl LaysRuns<'t>, key: VerdictKey, verdict: bool) {
        let table = match self.verdicts.get() {
            Some(table) => table,
            None => {
                let table = lay_one(lay, DigestTable::new(lay));
                self.verdicts.set(Some(table));
                table
            }
        };
        if table.get(key).is_none() {
            table.insert_unique(lay, lay_one(lay, VerdictEntry::new(key, verdict)));
        }
    }
}

/// The tier a chain writes: the root builder during the load, a habitat half's branch at the run.
/// A chain reaches it through its [`Habitat`] and never matches on it.
#[derive(Clone, Copy)]
pub enum Own<'t> {
    Builder(&'t RootBuilder<'t>),
    Branch(&'t Branch<'t>),
}

impl<'t> Own<'t> {
    pub(super) fn find(self, digest: TypeDigest) -> Option<Linked<'t>> {
        match self {
            Own::Builder(builder) => builder.find(digest),
            Own::Branch(branch) => branch.find(digest),
        }
    }

    pub(super) fn reach(self, digest: TypeDigest) -> Option<Linked<'t>> {
        match self {
            Own::Builder(builder) => builder.find(digest),
            Own::Branch(branch) => branch.reach(digest),
        }
    }

    pub(super) fn link_reach(self, digest: TypeDigest) -> Option<Linked<'t>> {
        match self {
            Own::Builder(_) => None,
            Own::Branch(branch) => branch.link_reach(digest),
        }
    }

    pub(super) fn insert(self, lay: impl LaysRuns<'t>, node: Linked<'t>) {
        match self {
            Own::Builder(builder) => builder.insert(node),
            Own::Branch(branch) => branch.insert(lay, node),
        }
    }

    pub(super) fn len(self) -> usize {
        match self {
            Own::Builder(builder) => builder.len(),
            Own::Branch(branch) => branch.len(),
        }
    }

    pub(super) fn verdict(self, key: VerdictKey) -> Option<bool> {
        match self {
            Own::Builder(builder) => builder.verdict(key),
            Own::Branch(branch) => branch.verdict(key),
        }
    }

    pub(super) fn record(self, lay: impl LaysRuns<'t>, key: VerdictKey, verdict: bool) {
        match self {
            Own::Builder(builder) => builder.record(key, verdict),
            Own::Branch(branch) => branch.record(lay, key, verdict),
        }
    }

    /// The branch, for a door only a branch serves.
    pub(super) fn branch(self) -> &'t Branch<'t> {
        match self {
            Own::Builder(_) => panic!("the root builder is not a branch"),
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
}

/// A scratch chain's region tier: the region half itself, so every call re-reads the region's
/// tier and a branch the region lays during the step is seen. Laid in the scratch half.
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
