//! `admits_with` — the walk that admits a carried type into a declared position and, where the
//! position quantifies, collects what would solve it.
//!
//! Instead of binding a variable to the first argument it meets, the walk records **every**
//! argument type that reaches the variable as a lower contribution (covariant position) or an
//! upper one (contravariant). [`Collector::solve`] then takes, per variable, the maximum of the
//! lower contributions, else the minimum of the upper ones, else the declared bound. A contribution
//! set with no maximum is a failure, not a join: the solver never mints a union nobody wrote, and
//! admission cannot depend on the order the slots are read.
//!
//! So `(f _ :Elt _ :Elt)` admits `(1, 2)` with `Elt = Number` and `(1, (1 | "x"))` with
//! `Elt = (Number | Str)`, and rejects `(1, "x")`. A caller who wants mixed arguments writes the
//! union in the slot type or in the bound.

use crate::memory::{BumpAllocator, BumpVec};

use super::handle::KType;
use super::node::TypeNode;
use super::order::{dominant, is_subtype_of};
use super::registry::TypeRegistry;
use super::walk::Variance;
use super::walk::binary::{Arm, Lockstep, lockstep};

/// Why a carried type does not fill a declared position. A contribution set rides as a slice of
/// the collector that held it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnifyFailure<'c> {
    /// The two types disagree structurally, or a leaf position is not satisfied — the ordinary type
    /// mismatch.
    Mismatch,
    /// The lower contributions to a variable have no maximum among them. Since [`join`] is
    /// subsumption-or-union, "no maximum" and "the join is a union none of them wrote" are the same
    /// condition.
    ///
    /// [`join`]: super::lattice::join
    NoMaximum {
        index: usize,
        contributions: &'c [KType],
    },
    /// The upper contributions to a variable have no minimum among them — the dual, which is what
    /// forbids an anonymous record or function.
    NoMinimum {
        index: usize,
        contributions: &'c [KType],
    },
    /// A variable's lower solution does not lie under its upper one.
    Disagree {
        index: usize,
        lower: KType,
        upper: KType,
    },
}

/// What a quantified position's arguments contributed, per variable. Every cell lives in the
/// scratch allocator the collector was built over.
///
/// Cells grow on demand, so a walk that does not know the enclosing group's arity up front can
/// still collect; [`new`](Collector::new) takes the arity a call knows, so a variable no argument
/// reached is visible as bound-only.
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
struct Mark {
    cells: usize,
    trail: usize,
}

impl<'s> Collector<'s> {
    /// One empty cell per quantifier — what a call collects its arguments into.
    pub fn new(scratch: BumpAllocator<'s>, arity: usize) -> Self {
        let cells = || {
            let mut cells = BumpVec::with_capacity_in(arity, scratch);
            cells.resize_with(arity, || BumpVec::new_in(scratch));
            cells
        };
        let mut bounds = BumpVec::with_capacity_in(arity, scratch);
        bounds.resize(arity, KType::ANY);
        Collector {
            scratch,
            lower: cells(),
            upper: cells(),
            bounds,
            trail: BumpVec::new_in(scratch),
        }
    }

    /// Where the history stands now.
    fn mark(&self) -> Mark {
        Mark {
            cells: self.bounds.len(),
            trail: self.trail.len(),
        }
    }

    /// Forget every contribution since `mark`. Contributions only ever append, so the trail names
    /// exactly what to pop, and a cell the walk grew since then goes with it.
    fn rollback(&mut self, mark: Mark) {
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

    /// The solution, in canonical quantifier order: per variable the maximum of its lower
    /// contributions, else the minimum of its upper ones, else its declared bound — checked to lie
    /// under both the upper side and the bound. Built in the collector's own scratch.
    ///
    /// Every solution is a contribution or a bound. Nothing here builds a type.
    pub fn solve(&self, types: &TypeRegistry<'_>) -> Result<BumpVec<'s, KType>, UnifyFailure<'_>> {
        let scratch = self.scratch;
        let mut solution = BumpVec::with_capacity_in(self.bounds.len(), scratch);
        for index in 0..self.bounds.len() {
            let bound = self.bounds[index];
            let lower = extremum(types, scratch, &self.lower[index], Bound::Maximum).ok_or(
                UnifyFailure::NoMaximum {
                    index,
                    contributions: &self.lower[index],
                },
            )?;
            let upper = extremum(types, scratch, &self.upper[index], Bound::Minimum).ok_or(
                UnifyFailure::NoMinimum {
                    index,
                    contributions: &self.upper[index],
                },
            )?;
            if let (Some(lower), Some(upper)) = (lower, upper)
                && !is_subtype_of(types, scratch, lower, upper)
            {
                return Err(UnifyFailure::Disagree {
                    index,
                    lower,
                    upper,
                });
            }
            let solved = lower.or(upper).unwrap_or(bound);
            if !is_subtype_of(types, scratch, solved, bound) {
                return Err(UnifyFailure::Mismatch);
            }
            solution.push(solved);
        }
        Ok(solution)
    }
}

/// Which end of a contribution set is being taken.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Bound {
    Maximum,
    Minimum,
}

/// The one member of `contributions` every other member lies under (or over). `Ok(None)` for an
/// empty set, `Err`-worthy `None` when the set has no such member.
fn extremum(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    contributions: &[KType],
    end: Bound,
) -> Option<Option<KType>> {
    if contributions.is_empty() {
        return Some(None);
    }
    dominant(contributions.len(), |candidate, other| match end {
        Bound::Maximum => is_subtype_of(
            types,
            scratch,
            contributions[other],
            contributions[candidate],
        ),
        Bound::Minimum => is_subtype_of(
            types,
            scratch,
            contributions[candidate],
            contributions[other],
        ),
    })
    .map(|found| Some(contributions[found]))
}

/// Does `carried` fill the position `declared`, and what does it contribute to the variables there?
///
/// A declared type holding no free quantifier answers in one step through the ordinary order, which
/// is what keeps every unquantified slot off this walk entirely. Admission itself only ever fails
/// with [`UnifyFailure::Mismatch`]; the contribution-set failures are [`Collector::solve`]'s.
pub fn admits_with(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    declared: KType,
    carried: KType,
    variance: Variance,
    collector: &mut Collector<'_>,
) -> Result<(), UnifyFailure<'static>> {
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
/// Binding is the last resort. A member that admits without touching a variable makes the stronger
/// claim, and trying a free variable first would let it swallow a member that matches exactly —
/// which is what would make `(Elt | Number)` fail to admit itself, since `Elt` would take the
/// `Number` contribution and leave the variable with two contributions and no maximum.
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

type Admission = Result<(), UnifyFailure<'static>>;

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
                Variance::Co => is_subtype_of(types, scratch, carried, declared),
                Variance::Contra => is_subtype_of(types, scratch, declared, carried),
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
        _scratch: BumpAllocator<'_>,
        _declared: KType,
        carried: KType,
        v: Variance,
    ) -> Admission {
        // A deferred FN return is a per-call-elaborated placeholder: it admits nothing on its own,
        // and a return position carrying one has nothing yet to disagree with.
        let deferred = matches!(types.node(carried), TypeNode::DeferredReturn(_));
        if deferred && v == Variance::Co {
            return Ok(());
        }
        Err(UnifyFailure::Mismatch)
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
            let mut admitted = false;
            for option in most_determined_first(types, scratch, declared, *one).iter() {
                let mark = self.collector.mark();
                match recurse(self, *option, *one, v) {
                    Ok(()) => {
                        admitted = true;
                        break;
                    }
                    Err(_) => self.collector.rollback(mark),
                }
            }
            if !admitted {
                return Err(UnifyFailure::Mismatch);
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
