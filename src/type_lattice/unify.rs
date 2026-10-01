//! `admits_with` — the walk that admits a carried type into a declared position and, where the
//! position quantifies, collects what would solve it.
//!
//! Instead of binding a variable to the first argument it meets, the walk records **every**
//! argument type that reaches the variable as a lower contribution (covariant position) or an
//! upper one (contravariant). [`Collector::solve`] bounds each variable by a pair — below by the
//! join of its lower contributions, above by the meet of its upper ones and its bound — and binds
//! the pair's least instance. A solve fails only where the set the pair denotes is empty, and
//! admission cannot depend on the order the slots are read.
//!
//! So `(f _ :Elt _ :Elt)` admits `(1, 2)` with `Elt = Number` and `(1, "x")` with
//! `Elt = (Number | Str)`. A head that wants one type across slots puts them in separate
//! [priority classes](super::ranking).
//!
//! A construction collects through [`Collector::least`], whose unreached variables are bounded by
//! `Never` rather than their declared bound: a family is covariant in its parameters, so its least
//! instance is the one the payload asks for.
//!
//! [`intervals`] reads a solve over static types as an [`Interval`] per variable: where every
//! solution a solve over arguments within those static types can reach lies.

use crate::memory::{BumpAllocator, BumpVec};

use super::handle::KType;
use super::lattice::{join_iter, meet};
use super::node::TypeNode;
use super::order::fits;
use super::registry::TypeRegistry;
use super::walk::Variance;
use super::walk::binary::{Arm, Lockstep, lockstep};
use super::walk::unary::{Visit, visit_in};

/// Why a carried type does not fill a declared position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnifyFailure {
    /// The two types disagree structurally, or a leaf position is not satisfied — the ordinary type
    /// mismatch.
    Mismatch,
    /// A variable's lower end — the join of its lower contributions — lies above `upper`, one of its
    /// upper contributions or its bound: the set its pair denotes is empty.
    Disagree {
        index: usize,
        lower: KType,
        upper: KType,
    },
}

/// A range of types: every solution a solve can reach over arguments within the static types it
/// collected, or every type a run carries where a static type is read. An end is a bound, not a
/// solution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Interval {
    pub lower: KType,
    pub upper: KType,
}

impl Interval {
    /// Anything under `bound`.
    pub fn within(bound: KType) -> Self {
        Interval {
            lower: KType::NEVER,
            upper: bound,
        }
    }

    /// Exactly `kt`.
    pub fn point(kt: KType) -> Self {
        Interval {
            lower: kt,
            upper: kt,
        }
    }

    pub fn is_exact(self) -> bool {
        self.lower == self.upper
    }
}

/// Each variable's interval, from the `solution` a solve over the positions `declared` gave, the
/// group bounded by `bounds`. Where every argument naming a variable was `exact`, each interval is
/// its solution. Otherwise the upper end is the solution where some covariant position under no
/// union names the variable — every admitted argument reaches one — and the bound elsewhere; and
/// the lower end is `Never` where some covariant position names it, and the solution elsewhere.
pub fn intervals<'s>(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'s>,
    declared: &[KType],
    bounds: &[KType],
    solution: &[KType],
    exact: bool,
) -> BumpVec<'s, Interval> {
    let mut out = BumpVec::with_capacity_in(solution.len(), scratch);
    out.extend(solution.iter().enumerate().map(|(index, solved)| {
        if exact {
            return Interval::point(*solved);
        }
        let mut names = Names::default();
        for position in declared {
            names.over(types, scratch, *position, index, Variance::Co, false);
        }
        Interval {
            lower: if names.named { KType::NEVER } else { *solved },
            upper: if names.reached {
                *solved
            } else {
                bounds.get(index).copied().unwrap_or(KType::ANY)
            },
        }
    }));
    out
}

/// Where one variable of a declared group stands at covariant positions: whether one names it, and
/// whether one under no union does.
#[derive(Default)]
struct Names {
    named: bool,
    reached: bool,
}

impl Names {
    /// Read `kt`, at `variance` and under a union or not, for the `index`-th variable. A nested
    /// binder's group shadows the declared one.
    fn over(
        &mut self,
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
        kt: KType,
        index: usize,
        variance: Variance,
        under_union: bool,
    ) {
        visit_in(types, scratch, kt, variance, &mut |_, node, context| {
            if node.binds_quantifiers() {
                return Visit::Skip;
            }
            match *node {
                TypeNode::Quantified { index: found, .. } => {
                    if found == index && context.variance() == Variance::Co {
                        self.named = true;
                        self.reached |= !under_union;
                    }
                    Visit::Skip
                }
                TypeNode::Union { members } => {
                    for member in members {
                        self.over(types, scratch, *member, index, context.variance(), true);
                    }
                    Visit::Skip
                }
                _ => Visit::Descend,
            }
        });
    }
}

/// What a quantified position's arguments contributed, per variable. Every cell lives in the
/// scratch allocator the collector was built over.
///
/// Cells grow on demand, so a walk that does not know the enclosing group's arity up front can
/// still collect; [`new`](Collector::new) takes the bounds of the group a call knows, so a variable
/// no argument reached solves to its bound.
pub struct Collector<'s> {
    scratch: BumpAllocator<'s>,
    lower: BumpVec<'s, BumpVec<'s, KType>>,
    upper: BumpVec<'s, BumpVec<'s, KType>>,
    /// Each variable's declared bound, recorded off the `Quantified` node when a contribution
    /// reached it.
    bounds: BumpVec<'s, KType>,
    /// Every contribution in arrival order — the cell it landed in and the bound that cell held
    /// before — so a [`rollback`](Self::rollback) pops exactly what a rejected attempt added.
    trail: BumpVec<'s, (usize, Variance, KType)>,
}

/// A point in a [`Collector`]'s history, for [`Collector::rollback`].
#[derive(Clone, Copy)]
pub(super) struct Mark {
    cells: usize,
    trail: usize,
}

impl<'s> Collector<'s> {
    /// One empty cell per variable of a group bounded by `bounds` — what a call collects its
    /// arguments into. A variable no contribution reaches solves to its bound.
    pub fn new(scratch: BumpAllocator<'s>, bounds: &[KType]) -> Self {
        Self::over(scratch, bounds.iter().copied())
    }

    /// One empty cell per quantifier, each bounded by [`KType::NEVER`] until a contribution reaches
    /// it — what a construction collects a payload into. A family is covariant in its parameters,
    /// so its least instance is the one the payload asks for, and a parameter the payload never
    /// reaches is bounded by `Never`.
    pub fn least(scratch: BumpAllocator<'s>, arity: usize) -> Self {
        Self::over(scratch, std::iter::repeat_n(KType::NEVER, arity))
    }

    /// One empty cell per bound, each cell holding its bound.
    fn over(scratch: BumpAllocator<'s>, bounds: impl ExactSizeIterator<Item = KType>) -> Self {
        let arity = bounds.len();
        let cells = || {
            let mut cells = BumpVec::with_capacity_in(arity, scratch);
            cells.resize_with(arity, || BumpVec::new_in(scratch));
            cells
        };
        let mut held = BumpVec::with_capacity_in(arity, scratch);
        held.extend(bounds);
        Collector {
            scratch,
            lower: cells(),
            upper: cells(),
            bounds: held,
            trail: BumpVec::new_in(scratch),
        }
    }

    /// Where the history stands now.
    pub(super) fn mark(&self) -> Mark {
        Mark {
            cells: self.bounds.len(),
            trail: self.trail.len(),
        }
    }

    /// Forget every contribution since `mark`. Contributions only ever append, so the trail names
    /// exactly what to pop, and a cell the walk grew since then goes with it.
    pub(super) fn rollback(&mut self, mark: Mark) {
        while self.trail.len() > mark.trail {
            let (index, variance, previous) = self.trail.pop().expect("the trail reaches the mark");
            match variance {
                Variance::Co => self.lower[index].pop(),
                Variance::Contra => self.upper[index].pop(),
            };
            self.bounds[index] = previous;
        }
        self.lower.truncate(mark.cells);
        self.upper.truncate(mark.cells);
        self.bounds.truncate(mark.cells);
    }

    /// Pin the `index`-th variable to `to`, as though `to` had reached it at both polarities: every
    /// later contribution must then lie on the right side of `to` for the variable to solve, and it
    /// solves to `to`. How a class-by-class admission holds a variable an earlier class solved.
    pub(super) fn pin(&mut self, index: usize, bound: KType, to: KType) {
        self.contribute(index, bound, to, Variance::Co);
        self.contribute(index, bound, to, Variance::Contra);
    }

    /// Record that `carried` reached the `index`-th variable at `variance`.
    fn contribute(&mut self, index: usize, bound: KType, carried: KType, variance: Variance) {
        if self.lower.len() <= index {
            let scratch = self.scratch;
            self.lower
                .resize_with(index + 1, || BumpVec::new_in(scratch));
            self.upper
                .resize_with(index + 1, || BumpVec::new_in(scratch));
            self.bounds.resize(index + 1, KType::ANY);
        }
        let cell = match variance {
            Variance::Co => &mut self.lower[index],
            Variance::Contra => &mut self.upper[index],
        };
        if cell.contains(&carried) {
            return;
        }
        cell.push(carried);
        let previous = std::mem::replace(&mut self.bounds[index], bound);
        self.trail.push((index, variance, previous));
    }

    /// The lower and upper contributions to the `index`-th variable, in arrival order — what
    /// reached it at a covariant position and what reached it at a contravariant one. Empty slices
    /// for an index no argument reached.
    #[cfg(test)]
    pub(super) fn contributions(&self, index: usize) -> (&[KType], &[KType]) {
        fn cell<'c>(cells: &'c [BumpVec<'_, KType>], index: usize) -> &'c [KType] {
            cells.get(index).map_or(&[], |cell| cell.as_slice())
        }
        (cell(&self.lower, index), cell(&self.upper, index))
    }

    /// The bound recorded for the `index`-th variable, or [`KType::ANY`] for an index no
    /// contribution reached.
    #[cfg(test)]
    pub(super) fn bound(&self, index: usize) -> KType {
        self.bounds.get(index).copied().unwrap_or(KType::ANY)
    }

    /// The solution, in canonical quantifier order: per variable the least instance of its pair —
    /// the join of its lower contributions where any reached it, else the meet of its upper ones
    /// and its bound. The join must lie under each upper contribution and the bound, or the set the
    /// pair denotes is empty. Built in the collector's own scratch.
    pub fn solve(&self, types: &TypeRegistry<'_>) -> Result<BumpVec<'s, KType>, UnifyFailure> {
        let scratch = self.scratch;
        let mut solution = BumpVec::with_capacity_in(self.bounds.len(), scratch);
        for index in 0..self.bounds.len() {
            let bound = self.bounds[index];
            let (lower, upper) = (&self.lower[index], &self.upper[index]);
            let solved = if lower.is_empty() {
                upper
                    .iter()
                    .fold(bound, |met, each| meet(types, scratch, met, *each))
            } else {
                let joined = join_iter(types, scratch, lower.iter().copied());
                // Each ceiling on its own: a meet may land below the greatest lower bound.
                for ceiling in upper.iter().copied().chain([bound]) {
                    if !fits(types, scratch, joined, ceiling) {
                        return Err(UnifyFailure::Disagree {
                            index,
                            lower: joined,
                            upper: ceiling,
                        });
                    }
                }
                joined
            };
            solution.push(solved);
        }
        Ok(solution)
    }
}

/// Does `carried` fill the position `declared`, and what does it contribute to the variables there?
///
/// A declared type holding no free quantifier answers in one step through the ordinary order, which
/// is what keeps every unquantified slot off this walk entirely. Admission itself only ever fails
/// with [`UnifyFailure::Mismatch`]; [`UnifyFailure::Disagree`] is [`Collector::solve`]'s.
pub fn admits_with(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    declared: KType,
    carried: KType,
    variance: Variance,
    collector: &mut Collector<'_>,
) -> Result<(), UnifyFailure> {
    lockstep(
        types,
        scratch,
        declared,
        carried,
        variance,
        &mut Admits { collector },
    )
}

/// The declared members of a union in the order they are tried against one carried member: an exact
/// match first, then the members with nothing to solve, then the rest.
///
/// Binding is the last resort. A member that admits without touching a variable gives a smaller
/// least instance: trying a free variable first would let it swallow a member that matches exactly,
/// so `(Elt | Number)` admitting `Number` would bind `Elt` larger than it needs to be.
fn most_determined_first<'s>(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'s>,
    declared: &[KType],
    carried: KType,
) -> BumpVec<'s, KType> {
    let mut order = BumpVec::with_capacity_in(declared.len(), scratch);
    if declared.contains(&carried) {
        order.push(carried);
    }
    for member in declared {
        if *member != carried && !types.contains_quantified(*member) {
            order.push(*member);
        }
    }
    for member in declared {
        if *member != carried && types.contains_quantified(*member) {
            order.push(*member);
        }
    }
    order
}

/// The collecting [`Lockstep`] instance. It holds the caller's collector so a declared-side union
/// can try each member in turn, rolling back what a rejected one contributed and keeping the first
/// that admits.
struct Admits<'c, 's> {
    collector: &'c mut Collector<'s>,
}

type Admission = Result<(), UnifyFailure>;

impl Admits<'_, '_> {
    /// Whether some member of `declared` admits the one carried type `one`, tried most determined
    /// first. A rejected member's contributions are rolled back; the admitting member's stay.
    fn admit_one(
        &mut self,
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
        declared: &[KType],
        one: KType,
        v: Variance,
        recurse: &mut dyn FnMut(&mut Self, KType, KType, Variance) -> Admission,
    ) -> bool {
        for option in most_determined_first(types, scratch, declared, one).iter() {
            let mark = self.collector.mark();
            match recurse(self, *option, one, v) {
                Ok(()) => return true,
                Err(_) => self.collector.rollback(mark),
            }
        }
        false
    }
}

impl Lockstep for Admits<'_, '_> {
    type Out = Admission;

    fn enter(
        &mut self,
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
        declared: KType,
        carried: KType,
        v: Variance,
    ) -> Option<Admission> {
        if !types.contains_quantified(declared) {
            let admits = match v {
                Variance::Co => fits(types, scratch, carried, declared),
                Variance::Contra => fits(types, scratch, declared, carried),
            };
            return Some(admits.then_some(()).ok_or(UnifyFailure::Mismatch));
        }
        match types.node(declared) {
            TypeNode::Quantified { index, bound } => {
                self.collector.contribute(index, bound, carried, v);
                Some(Ok(()))
            }
            _ => None,
        }
    }

    fn leaf(
        &mut self,
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
        declared: KType,
        carried: KType,
        v: Variance,
    ) -> Admission {
        // A carried rigid variable fills, at a covariant position, what its bound fills, and at a
        // contravariant one what its lower end fills: below one lie itself and what lies under its
        // lower end.
        let Some(ends) = types.node(carried).rigid_interval() else {
            return Err(UnifyFailure::Mismatch);
        };
        match v {
            Variance::Co => lockstep(types, scratch, declared, ends.upper, v, self),
            Variance::Contra if ends.lower != KType::NEVER => {
                lockstep(types, scratch, declared, ends.lower, v, self)
            }
            Variance::Contra => Err(UnifyFailure::Mismatch),
        }
    }

    fn set_wise(
        &mut self,
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
        declared: &[KType],
        carried: &[KType],
        v: Variance,
        recurse: &mut dyn FnMut(&mut Self, KType, KType, Variance) -> Admission,
    ) -> Admission {
        // Every carried member must be admitted by some declared member. A non-union side arrives
        // as a one-element slice, so this covers a union on either side and on both. The
        // declared-side choice rolls back on rejection, so a rejected member leaves no contribution
        // behind; contributions from every carried member accumulate in the one collector.
        for one in carried {
            if self.admit_one(types, scratch, declared, *one, v, recurse) {
                continue;
            }
            // A carried variable whose bound spans several declared members is admitted through
            // the bound's members, each by some declared member.
            let bound = types.node(*one).rigid_bound();
            let Some(bound) = bound.filter(|_| v == Variance::Co) else {
                return Err(UnifyFailure::Mismatch);
            };
            let TypeNode::Union { members } = types.node(bound) else {
                return Err(UnifyFailure::Mismatch);
            };
            let mark = self.collector.mark();
            for member in members {
                if !self.admit_one(types, scratch, declared, *member, v, recurse) {
                    self.collector.rollback(mark);
                    return Err(UnifyFailure::Mismatch);
                }
            }
        }
        Ok(())
    }

    fn structural(
        &mut self,
        _types: &TypeRegistry<'_>,
        _scratch: BumpAllocator<'_>,
        paired: &[Admission],
        arm: Arm<'_, '_>,
    ) -> Admission {
        if let Some(failure) = paired.iter().find(|out| out.is_err()) {
            return *failure;
        }
        // The width verdict reads `a ≤ b`; a covariant position asks whether the *carried* side
        // lies under the declared one, so its leftovers go in the other order.
        let permitted = match arm.variance {
            Variance::Co => arm.width.permits(arm.only_b, arm.only_a),
            Variance::Contra => arm.width.permits(arm.only_a, arm.only_b),
        };
        permitted.then_some(()).ok_or(UnifyFailure::Mismatch)
    }

    fn short_circuits(&self, out: &Admission) -> bool {
        out.is_err()
    }
}
