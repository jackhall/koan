//! The tree pool: the third region habitat, for a call subtree whose liveness is a stack
//! discipline rather than a matrix reading. See [design/tree-cells.md](../design/tree-cells.md).
//!
//! A tree cell lives **under a root** — a slab cell — through a chain of tree parents, and owns its
//! region outright. It takes no slab slot, so the pool grows without a cap; it has no row, no
//! column, no holder count and no reach table, because a parent outlives its children and every
//! hold a value inside the subtree can take points up its own chain at a cell that is still there.
//! No mask ever names a tree cell: a placement into one mints into its root, and a carrier homed in
//! one travels with the root's bit as its reach.
//!
//! What a slot does carry is the little that death needs: the chain links and the depth the
//! ancestry rule classifies by, the count of undisposed children that keeps a released parent
//! undisposed itself, the **pledge** naming the ancestor its bump will splice into, and the
//! tombstone links that say where its bytes went once it did.

use crate::handle::{CellHandle, SlabHandle, Stale, TreeHandle};
use crate::reattach::{Erased, Reattachable};
use crate::region::Region;
use crate::scratch::Scratch;

/// What one pool index currently holds.
///
/// `Dead` is the undisposed state, exactly as it is on the slab: the embedder declared the cell's
/// death while a child was still live, so the cell is stale to every door but its region stays put.
/// `Absorbed` is a tombstone — the cell is gone and its bytes moved, and the slot survives only to
/// answer "where to" for a dormant carrier still keyed to it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum TreeState {
    Free,
    Live,
    Dead,
    Absorbed,
}

/// The ancestor a cell's bump will splice into at death — the promise a placement door makes when
/// the verdict pins a value homed here into a cell further up the chain.
///
/// Always an ancestor on the cell's own chain, so the destination is disposed after the pledging
/// cell and the splice always finds it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Pledge {
    /// The root, whose region is a slab slot's.
    Root,
    /// A tree ancestor, by pool index.
    Tree(u32),
}

/// Where a tree cell sits relative to a placement destination on the same root — the classification
/// the ancestry rule turns on.
///
/// Not one of the crate's **relations**. Those are the two square bit matrices over slab slots,
/// birth and pin ([`Matrix`](crate::matrix::Matrix)), and no tree cell is in either. Ancestry is
/// read off the chain links instead, at a cost of the level distance between the two cells.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Ancestry {
    /// The destination is the cell itself, or a tree cell under it. It dies first, so a borrow into
    /// this cell's storage embedded there stays valid without any promise.
    Under,
    /// The destination is an ancestor on this cell's chain. A pin costs the splice price and
    /// pledges this cell — and every intermediate — to splice into it at death.
    Above,
    /// Neither: a cousin under the same root, or a cell under a different root entirely. Nothing
    /// outside the chain may outlive this cell while borrowing it, so the crossing is a forced
    /// copy.
    Apart,
}

/// One tree cell.
///
/// There is no reach table, no continuation reach and no hold set: a value homed here reaches
/// its root and nothing else, and the root's row and sealed-hold set are where every mint from
/// inside the subtree lands.
struct TreeCell<C: Reattachable> {
    generation: u32,
    state: TreeState,
    /// The slab slot of the root at the top of this cell's chain. The root cannot recycle while a
    /// tree cell under it is undisposed — its own disposal waits on the child count — so the slot
    /// number alone names it for as long as this cell can ask.
    root: u32,
    /// The pool index of the tree parent, and `None` when the parent is the root itself.
    parent: Option<u32>,
    /// `1` for a child of the root, and the parent's depth plus one otherwise. What makes the
    /// ancestry rule's test cost the level distance rather than the depth of the tree.
    depth: u32,
    /// Tree children that have not disposed — live and dead-but-undisposed alike. A parent disposes
    /// only once this reaches zero, which is what lets an embedder tear a subtree down in any
    /// order.
    children: u32,
    executing: bool,
    /// The shallowest ancestor a value homed here has been pinned into, and so the destination this
    /// cell's whole bump splices into at death.
    pledge: Option<Pledge>,
    /// Whether any value homed here was ever put to rest. A cell nothing was kept in leaves no
    /// tombstone: no key can name it, so nothing will ever ask where its bytes went.
    kept: bool,
    continuation: Option<Erased<C>>,
    region: Option<Region>,
    /// `Absorbed` only: where this cell's bytes went. Never repointed when *that* cell's bytes move
    /// on in turn — the chain lengthens instead — which is what keeps a splice O(1) list work
    /// however many tombstones hang off the dying cell.
    into: Option<CellHandle>,
    /// The head of the list of tombstones whose bytes spliced into this cell.
    tombstones: Option<u32>,
    /// The next tombstone on the tombstone list this one sits in.
    next: Option<u32>,
}

impl<C: Reattachable> TreeCell<C> {
    fn free(generation: u32) -> Self {
        TreeCell {
            generation,
            state: TreeState::Free,
            root: 0,
            parent: None,
            depth: 0,
            children: 0,
            executing: false,
            pledge: None,
            kept: false,
            continuation: None,
            region: None,
            into: None,
            tombstones: None,
            next: None,
        }
    }
}

/// The growable pool of tree cells: live ones, dead-but-undisposed ones, and the tombstones of the
/// ones whose bytes have moved.
///
/// No cap. The slab's is what bounds the matrix, and a tree cell is in no matrix; what bounds the
/// pool is the depth of the call tree the embedder is running, which is the program's business.
pub(crate) struct TreePool<C: Reattachable> {
    slots: Vec<TreeCell<C>>,
    free: Vec<u32>,
}

impl<C: Reattachable> TreePool<C> {
    pub(crate) fn new() -> Self {
        TreePool {
            slots: Vec::new(),
            free: Vec::new(),
        }
    }

    /// Whether the pool holds nothing at all — no live cell, no dead-but-undisposed one, and no
    /// tombstone. Part of the graph's end-of-program alarm.
    pub(crate) fn is_empty(&self) -> bool {
        self.free.len() == self.slots.len()
    }

    /// Take a slot for a new cell under `root`, with `parent` its tree parent when it has one.
    pub(crate) fn create(
        &mut self,
        root: u32,
        parent: Option<u32>,
        depth: u32,
        continuation: Option<Erased<C>>,
    ) -> TreeHandle {
        let index = match self.free.pop() {
            Some(index) => index,
            None => {
                self.slots.push(TreeCell::free(0));
                (self.slots.len() - 1) as u32
            }
        };
        let slot = &mut self.slots[index as usize];
        slot.state = TreeState::Live;
        slot.root = root;
        slot.parent = parent;
        slot.depth = depth;
        slot.continuation = continuation;
        TreeHandle::new(index, slot.generation)
    }

    /// The pool index a handle names, if that index still holds the live cell it was minted for.
    pub(crate) fn live_index(&self, handle: TreeHandle) -> Result<u32, Stale<TreeHandle>> {
        match self.slots.get(handle.index() as usize) {
            Some(slot)
                if slot.state == TreeState::Live && slot.generation == handle.generation() =>
            {
                Ok(handle.index())
            }
            _ => Err(Stale(handle)),
        }
    }

    /// The handle naming whatever occupies `index` right now.
    pub(crate) fn occupant(&self, index: u32) -> TreeHandle {
        TreeHandle::new(index, self.slots[index as usize].generation)
    }

    pub(crate) fn state(&self, index: u32) -> TreeState {
        self.slots[index as usize].state
    }

    pub(crate) fn root(&self, index: u32) -> u32 {
        self.slots[index as usize].root
    }

    pub(crate) fn parent(&self, index: u32) -> Option<u32> {
        self.slots[index as usize].parent
    }

    pub(crate) fn depth(&self, index: u32) -> u32 {
        self.slots[index as usize].depth
    }

    pub(crate) fn children(&self, index: u32) -> u32 {
        self.slots[index as usize].children
    }

    pub(crate) fn pledge(&self, index: u32) -> Option<Pledge> {
        self.slots[index as usize].pledge
    }

    pub(crate) fn tombstones(&self, index: u32) -> Option<u32> {
        self.slots[index as usize].tombstones
    }

    #[cfg(test)]
    pub(crate) fn tombstone_target(&self, index: u32) -> Option<CellHandle> {
        self.slots[index as usize].into
    }

    pub(crate) fn is_executing(&self, index: u32) -> bool {
        self.slots[index as usize].executing
    }

    pub(crate) fn set_executing(&mut self, index: u32, executing: bool) {
        self.slots[index as usize].executing = executing;
    }

    pub(crate) fn mark_dead(&mut self, index: u32) {
        self.slots[index as usize].state = TreeState::Dead;
    }

    pub(crate) fn mark_kept(&mut self, index: u32) {
        self.slots[index as usize].kept = true;
    }

    pub(crate) fn take_continuation(&mut self, index: u32) -> Option<Erased<C>> {
        self.slots[index as usize].continuation.take()
    }

    pub(crate) fn set_continuation(&mut self, index: u32, continuation: Option<Erased<C>>) {
        self.slots[index as usize].continuation = continuation;
    }

    pub(crate) fn add_child(&mut self, index: u32) {
        self.slots[index as usize].children += 1;
    }

    /// Report one child's disposal. The parent disposes when the count reaches zero and its own
    /// death has already been declared.
    pub(crate) fn drop_child(&mut self, index: u32) {
        let slot = &mut self.slots[index as usize];
        debug_assert!(slot.children > 0, "a tree cell disposed under no parent");
        slot.children -= 1;
    }

    /// The cell's region, minted on first use — the write surface a placement into it wants.
    pub(crate) fn region_mut(&mut self, index: u32) -> &mut Region {
        self.slots[index as usize]
            .region
            .get_or_insert_with(Region::new)
    }

    /// Take the cell's storage off it, for the splice or the drop that disposal performs.
    pub(crate) fn take_region(&mut self, index: u32) -> Option<Region> {
        self.slots[index as usize].region.take()
    }

    /// Splice a departing cell's bump into this one's bundle.
    pub(crate) fn splice_into(&mut self, index: u32, from: Option<Region>) {
        Region::splice_optional(&mut self.slots[index as usize].region, from);
    }

    /// Chunk bytes this cell's region bundle occupies, `0` where it never allocated.
    pub(crate) fn region_bytes(&self, index: u32) -> usize {
        self.slots[index as usize]
            .region
            .as_ref()
            .map_or(0, Region::allocated_bytes)
    }

    /// Where `dest` sits relative to the cell in `home`, both under the same root.
    ///
    /// The walk is depth-bounded: the deeper side steps up by exactly the level distance and the
    /// identities are compared once. Cost is the distance between the two, never the depth of the
    /// tree.
    pub(crate) fn ancestry(&self, home: u32, dest: u32) -> Ancestry {
        if home == dest {
            return Ancestry::Under;
        }
        let (home_depth, dest_depth) = (self.depth(home), self.depth(dest));
        if dest_depth > home_depth {
            match self.walk_up(dest, dest_depth - home_depth) == Some(home) {
                true => Ancestry::Under,
                false => Ancestry::Apart,
            }
        } else {
            match self.walk_up(home, home_depth - dest_depth) == Some(dest) {
                true => Ancestry::Above,
                false => Ancestry::Apart,
            }
        }
    }

    /// The ancestor `steps` levels above `index`, or `None` when the walk leaves the tree — which
    /// it cannot, for a step count derived from the two depths.
    fn walk_up(&self, index: u32, steps: u32) -> Option<u32> {
        let mut at = Some(index);
        for _ in 0..steps {
            at = self.parent(at?);
        }
        at
    }

    /// How deep a pledge's destination sits: the root is depth zero, a tree ancestor its own depth.
    /// A smaller number is shallower, and a shallower pledge subsumes a deeper one.
    pub(crate) fn pledge_depth(&self, pledge: Pledge) -> u32 {
        match pledge {
            Pledge::Root => 0,
            Pledge::Tree(index) => self.depth(index),
        }
    }

    /// The cells on the walk from `home` up to — and excluding — `dest`'s own level, whose pledge
    /// is not already `dest` or shallower.
    ///
    /// These are exactly the cells a pin into `dest` has to pledge, and their bundles are exactly
    /// what it costs: a grandchild pinned straight into its grandparent while its parent reclaimed
    /// would leave the grandparent's bundle borrowing bytes that are gone, so every intermediate is
    /// carried too.
    pub(crate) fn unpledged_up<'p>(
        &'p self,
        home: u32,
        dest: Pledge,
    ) -> impl Iterator<Item = u32> + 'p {
        let floor = self.pledge_depth(dest);
        let mut at = Some(home);
        std::iter::from_fn(move || {
            let index = at?;
            if self.depth(index) <= floor {
                return None;
            }
            at = self.parent(index);
            Some(index)
        })
        .filter(move |index| {
            self.pledge(*index)
                .is_none_or(|held| self.pledge_depth(held) > floor)
        })
    }

    /// Pledge `home` and every intermediate below `dest` to splice into it, leaving alone the ones
    /// already pledged at least that shallow.
    pub(crate) fn pledge_up(&mut self, home: u32, dest: Pledge) {
        let floor = self.pledge_depth(dest);
        let mut at = Some(home);
        while let Some(index) = at {
            if self.depth(index) <= floor {
                return;
            }
            at = self.parent(index);
            let held = self.slots[index as usize].pledge;
            if held.is_none_or(|held| self.pledge_depth(held) > floor) {
                self.slots[index as usize].pledge = Some(dest);
            }
        }
    }

    /// Turn a disposed cell into a tombstone pointing at `into`, pushed onto the tombstone list
    /// whose current head is `head`. The caller stores the new head, since the list may hang off a
    /// slab slot rather than a pool slot.
    pub(crate) fn entomb(&mut self, index: u32, into: CellHandle, head: Option<u32>) {
        let slot = &mut self.slots[index as usize];
        slot.state = TreeState::Absorbed;
        slot.into = Some(into);
        slot.next = head;
        slot.region = None;
        slot.continuation = None;
        // A tombstone is out of the tree: it parents nothing, is nobody's child, and pledges
        // nothing. All it answers is where its bytes went.
        slot.parent = None;
        slot.depth = 0;
        slot.pledge = None;
    }

    /// Whether a disposed cell has to stay in the pool as a tombstone: something was kept in it, or
    /// something spliced into it and is a tombstone in its own right.
    pub(crate) fn leaves_tombstone(&self, index: u32) -> bool {
        let slot = &self.slots[index as usize];
        slot.kept || slot.tombstones.is_some()
    }

    /// Push a tombstone onto this cell's own tombstone list.
    pub(crate) fn adopt_tombstone(&mut self, index: u32, tombstone: u32) {
        let head = self.slots[index as usize].tombstones;
        self.slots[tombstone as usize].next = head;
        self.slots[index as usize].tombstones = Some(tombstone);
    }

    /// Return a slot to the free list under a fresh generation, so every handle minted for the
    /// departing occupant is stale from here on.
    pub(crate) fn recycle(&mut self, index: u32) {
        let slot = &mut self.slots[index as usize];
        *slot = TreeCell::free(slot.generation.wrapping_add(1));
        self.free.push(index);
    }

    /// Free a whole tombstone list and everything hanging off it: the bytes those tombstones point
    /// at are gone, so a redeem under one of their keys must answer `Gone` rather than find a
    /// target.
    pub(crate) fn free_tombstones(&mut self, head: Option<u32>, scratch: &Scratch) {
        // Checked before the worklist is built: every slab reclaim and every retired relocation
        // entry calls this, and almost none of them has a tombstone under it.
        let Some(head) = head else {
            return;
        };
        let mut pending = scratch.vec_with_capacity(1);
        pending.push(head);
        while let Some(index) = pending.pop() {
            let slot = &self.slots[index as usize];
            debug_assert_eq!(
                slot.state,
                TreeState::Absorbed,
                "only tombstones hang off a tombstone list"
            );
            pending.extend(slot.next);
            pending.extend(slot.tombstones);
            self.recycle(index);
        }
    }

    /// Follow the tombstone chain from a key's home to whatever answers for its bytes now: a live
    /// or dead-but-undisposed tree cell, or a slab handle the relocation map takes over from.
    /// `None` once the chain reaches a slot that has recycled, which means the bytes were
    /// reclaimed.
    ///
    /// One array load per hop, and one hop per splice the bytes have been through since the keep.
    pub(crate) fn resolve(&self, handle: TreeHandle) -> Option<TreeForward> {
        let mut index = handle.index();
        let mut generation = handle.generation();
        loop {
            let slot = self.slots.get(index as usize)?;
            if slot.generation != generation {
                return None;
            }
            match slot.state {
                TreeState::Live | TreeState::Dead => return Some(TreeForward::Tree(index)),
                TreeState::Absorbed => match slot.into? {
                    CellHandle::Slab(handle) => return Some(TreeForward::Slab(handle)),
                    CellHandle::Tree(next) => {
                        index = next.index();
                        generation = next.generation();
                    }
                },
                TreeState::Free => return None,
            }
        }
    }

    /// Every index the pool currently occupies, in any state but free — what the property test
    /// walks and what the emptiness assert counts.
    #[cfg(test)]
    pub(crate) fn occupied(&self) -> impl Iterator<Item = u32> + '_ {
        (0..self.slots.len() as u32).filter(|index| self.state(*index) != TreeState::Free)
    }

    #[cfg(test)]
    pub(crate) fn next_tombstone(&self, index: u32) -> Option<u32> {
        self.slots[index as usize].next
    }
}

/// Where a tombstone chain ends: a tree cell that still holds the bytes, or the slab handle the
/// relocation map answers for from there on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum TreeForward {
    Tree(u32),
    Slab(SlabHandle),
}
