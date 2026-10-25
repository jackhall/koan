//! **Group numbering**: the order a scheme's variables take in its group, read off the body alone,
//! so two schemes that differ only in their variables' names or declaration order intern to one
//! handle ([laws.md § One type, one handle](laws.md#one-type-one-handle)).
//!
//! A variable is numbered at its first occurrence in a walk of the positions the scheme binds. The
//! walk reads only what is ordered: shape slots in order, a record's fields, a function's
//! parameters, an application's arguments and pins by name, never in the order a node stores them,
//! which is the order first written. A union or a meet of signatures is a set: its members' stored
//! order is first-written and its canonical order is by handle, and a member's handle depends on
//! the indices the variables inside it hold. So the first pass walks around every set and defers
//! it, which numbers every variable that occurs outside one, as almost every variable does.
//!
//! The second pass orders each deferred set's members by a key that reads no index: the member
//! with each numbered variable at its number and every other at one placeholder per bound. Each
//! member whose key is unique is walked in key order, numbering more variables and deferring the
//! sets nested in it, and the rounds repeat until none moves. Members still tied after that encode
//! a graph over their variables, and only a search settles them: each variable of the first tie
//! is numbered next in turn, the rest settled, and the least result kept. That runs only for a
//! variable that occurs inside sets alone, between members that differ only in which such
//! variables they hold.
//!
//! A variable no position names comes last, by bound: two with one bound are interchangeable.

use crate::bump::{BumpAllocator, BumpVec};

use super::chain::TypeChain;
use super::handle::{Handle, KType, TypeHandle};
use super::node::TypeNode;
use super::stored::Linked;
use super::substitute::substitute_quantified;
use super::tiers::Habitat;
use super::walk::Variance;
use super::walk::unary::{Visit, visit, visit_free_quantified};
use crate::symbols::BinderSymbol;

/// The declaration indices of a group bounded by `bounds`, in group order, numbered over
/// `positions`, the types the scheme binds in the order its doors walk them.
pub(super) fn group_order<'s, 't>(
    types: &TypeChain<'t, impl Habitat<'t>>,
    scratch: BumpAllocator<'s>,
    bounds: &[KType],
    positions: &[Linked<'t>],
) -> BumpVec<'s, usize> {
    let mut numbering = Numbering::new(scratch, bounds.len());
    for position in positions {
        numbering.walk(scratch, *position);
    }
    let settled = settle(types, scratch, bounds, positions, numbering);
    settled.order(scratch, bounds)
}

/// How far a group's numbering has got: each declared variable's number (`usize::MAX` until it
/// has one), the next number, the sets deferred in the order the walk met them, and the set
/// members walked already.
#[derive(Clone)]
struct Numbering<'s, 't> {
    ranks: BumpVec<'s, usize>,
    next: usize,
    pending: BumpVec<'s, Linked<'t>>,
    walked: BumpVec<'s, Linked<'t>>,
}

impl<'s, 't> Numbering<'s, 't> {
    fn new(scratch: BumpAllocator<'s>, arity: usize) -> Self {
        let mut ranks = BumpVec::with_capacity_in(arity, scratch);
        ranks.resize(arity, usize::MAX);
        Numbering {
            ranks,
            next: 0,
            pending: BumpVec::new_in(scratch),
            walked: BumpVec::new_in(scratch),
        }
    }

    fn number(&mut self, index: usize) {
        if let Some(rank) = self.ranks.get_mut(index)
            && *rank == usize::MAX
        {
            *rank = self.next;
            self.next += 1;
        }
    }

    /// Number the variables `kt` holds outside every set, in the ordered walk, and defer each set
    /// it reaches. A nested scheme binds its own group, so the walk skips it.
    fn walk(&mut self, scratch: BumpAllocator<'_>, kt: Linked<'t>) {
        visit(scratch, kt, &mut |kt, node, _| {
            if node.binds_quantifiers() {
                return Visit::Skip;
            }
            match *node {
                TypeNode::Quantified { index, .. } => self.number(index),
                TypeNode::Union { .. } | TypeNode::SignatureMeet { .. } => {
                    if !self.pending.contains(&kt) {
                        self.pending.push(kt);
                    }
                }
                TypeNode::Record { fields } => self.by_name(scratch, fields.raw()),
                TypeNode::KFunction { params, ret } => {
                    self.by_name(scratch, params.raw());
                    self.walk(scratch, ret);
                }
                TypeNode::ConstructorApply {
                    constructor,
                    arguments,
                } => {
                    self.walk(scratch, constructor);
                    self.by_name(scratch, arguments.raw());
                }
                TypeNode::SignatureApply { pins, .. } => self.by_name(scratch, pins.raw()),
                _ => return Visit::Descend,
            }
            Visit::Skip
        });
    }

    /// Walk `fields` by name, as their record's identity reads them.
    fn by_name(&mut self, scratch: BumpAllocator<'_>, fields: &[(BinderSymbol, Linked<'t>)]) {
        let mut sorted = BumpVec::with_capacity_in(fields.len(), scratch);
        sorted.extend_from_slice(fields);
        sorted.sort_unstable_by_key(|(name, _)| name.symbol());
        for (_, kt) in sorted.iter() {
            self.walk(scratch, *kt);
        }
    }

    /// Whether `kt` holds a variable not yet numbered.
    fn unnumbered_in(&self, scratch: BumpAllocator<'_>, kt: Linked<'t>) -> bool {
        kt.contains_quantified()
            && visit_free_quantified(scratch, kt, Variance::Co, &mut |index, _| match self
                .ranks
                .get(index)
            {
                Some(&usize::MAX) => Visit::Stop,
                _ => Visit::Skip,
            })
    }

    /// The members of `set` a round still has to walk, each beside its key: those not walked yet
    /// that hold a variable not yet numbered, sorted by key.
    fn open_members(
        &self,
        types: &TypeChain<'t, impl Habitat<'t>>,
        scratch: BumpAllocator<'s>,
        bounds: &[KType],
        set: Linked<'t>,
    ) -> BumpVec<'s, (Handle, Linked<'t>)> {
        let members = match set.view() {
            TypeNode::Union { members } | TypeNode::SignatureMeet { members } => members,
            _ => unreachable!("a deferred set is a union or a meet"),
        };
        // Each numbered variable at its number, every other at one placeholder per bound.
        let arity = self.ranks.len();
        let mut bindings = BumpVec::with_capacity_in(arity, scratch);
        bindings.extend((0..arity).map(|index| {
            let rank = match self.ranks[index] {
                usize::MAX => arity,
                rank => rank,
            };
            types.quantified_node(rank, bounds[index])
        }));
        let mut open = BumpVec::new_in(scratch);
        for member in members.iter().copied() {
            if !self.walked.contains(&member) && self.unnumbered_in(scratch, member) {
                let key = substitute_quantified(types, scratch, member, &bindings).handle();
                open.push((key, member));
            }
        }
        open.sort_unstable_by_key(|(key, _)| *key);
        open
    }

    /// Declaration indices in group order: numbered variables by number, the rest by bound.
    fn order(&self, scratch: BumpAllocator<'s>, bounds: &[KType]) -> BumpVec<'s, usize> {
        let mut order = BumpVec::with_capacity_in(self.ranks.len(), scratch);
        order.extend(0..self.ranks.len());
        order.sort_by_key(|index| (self.ranks[*index], bounds[*index].raw()));
        order
    }
}

/// `numbering` carried through the deferred sets until every variable that occurs has a number.
fn settle<'s, 't>(
    types: &TypeChain<'t, impl Habitat<'t>>,
    scratch: BumpAllocator<'s>,
    bounds: &[KType],
    positions: &[Linked<'t>],
    mut numbering: Numbering<'s, 't>,
) -> Numbering<'s, 't> {
    // Rounds: in each deferred set, walk every open member whose key no other open member shares.
    let tie = loop {
        let mut moved = false;
        let mut tie = None;
        let mut at = 0;
        while at < numbering.pending.len() {
            let set = numbering.pending[at];
            at += 1;
            let open = numbering.open_members(types, scratch, bounds, set);
            for (index, (key, member)) in open.iter().enumerate() {
                let shared = (index > 0 && open[index - 1].0 == *key)
                    || open.get(index + 1).is_some_and(|(next, _)| next == key);
                if shared {
                    tie.get_or_insert(set);
                } else {
                    numbering.walked.push(*member);
                    numbering.walk(scratch, *member);
                    moved = true;
                }
            }
        }
        if !moved {
            break tie;
        }
    };
    let Some(set) = tie else {
        return numbering;
    };
    // A tie no round moves: number each variable of the first tied run next in turn, settle the
    // rest, and keep the least result.
    let open = numbering.open_members(types, scratch, bounds, set);
    let run = open
        .iter()
        .take_while(|(key, _)| *key == open[0].0)
        .map(|(_, member)| *member);
    let mut candidates = BumpVec::new_in(scratch);
    for member in run {
        visit_free_quantified(scratch, member, Variance::Co, &mut |index, _| {
            if numbering.ranks.get(index) == Some(&usize::MAX) && !candidates.contains(&index) {
                candidates.push(index);
            }
            Visit::Skip
        });
    }
    let mut best: Option<(BumpVec<'s, Handle>, Numbering<'s, 't>)> = None;
    for candidate in candidates.iter().copied() {
        let mut branch = numbering.clone();
        branch.number(candidate);
        let branch = settle(types, scratch, bounds, positions, branch);
        let score = score(types, scratch, bounds, positions, &branch);
        if best.as_ref().is_none_or(|(least, _)| score < *least) {
            best = Some((score, branch));
        }
    }
    best.expect("a tied run holds a variable not yet numbered")
        .1
}

/// What a finished numbering builds, as a sequence that reads no index: the bounds in group order,
/// then each position with the group substituted into that order.
fn score<'s, 't>(
    types: &TypeChain<'t, impl Habitat<'t>>,
    scratch: BumpAllocator<'s>,
    bounds: &[KType],
    positions: &[Linked<'t>],
    numbering: &Numbering<'s, 't>,
) -> BumpVec<'s, Handle> {
    let order = numbering.order(scratch, bounds);
    let mut ranks = BumpVec::with_capacity_in(order.len(), scratch);
    ranks.resize(order.len(), 0);
    for (rank, declared) in order.iter().enumerate() {
        ranks[*declared] = rank;
    }
    let mut bindings = BumpVec::with_capacity_in(order.len(), scratch);
    bindings.extend(
        (ranks.iter().zip(bounds)).map(|(rank, bound)| types.quantified_node(*rank, *bound)),
    );
    let mut score = BumpVec::with_capacity_in(order.len() + positions.len(), scratch);
    score.extend(order.iter().map(|declared| bounds[*declared].raw()));
    score.extend(
        (positions.iter())
            .map(|position| substitute_quantified(types, scratch, *position, &bindings).handle()),
    );
    score
}
