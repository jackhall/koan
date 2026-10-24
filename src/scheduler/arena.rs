//! The build arena: the settlements a graph's frames run under, kept as books beside the cell
//! graph rather than as cells in it.
//!
//! An arena node is a settlement: one body's facts settled under one binding, found by its parent
//! and its [`SettlementKey`]. Its parent is the arena node its callee was born under, or the root,
//! the frozen load, which is no slot ([`ArenaRef::ROOT`]). A run cell born under an arena node
//! borrows it for its life, through the drain, so a borrowed arena node is never evicted. Each
//! slot carries a generation, bumped when its arena node is evicted, so an [`ArenaRef`] held past
//! an eviction names nothing rather than the slot's next tenant.
//!
//! **Held.** An arena node is held while a run cell borrows it or while a held arena node lies
//! below it. The held total moves only where a count turns from zero or to zero, and each turn
//! propagates to the parent, so a borrow costs the depth of its chain.
//!
//! **Eviction** runs after a miss: while the unheld bytes exceed the [`BuildBudget`], the unheld
//! arena node with no children and the oldest use goes. A parent is no candidate while it has a
//! child, so eviction takes leaves first, and a linear scan is enough since the budget bounds the
//! count. See [build/README.md § The store](../build/README.md#the-store).

use std::collections::HashMap;

use crate::memory::Bump;
use crate::values::{ArenaRef, BuildBudget, SettlementKey};

/// A graph's arena nodes, their books, and the budget their unheld bytes stay within.
pub(super) struct BuildArena {
    slots: Vec<Slot>,
    /// Slots holding no arena node, reused before the slab grows.
    free: Vec<u32>,
    /// Every live settlement, by its parent and key.
    children: HashMap<(ArenaRef, SettlementKey), ArenaRef>,
    budget: BuildBudget,
    /// Bytes of every live arena node.
    total: usize,
    /// Bytes of every held arena node.
    held: usize,
    /// The use counter each stamp takes the next value of.
    clock: u64,
}

/// One slot of the arena: its generation, and the arena node it holds, if any.
struct Slot {
    generation: u32,
    node: Option<ArenaNode>,
}

/// A settlement's books: where it sits, who holds it, when it was last used, and what it weighs.
struct ArenaNode {
    parent: ArenaRef,
    key: SettlementKey,
    /// Run cells borrowing it.
    borrowers: usize,
    /// Children that are held.
    held_children: usize,
    /// Live children.
    children: usize,
    /// The clock at its last use: a miss, a hit, or a hop under it.
    used: u64,
    bytes: usize,
    /// The settlement's own storage. It holds the key alone until a settlement holds its facts.
    #[expect(
        dead_code,
        reason = "owned for its bytes until a settlement's facts are laid in it"
    )]
    bump: Bump,
}

impl ArenaNode {
    fn is_held(&self) -> bool {
        self.borrowers > 0 || self.held_children > 0
    }
}

impl BuildArena {
    /// An empty arena whose unheld bytes stay within `budget`.
    pub(super) fn new(budget: BuildBudget) -> Self {
        BuildArena {
            slots: Vec::new(),
            free: Vec::new(),
            children: HashMap::new(),
            budget,
            total: 0,
            held: 0,
            clock: 0,
        }
    }

    fn node(&self, at: ArenaRef) -> Option<&ArenaNode> {
        let (slot, generation) = at.opened()?;
        let slot = self.slots.get(slot as usize)?;
        (slot.generation == generation).then_some(slot.node.as_ref())?
    }

    fn node_mut(&mut self, at: ArenaRef) -> Option<&mut ArenaNode> {
        let (slot, generation) = at.opened()?;
        let slot = self.slots.get_mut(slot as usize)?;
        (slot.generation == generation).then_some(slot.node.as_mut())?
    }

    /// Whether `at` names a live arena node. The root always does.
    pub(super) fn is_live(&self, at: ArenaRef) -> bool {
        at == ArenaRef::ROOT || self.node(at).is_some()
    }

    /// The live settlement keyed `key` under `parent`, if any. Stamps nothing.
    pub(super) fn find(&self, parent: ArenaRef, key: SettlementKey) -> Option<ArenaRef> {
        self.children.get(&(parent, key)).copied()
    }

    /// A new settlement keyed `key` under the live `parent`, unheld and stamped. Evicts nothing:
    /// the drain calls [`evict_to_budget`](Self::evict_to_budget) once a frame borrows it.
    pub(super) fn insert(&mut self, parent: ArenaRef, key: SettlementKey) -> ArenaRef {
        debug_assert!(
            self.is_live(parent),
            "a settlement is built under a live parent"
        );
        debug_assert!(
            self.find(parent, key).is_none(),
            "a key settles once per parent"
        );
        let bump = Bump::new();
        bump.alloc(key);
        let bytes = size_of::<ArenaNode>() + bump.allocated_bytes();
        self.clock += 1;
        let node = ArenaNode {
            parent,
            key,
            borrowers: 0,
            held_children: 0,
            children: 0,
            used: self.clock,
            bytes,
            bump,
        };
        let at = match self.free.pop() {
            Some(slot) => {
                let entry = &mut self.slots[slot as usize];
                entry.node = Some(node);
                ArenaRef::of(slot, entry.generation)
            }
            None => {
                let slot = u32::try_from(self.slots.len()).expect("the arena fits u32 slots");
                self.slots.push(Slot {
                    generation: 0,
                    node: Some(node),
                });
                ArenaRef::of(slot, 0)
            }
        };
        self.total += bytes;
        if let Some(parent) = self.node_mut(parent) {
            parent.children += 1;
        }
        self.children.insert((parent, key), at);
        at
    }

    /// Mark `at` used now: a hit, or a hop under it. The root takes no stamp.
    pub(super) fn stamp(&mut self, at: ArenaRef) {
        self.clock += 1;
        let clock = self.clock;
        if let Some(node) = self.node_mut(at) {
            node.used = clock;
        }
    }

    /// Count one more run cell borrowing the live `at`. The root is never counted.
    pub(super) fn borrow(&mut self, at: ArenaRef) {
        let Some(node) = self.node_mut(at) else {
            debug_assert_eq!(at, ArenaRef::ROOT, "a run cell borrows a live arena node");
            return;
        };
        let was = node.is_held();
        node.borrowers += 1;
        if !was {
            self.turned(at, true);
        }
    }

    /// Count one fewer run cell borrowing `at`, which it borrowed.
    pub(super) fn release(&mut self, at: ArenaRef) {
        let Some(node) = self.node_mut(at) else {
            debug_assert_eq!(at, ArenaRef::ROOT, "a borrowed arena node stays live");
            return;
        };
        node.borrowers -= 1;
        if !node.is_held() {
            self.turned(at, false);
        }
    }

    /// Move `at`'s bytes into or out of the held total, as it turned held or unheld, and carry
    /// the turn up through every parent it turns in turn.
    fn turned(&mut self, mut at: ArenaRef, held: bool) {
        loop {
            let node = self.node(at).expect("a turning arena node is live");
            let (bytes, parent) = (node.bytes, node.parent);
            if held {
                self.held += bytes;
            } else {
                self.held -= bytes;
            }
            let Some(parent_node) = self.node_mut(parent) else {
                return;
            };
            let was = parent_node.is_held();
            if held {
                parent_node.held_children += 1;
            } else {
                parent_node.held_children -= 1;
            }
            if was == parent_node.is_held() {
                return;
            }
            at = parent;
        }
    }

    /// Evict unheld leaves, least recently used first, until the unheld bytes are within the
    /// budget, and return how many went.
    pub(super) fn evict_to_budget(&mut self) -> usize {
        let mut evicted = 0;
        while self.total - self.held > self.budget.bytes.get() {
            let victim = (self.slots.iter().enumerate())
                .filter_map(|(slot, entry)| Some((slot, entry.generation, entry.node.as_ref()?)))
                .filter(|(_, _, node)| !node.is_held() && node.children == 0)
                .min_by_key(|(_, _, node)| node.used)
                .map(|(slot, generation, _)| ArenaRef::of(slot as u32, generation));
            let Some(victim) = victim else {
                break;
            };
            self.evict(victim);
            evicted += 1;
        }
        evicted
    }

    /// Remove the unheld, childless `at`: its slot's generation moves on, so every handle to it
    /// goes stale.
    fn evict(&mut self, at: ArenaRef) {
        let (slot, _) = at.opened().expect("the root is never evicted");
        let entry = &mut self.slots[slot as usize];
        let node = entry.node.take().expect("an evicted arena node is live");
        debug_assert!(
            !node.is_held() && node.children == 0,
            "eviction takes unheld leaves"
        );
        entry.generation += 1;
        self.free.push(slot);
        self.total -= node.bytes;
        self.children.remove(&(node.parent, node.key));
        if let Some(parent) = self.node_mut(node.parent) {
            parent.children -= 1;
        }
    }

    /// How many settlements are live.
    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.children.len()
    }

    /// Every live settlement.
    #[cfg(test)]
    pub(super) fn settlements(&self) -> impl Iterator<Item = ArenaRef> + '_ {
        self.children.values().copied()
    }

    /// How many live settlements lie directly under `parent`.
    #[cfg(test)]
    pub(super) fn children_of(&self, parent: ArenaRef) -> usize {
        (self.children.keys())
            .filter(|(under, _)| *under == parent)
            .count()
    }

    /// What the live `at` weighs.
    #[cfg(test)]
    pub(super) fn bytes_of(&self, at: ArenaRef) -> Option<usize> {
        Some(self.node(at)?.bytes)
    }

    /// The bytes of live arena nodes no run cell holds.
    #[cfg(test)]
    pub(super) fn unheld_bytes(&self) -> usize {
        self.total - self.held
    }

    /// How many run cells borrow `at`: `None` for an arena node that is not live, or the root.
    #[cfg(test)]
    pub(super) fn borrowers(&self, at: ArenaRef) -> Option<usize> {
        Some(self.node(at)?.borrowers)
    }
}
