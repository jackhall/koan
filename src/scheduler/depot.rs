//! The build depot: the compositions a graph's frames run under, kept as books beside the cell
//! graph rather than as cells in it, and what a request's cell runs under.
//!
//! A depot slot is a composition: the facts of code composed at run time, a child of the root found
//! by its syntax digest — a [`Composed`], a copy of its syntax and the shapes and records built over
//! it in program storage of its own, which a step under it reads as its chain's base in place of
//! the load. The books hold the digest alone: the syntax a miss copies comes from the code value or
//! callee that asked, through the cell that bounces it ([`ComposedSyntax`]). A run cell born under
//! a depot slot borrows it for its life, through the drain, so a borrowed depot slot is never
//! evicted. Each slot carries a generation, bumped when its depot slot is evicted, so a
//! [`DepotRef`] held past an eviction names nothing rather than the slot's next tenant.
//!
//! A call's frame runs under the depot slot its callee was born under, or the root where that is
//! gone; any other cell runs under what its asker runs under ([`Lending`]). An `EVAL` of code
//! composed at run time runs under its composition, found by the code's syntax digest. The books
//! hold digests, not syntax, so a miss births the cell under the root, and its first step —
//! holding the code value, or the callee whose body record carries its root — copies the syntax and
//! bounces into itself with the copy ([`Compose::Owned`]), which the drain builds the composition
//! over through the program's [`Composes`] door.
//!
//! **Eviction** runs after a miss: while the unheld bytes exceed the [`BuildBudget`], the unheld
//! depot slot with the oldest use goes. Each victim is found by a linear scan of the live depot
//! slots, held ones included. See [build/README.md § The store](../build/README.md#the-store).

use std::collections::HashMap;
use std::marker::PhantomData;
use std::rc::Rc;

use self_cell::self_cell;

#[cfg(test)]
use crate::build::lay::BindingGraph;
use crate::build::lay::{BaseInstances, GraphCell};
use crate::memory::Bump;
use crate::scope::{ComposedSyntax, Composition};
use crate::values::{BuildBudget, ContentDigest, DepotRef};

/// What a request's cell runs under, resolved by the drain at its creation.
pub(super) enum Lending {
    /// What its asker runs under, as a top-level statement's evaluation or a written quote's
    /// `EVAL` does.
    Parent,
    /// A call's frame: the depot slot its callee was born under, where it is live, and the root
    /// otherwise.
    Call(DepotRef),
    /// The composition of code composed at run time, found by its syntax digest and stamped, or
    /// built over the syntax a bounce copied.
    Compose(Compose),
}

/// What a cell running composed code asks of the drain: its composition by syntax digest — the
/// root, where none is live, so the cell's first step bounces — or built over the syntax that
/// bounce copied, where a sibling has not built it meanwhile.
pub(super) enum Compose {
    Find(ContentDigest),
    Owned(ComposedSyntax),
}

/// The door a miss composes through, which the program hands the graph at load: the composition
/// of `syntax`, code composed at run time copied into storage of its own: its shapes built alone
/// over the copy and walked with nothing bound, laid in that storage, its walk interning through a
/// chain laid in `bump`, where a term of each type its facts name past the agent tier is laid too.
/// `witness` is that `'graph` outlives the storage.
pub trait Composes<'graph> {
    fn compose<'a>(
        &self,
        syntax: &'a ComposedSyntax,
        home: ComposedHome<'a>,
        witness: &'a &'graph (),
    ) -> ComposedFacts<'a>;
}

/// Where a composition's facts and instances are laid: the bump its walk interns through and its
/// terms rest in, the cell its binding graph is set in, the bump its instances lend, and the bytes
/// their answers rest within.
#[derive(Clone, Copy)]
pub struct ComposedHome<'a> {
    pub bump: &'a Bump,
    pub laid: &'a crate::build::lay::GraphCell,
    pub lend: &'a Bump,
    pub budget: usize,
}

/// A composition's facts beside the instances of its
/// [binding graph](crate::build::lay::BindingGraph), which its depot slot owns and drops: none
/// where its code is refused.
pub type ComposedFacts<'a> = (
    Composition<'a>,
    Option<&'a dyn crate::build::lay::BaseInstances>,
);

/// The door of a graph that runs no program: it composes no code.
pub(crate) struct Uncomposed;

impl<'graph> Composes<'graph> for Uncomposed {
    fn compose<'a>(
        &self,
        _: &'a ComposedSyntax,
        _: ComposedHome<'a>,
        _: &'a &'graph (),
    ) -> ComposedFacts<'a> {
        unreachable!("a graph that runs no program runs no composed code")
    }
}

/// A graph's depot slots, their books, and the budget their unheld bytes stay within.
pub(super) struct BuildDepot<'graph> {
    slots: Vec<Slot<'graph>>,
    /// Slots holding no depot slot, reused before the slab grows.
    free: Vec<u32>,
    /// Every live composition, by its syntax digest.
    compositions: HashMap<ContentDigest, DepotRef>,
    budget: BuildBudget,
    /// Bytes of every live depot slot.
    total: usize,
    /// Bytes of every held depot slot.
    held: usize,
    /// The use counter each stamp takes the next value of.
    clock: u64,
}

/// One slot of the depot: its generation, and the depot slot it holds, if any.
struct Slot<'graph> {
    generation: u32,
    deposit: Option<Deposit<'graph>>,
}

/// A depot slot's books: who holds it, when it was last used, and what it weighs, beside its
/// facts.
struct Deposit<'graph> {
    digest: ContentDigest,
    facts: Rc<Composed<'graph>>,
    /// Run cells borrowing it.
    borrowers: usize,
    /// The clock at its last use: a miss, or a hit.
    used: u64,
    bytes: usize,
}

/// What a step under a depot slot reads facts through: the composition its chain rests on, by
/// its syntax digest, if not the load, with the instances of its binding graph.
pub(super) struct Door<'a> {
    pub composition: Option<(ContentDigest, &'a Composition<'a>)>,
    pub instances: Option<&'a dyn BaseInstances>,
}

/// What a composition's facts are laid in: its syntax's own program storage, which its shapes and
/// records rest in beside the copy, as the load's rest in the program's, a bump its walk
/// interns through and its terms are laid in, the cell its binding graph is set in, and the bump
/// its instances lend, within `budget` bytes of answers. Its facts may read what lives at
/// `'graph`, which outlives it.
pub(super) struct ComposedOwner<'graph> {
    syntax: ComposedSyntax,
    bump: Bump,
    laid: GraphCell,
    lend: Bump,
    budget: usize,
    graph: PhantomData<&'graph ()>,
}

self_cell!(
    /// A composition's facts beside its binding graph's instances, laid in the storage its owner
    /// keeps. A step reads them only borrowed from the depot.
    pub(super) struct Composed<'graph> {
        owner: ComposedOwner<'graph>,
        #[covariant]
        dependent: ComposedFacts,
    }
);

impl<'graph> Composed<'graph> {
    /// The facts `compose` lays over `syntax`, in its storage and beside fresh bumps, its
    /// instances' answers resting within `budget` bytes, handed the witness that `'graph`
    /// outlives both.
    pub(super) fn build(
        syntax: ComposedSyntax,
        budget: usize,
        compose: impl for<'a> FnOnce(
            &'a ComposedSyntax,
            ComposedHome<'a>,
            &'a &'graph (),
        ) -> ComposedFacts<'a>,
    ) -> Self {
        let owner = ComposedOwner {
            syntax,
            bump: Bump::new(),
            laid: GraphCell::new(),
            lend: Bump::new(),
            budget,
            graph: PhantomData,
        };
        Composed::new(owner, |owner| {
            let home = ComposedHome {
                bump: &owner.bump,
                laid: &owner.laid,
                lend: &owner.lend,
                budget: owner.budget,
            };
            compose(&owner.syntax, home, &&())
        })
    }

    /// The facts themselves.
    pub(super) fn facts(&self) -> &Composition<'_> {
        &self.borrow_dependent().0
    }

    /// The instances of its binding graph: none where its code is refused.
    pub(super) fn instances(&self) -> Option<&dyn BaseInstances> {
        self.borrow_dependent().1
    }

    /// Its binding graph: none where its code is refused.
    #[cfg(test)]
    pub(super) fn binding(&self) -> Option<BindingGraph<'_>> {
        self.instances().map(BaseInstances::graph)
    }

    /// What the facts weigh: their storage's bytes and their bump's.
    fn bytes(&self) -> usize {
        let owner = self.borrow_owner();
        owner.syntax.allocated_bytes() + owner.bump.allocated_bytes()
    }
}

impl<'graph> BuildDepot<'graph> {
    /// The budget its unheld bytes stay within.
    pub(super) fn budget(&self) -> BuildBudget {
        self.budget
    }

    /// An empty depot whose unheld bytes stay within `budget`.
    pub(super) fn new(budget: BuildBudget) -> Self {
        BuildDepot {
            slots: Vec::new(),
            free: Vec::new(),
            compositions: HashMap::new(),
            budget,
            total: 0,
            held: 0,
            clock: 0,
        }
    }

    fn deposit(&self, at: DepotRef) -> Option<&Deposit<'graph>> {
        let (slot, generation) = at.opened()?;
        let slot = self.slots.get(slot as usize)?;
        (slot.generation == generation).then_some(slot.deposit.as_ref())?
    }

    fn deposit_mut(&mut self, at: DepotRef) -> Option<&mut Deposit<'graph>> {
        let (slot, generation) = at.opened()?;
        let slot = self.slots.get_mut(slot as usize)?;
        (slot.generation == generation).then_some(slot.deposit.as_mut())?
    }

    /// Whether `at` names a live depot slot. The root always does.
    pub(super) fn is_live(&self, at: DepotRef) -> bool {
        at.opened().is_none() || self.deposit(at).is_some()
    }

    /// What a step under the live `at` reads: nothing past the load for the root.
    pub(super) fn door(&self, at: DepotRef) -> Door<'_> {
        match self.deposit(at) {
            None => Door {
                composition: None,
                instances: None,
            },
            Some(deposit) => Door {
                composition: Some((deposit.digest, deposit.facts.facts())),
                instances: deposit.facts.instances(),
            },
        }
    }

    /// The live composition of the code whose syntax digest is `digest`, if any. Stamps nothing.
    pub(super) fn composition(&self, digest: ContentDigest) -> Option<DepotRef> {
        self.compositions.get(&digest).copied()
    }

    /// A new composition of the code whose syntax digest is `digest`, holding `facts`, unheld and
    /// stamped. Evicts nothing: the drain calls [`evict_to_budget`](Self::evict_to_budget) once a
    /// cell borrows it.
    pub(super) fn compose(&mut self, digest: ContentDigest, facts: Composed<'graph>) -> DepotRef {
        debug_assert!(self.composition(digest).is_none(), "a syntax composes once");
        let bytes = size_of::<Deposit<'_>>() + facts.bytes();
        self.clock += 1;
        let deposit = Deposit {
            digest,
            facts: Rc::new(facts),
            borrowers: 0,
            used: self.clock,
            bytes,
        };
        let at = match self.free.pop() {
            Some(slot) => {
                let entry = &mut self.slots[slot as usize];
                entry.deposit = Some(deposit);
                DepotRef::of(slot, entry.generation)
            }
            None => {
                let slot = u32::try_from(self.slots.len()).expect("the depot fits u32 slots");
                self.slots.push(Slot {
                    generation: 0,
                    deposit: Some(deposit),
                });
                DepotRef::of(slot, 0)
            }
        };
        self.total += bytes;
        self.compositions.insert(digest, at);
        at
    }

    /// Mark `at` used now: a hit. The root takes no stamp.
    pub(super) fn stamp(&mut self, at: DepotRef) {
        self.clock += 1;
        let clock = self.clock;
        if let Some(deposit) = self.deposit_mut(at) {
            deposit.used = clock;
        }
    }

    /// Count one more run cell borrowing the live `at`. The root is never counted.
    pub(super) fn borrow(&mut self, at: DepotRef) {
        let Some(deposit) = self.deposit_mut(at) else {
            debug_assert!(
                at.opened().is_none(),
                "a run cell borrows a live depot slot"
            );
            return;
        };
        deposit.borrowers += 1;
        if deposit.borrowers == 1 {
            self.held += deposit.bytes;
        }
    }

    /// Count one fewer run cell borrowing `at`, which it borrowed.
    pub(super) fn release(&mut self, at: DepotRef) {
        let Some(deposit) = self.deposit_mut(at) else {
            debug_assert!(at.opened().is_none(), "a borrowed depot slot stays live");
            return;
        };
        deposit.borrowers -= 1;
        if deposit.borrowers == 0 {
            self.held -= deposit.bytes;
        }
    }

    /// Evict unheld depot slots, least recently used first, until the unheld bytes are within the
    /// budget, and return how many went. Each victim is found by a scan of every live depot slot,
    /// held ones included: a miss usually evicts one or none, and the live slots are what the
    /// budget keeps beside what live frames hold.
    pub(super) fn evict_to_budget(&mut self) -> usize {
        let mut evicted = 0;
        while self.total - self.held > self.budget.bytes.get() {
            let victim = (self.slots.iter().enumerate())
                .filter_map(|(slot, entry)| Some((slot, entry.generation, entry.deposit.as_ref()?)))
                .filter(|(_, _, deposit)| deposit.borrowers == 0)
                .min_by_key(|(_, _, deposit)| deposit.used)
                .map(|(slot, generation, _)| DepotRef::of(slot as u32, generation));
            let Some(victim) = victim else {
                break;
            };
            self.evict(victim);
            evicted += 1;
        }
        evicted
    }

    /// Remove the unheld `at`: its slot's generation moves on, so every handle to it goes stale.
    fn evict(&mut self, at: DepotRef) {
        let (slot, _) = at.opened().expect("the root is never evicted");
        let entry = &mut self.slots[slot as usize];
        let deposit = entry.deposit.take().expect("an evicted depot slot is live");
        debug_assert!(deposit.borrowers == 0, "eviction takes unheld depot slots");
        entry.generation += 1;
        self.free.push(slot);
        self.total -= deposit.bytes;
        self.compositions.remove(&deposit.digest);
    }

    /// How many compositions are live.
    #[cfg(test)]
    pub(super) fn compositions(&self) -> usize {
        self.compositions.len()
    }

    /// How many times the answer function ran for an instance of a live composition's graph.
    #[cfg(test)]
    pub(super) fn composed_runs(&self) -> u64 {
        (self.compositions.values())
            .filter_map(|at| self.deposit(*at)?.facts.instances().map(|each| each.runs()))
            .sum()
    }

    /// The bytes of live depot slots no run cell holds.
    #[cfg(test)]
    pub(super) fn unheld_bytes(&self) -> usize {
        self.total - self.held
    }
}
