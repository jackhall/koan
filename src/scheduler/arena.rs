//! The build arena: the settlements a graph's frames run under, kept as books beside the cell
//! graph rather than as cells in it.
//!
//! An arena node is a settlement or a composition. A settlement is one body's facts settled under
//! one binding, found by its parent and its [`SettlementKey`]. Its parent is the arena node its
//! callee was born under, or the root, the frozen load, which is no slot ([`ArenaRef::ROOT`]). Its
//! facts are a [`Settled`]: a bump holding them beside a shared hold on its parent's, which they
//! read below them, so a step reads the chain through the newest alone, borrowed from the arena for
//! the step. A composition is the facts of code composed at run time, a child of the root found
//! by its syntax digest: a [`Composed`], a copy of its syntax and the shapes and records built over
//! it in program storage of its own, which a step under it reads as its chain's base in place of
//! the load. The books hold the digest alone: the syntax a miss copies comes from the code value
//! or callee that asked, through the cell that bounces it ([`ComposedSyntax`]). A run cell born under an arena node
//! borrows it for its life, through the drain, so a borrowed arena node is never evicted. Each
//! slot carries a generation, bumped when its arena node is evicted, so an [`ArenaRef`] held past
//! an eviction names nothing rather than the slot's next tenant.
//!
//! A settlement's parent is a settlement or a composition; one under a composition reads the
//! composition as its chain's base. A settlement under a composition that binds a module by a
//! tree another composition computed **pins** that composition: it keeps a shared hold on its
//! facts, and the composition counts it as a pinner.
//!
//! **Held.** An arena node is held while a run cell borrows it, while a held arena node lies below
//! it, or while a held settlement pins it. The held total moves only where a count turns from zero
//! or to zero, and each turn propagates to the parent and the pinned compositions, so a borrow
//! costs the depth of its chain.
//!
//! **Eviction** runs after a miss: while the unheld bytes exceed the [`BuildBudget`], the unheld
//! arena node with no children, no pinner and the oldest use goes. A parent is no candidate while
//! it has a child, nor a composition while a settlement pins it, so eviction takes leaves first.
//! Each victim is found by a linear scan of the live arena nodes, held ones included. See
//! [build/README.md § The store](../build/README.md#the-store).

use std::collections::HashMap;
use std::marker::PhantomData;
use std::rc::Rc;

use self_cell::self_cell;
use smallvec::SmallVec;

use crate::memory::Bump;
use crate::scope::{ComposedSyntax, Composition, Facts, LoadFacts, Settlement};
use crate::values::{ArenaRef, BuildBudget, ContentDigest, SettlementKey};

/// A graph's arena nodes, their books, and the budget their unheld bytes stay within.
pub(super) struct BuildArena<'graph> {
    slots: Vec<Slot<'graph>>,
    /// Slots holding no arena node, reused before the slab grows.
    free: Vec<u32>,
    /// Every live settlement, by its parent and key.
    children: HashMap<(ArenaRef, SettlementKey), ArenaRef>,
    /// Every live composition, by its syntax digest.
    compositions: HashMap<ContentDigest, ArenaRef>,
    budget: BuildBudget,
    /// Bytes of every live arena node.
    total: usize,
    /// Bytes of every held arena node.
    held: usize,
    /// The use counter each stamp takes the next value of.
    clock: u64,
}

/// One slot of the arena: its generation, and the arena node it holds, if any.
struct Slot<'graph> {
    generation: u32,
    node: Option<ArenaNode<'graph>>,
}

/// An arena node's books: where it sits, who holds it, when it was last used, and what it weighs,
/// beside its facts.
struct ArenaNode<'graph> {
    parent: ArenaRef,
    kind: Kind<'graph>,
    /// Run cells borrowing it.
    borrowers: usize,
    /// Children that are held.
    held_children: usize,
    /// Live children.
    children: usize,
    /// Live settlements pinning it, and those of them held: a composition's alone.
    pinners: usize,
    held_pinners: usize,
    /// The clock at its last use: a miss, a hit, or a hop under it.
    used: u64,
    bytes: usize,
}

/// Which arena node it is, by what finds it, beside its facts.
enum Kind<'graph> {
    Settlement {
        key: SettlementKey,
        facts: Rc<Settled<'graph>>,
        /// The compositions it pins.
        pins: SmallVec<[ArenaRef; 1]>,
    },
    Composition {
        digest: ContentDigest,
        facts: Rc<Composed<'graph>>,
    },
}

/// What a step under an arena node reads facts through: the composition its chain rests on, by
/// its syntax digest, if not the load, and the settlement it runs under, if any.
pub(super) struct Door<'a> {
    pub composition: Option<(ContentDigest, &'a Composition<'a>)>,
    pub top: Option<&'a Settlement<'a>>,
}

/// What a composition's facts are laid in: its syntax's own program storage, which its shapes and
/// records rest in beside the copy, as the load's rest in the program's, and a bump its walk
/// interns through and its terms are laid in. Its facts may read what lives at `'graph`, which
/// outlives it.
pub(super) struct ComposedOwner<'graph> {
    syntax: ComposedSyntax,
    bump: Bump,
    graph: PhantomData<&'graph ()>,
}

self_cell!(
    /// A composition's facts, laid in the storage its owner keeps. A step reads them only borrowed
    /// from the arena, as it reads a settlement's.
    pub(super) struct Composed<'graph> {
        owner: ComposedOwner<'graph>,
        #[covariant]
        dependent: Composition,
    }
);

impl<'graph> Composed<'graph> {
    /// The facts `compose` lays over `syntax`, in its storage and beside a fresh bump, handed the
    /// witness that `'graph` outlives both.
    pub(super) fn build(
        syntax: ComposedSyntax,
        compose: impl for<'a> FnOnce(&'a ComposedSyntax, &'a Bump, &'a &'graph ()) -> Composition<'a>,
    ) -> Self {
        let owner = ComposedOwner {
            syntax,
            bump: Bump::new(),
            graph: PhantomData,
        };
        Composed::new(owner, |owner| compose(&owner.syntax, &owner.bump, &&()))
    }

    /// The facts themselves.
    pub(super) fn facts(&self) -> &Composition<'_> {
        self.borrow_dependent()
    }

    /// What the facts weigh: their storage's bytes and their bump's.
    fn bytes(&self) -> usize {
        let owner = self.borrow_owner();
        owner.syntax.allocated_bytes() + owner.bump.allocated_bytes()
    }
}

/// What a settlement's facts are laid in: their own bump, beside the parent settlement they read
/// below them, the composition their chain rests on, if not the load, by syntax digest, and the
/// compositions whose trees they bind a module by, each of which they keep alive; and the load
/// beneath all.
pub(super) struct SettledOwner<'graph> {
    bump: Bump,
    parent: Option<Rc<Settled<'graph>>>,
    base: Option<(ContentDigest, Rc<Composed<'graph>>)>,
    pins: SmallVec<[Rc<Composed<'graph>>; 1]>,
    load: &'graph LoadFacts<'graph>,
}

/// What a new settlement is laid over: the settlement or the composition it is a child of, or the
/// root.
pub(super) enum Under<'graph> {
    Root,
    Settlement(Rc<Settled<'graph>>),
    Composition(ContentDigest, Rc<Composed<'graph>>),
}

self_cell!(
    /// A settlement's facts, laid in the bump its owner keeps. A step reads them only borrowed
    /// from the arena, at a borrow inside its own, so nothing of them can be laid in a region.
    pub(super) struct Settled<'graph> {
        owner: SettledOwner<'graph>,
        #[covariant]
        dependent: Settlement,
    }
);

impl<'graph> Settled<'graph> {
    /// The facts `settle` lays in a new bump over `under`'s, its chain's base beneath — the load,
    /// or the composition it rests on — beside the facts of each composition in `pins`: `None`
    /// where it settles nothing. `settle` reads the layers it rests on, and is handed the witness
    /// that `'graph` outlives the bump.
    pub(super) fn build(
        under: Under<'graph>,
        pins: SmallVec<[Rc<Composed<'graph>>; 1]>,
        load: &'graph LoadFacts<'graph>,
        settle: impl for<'a> FnOnce(
            &'a Bump,
            Facts<'a>,
            &[&'a Composition<'a>],
            &'a &'graph (),
        ) -> Option<Settlement<'a>>,
    ) -> Option<Self> {
        let (parent, base) = match under {
            Under::Root => (None, None),
            Under::Settlement(parent) => {
                let base = parent.borrow_owner().base.clone();
                (Some(parent), base)
            }
            Under::Composition(digest, composition) => (None, Some((digest, composition))),
        };
        let owner = SettledOwner {
            bump: Bump::new(),
            parent,
            base,
            pins,
            load,
        };
        Settled::try_new(owner, |owner| {
            let top = owner.parent.as_deref().map(Settled::borrow_dependent);
            let facts = match &owner.base {
                Some((_, composition)) => Facts::composed(composition.facts(), top),
                None => Facts::under(owner.load, top),
            };
            let pinned: SmallVec<[&Composition<'_>; 1]> =
                owner.pins.iter().map(|pin| pin.facts()).collect();
            settle(&owner.bump, facts, &pinned, &&()).ok_or(())
        })
        .ok()
    }

    /// The composition its chain rests on, by syntax digest, if not the load.
    fn base(&self) -> Option<(ContentDigest, &Composition<'_>)> {
        (self.borrow_owner().base.as_ref())
            .map(|(digest, composition)| (*digest, composition.facts()))
    }

    /// The facts themselves.
    pub(super) fn facts(&self) -> &Settlement<'_> {
        self.borrow_dependent()
    }

    /// What the facts weigh: their bump's bytes.
    fn bytes(&self) -> usize {
        self.borrow_owner().bump.allocated_bytes()
    }
}

impl ArenaNode<'_> {
    fn is_held(&self) -> bool {
        self.borrowers > 0 || self.held_children > 0 || self.held_pinners > 0
    }

    /// The compositions it pins: none for a composition.
    fn pins(&self) -> &[ArenaRef] {
        match &self.kind {
            Kind::Settlement { pins, .. } => pins,
            Kind::Composition { .. } => &[],
        }
    }
}

impl<'graph> BuildArena<'graph> {
    /// An empty arena whose unheld bytes stay within `budget`.
    pub(super) fn new(budget: BuildBudget) -> Self {
        BuildArena {
            slots: Vec::new(),
            free: Vec::new(),
            children: HashMap::new(),
            compositions: HashMap::new(),
            budget,
            total: 0,
            held: 0,
            clock: 0,
        }
    }

    fn node(&self, at: ArenaRef) -> Option<&ArenaNode<'graph>> {
        let (slot, generation) = at.opened()?;
        let slot = self.slots.get(slot as usize)?;
        (slot.generation == generation).then_some(slot.node.as_ref())?
    }

    fn node_mut(&mut self, at: ArenaRef) -> Option<&mut ArenaNode<'graph>> {
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

    /// What a settlement built under the live `at` is laid over: the root for the root or a
    /// handle gone stale.
    pub(super) fn under(&self, at: ArenaRef) -> Under<'graph> {
        match self.node(at).map(|node| &node.kind) {
            None => Under::Root,
            Some(Kind::Settlement { facts, .. }) => Under::Settlement(Rc::clone(facts)),
            Some(Kind::Composition { digest, facts }) => {
                Under::Composition(*digest, Rc::clone(facts))
            }
        }
    }

    /// A shared hold on the facts of each live composition in `pins`.
    pub(super) fn pinned(&self, pins: &[ArenaRef]) -> SmallVec<[Rc<Composed<'graph>>; 1]> {
        (pins.iter())
            .map(|at| match self.node(*at).map(|node| &node.kind) {
                Some(Kind::Composition { facts, .. }) => Rc::clone(facts),
                _ => unreachable!("a pin is a live composition"),
            })
            .collect()
    }

    /// What a step under the live `at` reads: nothing past the load for the root.
    pub(super) fn door(&self, at: ArenaRef) -> Door<'_> {
        match self.node(at).map(|node| &node.kind) {
            None => Door {
                composition: None,
                top: None,
            },
            Some(Kind::Settlement { facts, .. }) => Door {
                composition: facts.base(),
                top: Some(facts.facts()),
            },
            Some(Kind::Composition { digest, facts }) => Door {
                composition: Some((*digest, facts.facts())),
                top: None,
            },
        }
    }

    /// The live composition of the code whose syntax digest is `digest`, if any. Stamps nothing.
    pub(super) fn composition(&self, digest: ContentDigest) -> Option<ArenaRef> {
        self.compositions.get(&digest).copied()
    }

    /// A new settlement keyed `key` under the live `parent`, holding `facts` and pinning each live
    /// composition in `pins`, unheld and stamped. Evicts nothing: the drain calls
    /// [`evict_to_budget`](Self::evict_to_budget) once a frame borrows it.
    pub(super) fn insert(
        &mut self,
        parent: ArenaRef,
        key: SettlementKey,
        facts: Settled<'graph>,
        pins: SmallVec<[ArenaRef; 1]>,
    ) -> ArenaRef {
        debug_assert!(
            self.is_live(parent),
            "a settlement is built under a live parent"
        );
        debug_assert!(
            self.find(parent, key).is_none(),
            "a key settles once per parent"
        );
        for pin in &pins {
            let node = self
                .node_mut(*pin)
                .expect("a settlement pins a live composition");
            node.pinners += 1;
        }
        let bytes = facts.bytes();
        let facts = Rc::new(facts);
        let at = self.laid(parent, bytes, Kind::Settlement { key, facts, pins });
        self.children.insert((parent, key), at);
        at
    }

    /// A new composition of the code whose syntax digest is `digest`, a child of the root holding
    /// `facts`, unheld and stamped. Evicts nothing, as [`insert`](Self::insert) does not.
    pub(super) fn compose(&mut self, digest: ContentDigest, facts: Composed<'graph>) -> ArenaRef {
        debug_assert!(self.composition(digest).is_none(), "a syntax composes once");
        let bytes = facts.bytes();
        let facts = Rc::new(facts);
        let at = self.laid(ArenaRef::ROOT, bytes, Kind::Composition { digest, facts });
        self.compositions.insert(digest, at);
        at
    }

    /// A slot for a new arena node of `kind` weighing `bytes` past its books, under `parent`.
    fn laid(&mut self, parent: ArenaRef, bytes: usize, kind: Kind<'graph>) -> ArenaRef {
        let bytes = size_of::<ArenaNode<'_>>() + bytes;
        self.clock += 1;
        let node = ArenaNode {
            parent,
            kind,
            borrowers: 0,
            held_children: 0,
            children: 0,
            pinners: 0,
            held_pinners: 0,
            used: self.clock,
            bytes,
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
    /// the turn up through every parent it turns in turn, and to every composition each pins.
    fn turned(&mut self, mut at: ArenaRef, held: bool) {
        loop {
            let node = self.node(at).expect("a turning arena node is live");
            let (bytes, parent) = (node.bytes, node.parent);
            let pins: SmallVec<[ArenaRef; 1]> = node.pins().into();
            if held {
                self.held += bytes;
            } else {
                self.held -= bytes;
            }
            // A pinned composition is a child of the root, so its own turn goes no further.
            for pin in pins {
                let pinned = self.node_mut(pin).expect("a pinned composition stays live");
                let was = pinned.is_held();
                if held {
                    pinned.held_pinners += 1;
                } else {
                    pinned.held_pinners -= 1;
                }
                if was != pinned.is_held() {
                    let bytes = pinned.bytes;
                    if held {
                        self.held += bytes;
                    } else {
                        self.held -= bytes;
                    }
                }
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
    /// budget, and return how many went. Each victim is found by a scan of every live arena node,
    /// held ones included: a miss usually evicts one or none, and the live nodes are what the
    /// budget keeps beside what live frames hold.
    pub(super) fn evict_to_budget(&mut self) -> usize {
        let mut evicted = 0;
        while self.total - self.held > self.budget.bytes.get() {
            let victim = (self.slots.iter().enumerate())
                .filter_map(|(slot, entry)| Some((slot, entry.generation, entry.node.as_ref()?)))
                .filter(|(_, _, node)| !node.is_held() && node.children == 0 && node.pinners == 0)
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
            !node.is_held() && node.children == 0 && node.pinners == 0,
            "eviction takes unheld, unpinned leaves"
        );
        entry.generation += 1;
        self.free.push(slot);
        self.total -= node.bytes;
        match &node.kind {
            Kind::Settlement { key, .. } => self.children.remove(&(node.parent, *key)),
            Kind::Composition { digest, .. } => self.compositions.remove(digest),
        };
        for pin in node.pins() {
            let pinned = self
                .node_mut(*pin)
                .expect("a pinned composition outlives its pinners");
            pinned.pinners -= 1;
        }
        if let Some(parent) = self.node_mut(node.parent) {
            parent.children -= 1;
        }
    }

    /// How many settlements are live.
    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.children.len()
    }

    /// How many compositions are live.
    #[cfg(test)]
    pub(super) fn compositions(&self) -> usize {
        self.compositions.len()
    }

    /// How many live settlements lie directly under a composition, and how many live compositions
    /// some settlement pins.
    #[cfg(test)]
    pub(super) fn under_compositions(&self) -> (usize, usize) {
        let compositions = || (self.compositions.values()).filter_map(|at| self.node(*at));
        (
            compositions().map(|node| node.children).sum(),
            compositions().filter(|node| node.pinners > 0).count(),
        )
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
