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
//! A declared union is admitted through one member, tried most determined first
//! ([`most_determined_first`]): among the members that bind, a strictly more specific one
//! ([`member_at_least`]) first, so the choice never turns on how the union stores its members. Two
//! members neither of which is more specific, that one argument can both admit, **tie**
//! ([`tied_members`]); the elaborator refuses a binder holding such a pair.
//!
//! A construction collects through [`Collector::least`], whose unreached variables are bounded by
//! `Never` rather than their declared bound: a family is covariant in its parameters, so its least
//! instance is the one the payload asks for.
//!
//! [`intervals`] reads a solve over static types as an [`Interval`] per variable: where every
//! solution a solve over arguments within those static types can reach lies.
//!
//! A collector is typed by what it collects. A `Collector<KType>` takes concrete contributions
//! against concrete bounds, so its solution — joins and meets of those — is concrete; a
//! `Collector<Parametric>` may take a variable, which a join keeps beside the rest and the solver's
//! meet relates by the rigid rule. Inside the lattice a collector holds raw handles.
//!
//! Over static types naming lexical variables, [`Collector::reproducible`] reports whether a run,
//! binding each variable, reproduces the solve: not where an admission read a variable through its
//! ends, a variable's answer is a meet over one, or the solve fails at a verdict over one. A closed
//! solve always is.

use crate::bump::{BumpAllocator, BumpVec};

use std::marker::PhantomData;

use super::handle::{Handle, KType, Parametric, TypeHandle, wrap};
use super::lattice::{join_iter, meet_through_variables};
use super::node::TypeNode;
use super::order::fits;
use super::registry::TypeRegistry;
use super::substitute::bound_above;
use super::verdicts::Relation;
use super::walk::Variance;
use super::walk::binary::{Arm, Lockstep, lockstep};
use super::walk::unary::{Visit, visit_in};

/// Why a carried type does not fill a declared position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnifyFailure<T = Parametric> {
    /// The two types disagree structurally, or a leaf position is not satisfied — the ordinary type
    /// mismatch.
    Mismatch,
    /// A variable's lower end — the join of its lower contributions — lies above `upper`, one of its
    /// upper contributions or its bound: the set its pair denotes is empty.
    Disagree { index: usize, lower: T, upper: T },
}

impl UnifyFailure<Handle> {
    fn typed<T: TypeHandle>(self) -> UnifyFailure<T> {
        match self {
            UnifyFailure::Mismatch => UnifyFailure::Mismatch,
            UnifyFailure::Disagree {
                index,
                lower,
                upper,
            } => UnifyFailure::Disagree {
                index,
                lower: wrap(lower),
                upper: wrap(upper),
            },
        }
    }
}

/// A range of types: every solution a solve can reach over arguments within the static types it
/// collected, or every type a run carries where a static type is read. An end is a bound, not a
/// solution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Interval<T = Parametric> {
    pub lower: T,
    pub upper: T,
}

impl<T: TypeHandle> Interval<T> {
    /// Anything under `bound`.
    pub fn within(bound: KType) -> Self {
        Interval {
            lower: wrap(Handle::NEVER),
            upper: wrap(bound.raw()),
        }
    }

    /// Exactly `kt`.
    pub fn point(kt: T) -> Self {
        Interval {
            lower: kt,
            upper: kt,
        }
    }

    pub fn is_exact(self) -> bool {
        self.lower == self.upper
    }

    /// Both ends raw.
    pub(super) fn raw(self) -> Interval<Handle> {
        Interval {
            lower: self.lower.raw(),
            upper: self.upper.raw(),
        }
    }
}

impl Interval<Handle> {
    /// Both ends as `T`; the caller answers for `T`'s promise.
    pub(super) fn typed<T: TypeHandle>(self) -> Interval<T> {
        Interval {
            lower: wrap(self.lower),
            upper: wrap(self.upper),
        }
    }
}

/// A concrete interval stands where a parametric one may.
impl From<Interval<KType>> for Interval<Parametric> {
    fn from(interval: Interval<KType>) -> Self {
        Interval {
            lower: interval.lower.into(),
            upper: interval.upper.into(),
        }
    }
}

/// Each variable's interval, from the `solution` a solve over the positions `declared` gave, the
/// group bounded by `bounds`. Where every argument naming a variable was `exact`, each interval is
/// its solution. Otherwise the upper end is the solution where some covariant position under no
/// union names the variable — every admitted argument reaches one — and the bound elsewhere; and
/// the lower end is `Never` where some covariant position names it, and the solution elsewhere.
///
/// Each end is a solution or a bound, so an interval over a `T` solution is a `T` interval.
pub fn intervals<'s, D: TypeHandle, T: TypeHandle>(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'s>,
    declared: &[D],
    bounds: &[KType],
    solution: &[T],
    exact: bool,
) -> BumpVec<'s, Interval<T>> {
    let mut out = BumpVec::with_capacity_in(solution.len(), scratch);
    out.extend(solution.iter().enumerate().map(|(index, solved)| {
        if exact {
            return Interval::point(*solved);
        }
        let mut names = Names::default();
        for position in declared {
            names.over(types, scratch, position.raw(), index, Variance::Co, false);
        }
        Interval {
            lower: if names.named {
                wrap(Handle::NEVER)
            } else {
                *solved
            },
            upper: if names.reached {
                *solved
            } else {
                wrap(bounds.get(index).copied().unwrap_or(KType::ANY).raw())
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
        kt: Handle,
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
                    for member in members.iter() {
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
///
/// `T` is what the collector takes and solves to: every contribution and pin is a `T`, and every
/// bound a [`KType`], so every join and meet the solve takes is a `T`.
pub struct Collector<'s, T = Parametric> {
    scratch: BumpAllocator<'s>,
    lower: BumpVec<'s, BumpVec<'s, Handle>>,
    upper: BumpVec<'s, BumpVec<'s, Handle>>,
    /// Each variable's declared bound, recorded off the `Quantified` node when a contribution
    /// reached it.
    bounds: BumpVec<'s, KType>,
    /// Every contribution in arrival order — the cell it landed in and the bound that cell held
    /// before — so a [`rollback`](Self::rollback) pops exactly what a rejected attempt added.
    trail: BumpVec<'s, (usize, Variance, KType)>,
    /// Whether some admission read a rigid variable through its ends: `Admits::leaf`, the bound
    /// split in `Admits::set_wise`, or a false verdict over one. Never rolled back — a rejected
    /// union member that read one is a choice some binding makes otherwise.
    bound_read: bool,
    takes: PhantomData<T>,
}

/// A point in a [`Collector`]'s history, for [`Collector::rollback`].
#[derive(Clone, Copy)]
pub(super) struct Mark {
    cells: usize,
    trail: usize,
}

impl<'s, T: TypeHandle> Collector<'s, T> {
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
            bound_read: false,
            takes: PhantomData,
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
    /// exactly what to pop, and a cell the walk grew since then goes with it. A bound read since
    /// then stays recorded.
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
    pub fn pin(&mut self, index: usize, bound: KType, to: T) {
        self.contribute(index, bound, to.raw(), Variance::Co);
        self.contribute(index, bound, to.raw(), Variance::Contra);
    }

    /// Record that `carried` reached the `index`-th variable at `variance`.
    fn contribute(&mut self, index: usize, bound: KType, carried: Handle, variance: Variance) {
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

    /// Whether any contribution, lower or upper, reached the `index`-th variable.
    pub(super) fn reached(&self, index: usize) -> bool {
        let reached =
            |cells: &[BumpVec<'_, Handle>]| cells.get(index).is_some_and(|cell| !cell.is_empty());
        reached(&self.lower) || reached(&self.upper)
    }

    /// The lower and upper contributions to the `index`-th variable, in arrival order — what
    /// reached it at a covariant position and what reached it at a contravariant one. Empty slices
    /// for an index no argument reached.
    #[cfg(test)]
    pub(super) fn contributions(&self, index: usize) -> (&[Handle], &[Handle]) {
        fn cell<'c>(cells: &'c [BumpVec<'_, Handle>], index: usize) -> &'c [Handle] {
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
    pub fn solve(&self, types: &TypeRegistry<'_>) -> Result<BumpVec<'s, T>, UnifyFailure<T>> {
        let mut solution = BumpVec::with_capacity_in(self.bounds.len(), self.scratch);
        for index in 0..self.bounds.len() {
            match self.answer(types, index) {
                Ok(solved) => solution.push(wrap(solved)),
                Err((lower, upper)) => {
                    return Err(UnifyFailure::Disagree {
                        index,
                        lower,
                        upper,
                    }
                    .typed());
                }
            }
        }
        Ok(solution)
    }

    /// Whether a run reproduces this solve: binding each lexical variable the contributions name as
    /// the run does and then solving gives the solution so bound, and fails where this fails. A
    /// contribution handed whole to a cell binds member by member, a join of such keeps each beside
    /// the rest, and a verdict that holds over a variable holds at every binding. What may not: a
    /// read through a variable's ends (`bound_read`), a meet over one, and a verdict that fails
    /// over one.
    pub fn reproducible(&self, types: &TypeRegistry<'_>) -> bool {
        !self.bound_read
            && (0..self.bounds.len()).all(|index| match self.answer(types, index) {
                Ok(_) => {
                    !self.lower[index].is_empty()
                        || self.upper[index]
                            .iter()
                            .all(|upper| !types.contains_rigid(*upper))
                }
                Err((joined, ceiling)) => {
                    !types.contains_rigid(joined) && !types.contains_rigid(ceiling)
                }
            })
    }

    /// The `index`-th variable's answer: the join of its lower contributions where any reached it,
    /// else the meet of its upper ones and its bound; or, where the join does not lie under some
    /// ceiling — an upper contribution or the bound — the join beside the first such ceiling.
    fn answer(&self, types: &TypeRegistry<'_>, index: usize) -> Result<Handle, (Handle, Handle)> {
        let scratch = self.scratch;
        let bound = self.bounds[index].raw();
        let (lower, upper) = (&self.lower[index], &self.upper[index]);
        if lower.is_empty() {
            return Ok(upper.iter().fold(bound, |met, each| {
                meet_through_variables(types, scratch, met, *each)
            }));
        }
        let joined = join_iter(types, scratch, lower.iter().copied());
        // Each ceiling on its own: a meet may land below the greatest lower bound.
        match upper
            .iter()
            .copied()
            .chain([bound])
            .find(|ceiling| !fits(types, scratch, joined, *ceiling))
        {
            Some(ceiling) => Err((joined, ceiling)),
            None => Ok(joined),
        }
    }
}

/// Does `carried` fill the position `declared`, and what does it contribute to the variables there?
///
/// A declared type holding no free quantifier answers in one step through *fits*, which is what
/// keeps every unquantified slot off this walk entirely. Admission itself only ever fails with
/// [`UnifyFailure::Mismatch`]; [`UnifyFailure::Disagree`] is [`Collector::solve`]'s.
///
/// What reaches a variable is `carried` or a part of it, so a collector of `T`s takes a `T`.
pub fn admits_with<T: TypeHandle>(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    declared: Parametric,
    carried: T,
    variance: Variance,
    collector: &mut Collector<'_, T>,
) -> Result<(), UnifyFailure<T>> {
    admits(
        types,
        scratch,
        declared.raw(),
        carried.raw(),
        variance,
        collector,
    )
    .map_err(UnifyFailure::typed)
}

/// [`admits_with`] over raw handles, for the lattice's own walks.
pub(super) fn admits<T: TypeHandle>(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    declared: Handle,
    carried: Handle,
    variance: Variance,
    collector: &mut Collector<'_, T>,
) -> Result<(), UnifyFailure<Handle>> {
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
/// match first, then the members with nothing to solve, then the rest, each strictly more specific
/// one ([`member_at_least`]) before the members it is more specific than, and none whose greatest
/// instance `carried` does not fit.
///
/// Binding is the last resort. A member that admits without touching a variable gives a smaller
/// least instance: trying a free variable first would let it swallow a member that matches exactly,
/// so `(Elt | Number)` admitting `Number` would bind `Elt` larger than it needs to be. Among the
/// members that bind, the more specific binds less, so `((LIST OF Elt) | Key)` admitting `[1]`
/// binds `Elt` to `Number`, not `Key` to the list. Two members neither of which is more specific
/// that one argument can both admit [tie](self::tied_members), and a binder holding such a pair is
/// refused where it is declared; any order of the rest gives one answer.
pub(super) fn most_determined_first<'s>(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'s>,
    declared: &[Handle],
    carried: Handle,
) -> BumpVec<'s, Handle> {
    let mut order = BumpVec::with_capacity_in(declared.len(), scratch);
    if declared.contains(&carried) {
        order.push(carried);
    }
    for member in declared {
        if *member != carried && !types.contains_quantified(*member) {
            order.push(*member);
        }
    }
    let mut binding = BumpVec::with_capacity_in(declared.len(), scratch);
    binding.extend(
        (declared.iter().copied())
            .filter(|member| *member != carried && types.contains_quantified(*member)),
    );
    // A member whose greatest instance `carried` does not fit has no solve that admits it, though
    // its walk may admit: the solve checks each variable's bound after the choice. Dropped here, so
    // two members disjoint above are never both tried.
    if binding.len() > 1 {
        binding.retain(|member| {
            fits(
                types,
                scratch,
                carried,
                bound_above(types, scratch, *member),
            )
        });
    }
    let beats = |a: Handle, b: Handle| {
        member_at_least(types, scratch, a, b) && !member_at_least(types, scratch, b, a)
    };
    while !binding.is_empty() {
        let next = match binding.len() {
            1 => 0,
            _ => (0..binding.len())
                .find(|at| !binding.iter().any(|other| beats(*other, binding[*at])))
                .unwrap_or(0),
        };
        order.push(binding.remove(next));
    }
    order
}

/// Whether the union member `a` is at least as specific as its fellow member `b`: `b` admits `a`
/// with `a`'s variables rigid, as [`class_at_least`](super::ranking::class_at_least) compares two
/// shapes' slots. Recorded in the verdict table.
pub(super) fn member_at_least(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    a: Handle,
    b: Handle,
) -> bool {
    if let Some(known) = types.verdict(a.digest(), b.digest(), Relation::MemberAtLeast) {
        return known;
    }
    let mut collector = Collector::<Handle>::new(scratch, &[]);
    let answer = admits(types, scratch, b, a, Variance::Co, &mut collector).is_ok()
        && collector.solve(types).is_ok();
    types.record_verdict(a.digest(), b.digest(), Relation::MemberAtLeast, answer);
    answer
}

/// The first two members of a union at a covariant position of `declared` — a binder's parameter,
/// slot or representation, read under its own group — that **tie**: both name the group, and each
/// is at least as specific as the other ([`member_at_least`]), or neither is and some type lies
/// under both read from above. An argument both admit then has no member to prefer, so the solve
/// would turn on which member is stored first. A contravariant position takes its union whole, and
/// a nested binder's group is its own.
pub(super) fn tied_members(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    declared: Handle,
) -> Option<(Handle, Handle)> {
    let mut tied = None;
    visit_in(
        types,
        scratch,
        declared,
        Variance::Co,
        &mut |_, node, context| {
            if node.binds_quantifiers() {
                return Visit::Skip;
            }
            let TypeNode::Union { members } = *node else {
                return Visit::Descend;
            };
            if context.variance() == Variance::Contra {
                return Visit::Descend;
            }
            let mut binding = BumpVec::new_in(scratch);
            binding.extend(
                (members.iter().copied()).filter(|member| types.contains_quantified(*member)),
            );
            for (at, a) in binding.iter().enumerate() {
                if let Some(b) = binding[at + 1..]
                    .iter()
                    .find(|b| ties(types, scratch, *a, **b))
                {
                    tied = Some((*a, *b));
                    return Visit::Stop;
                }
            }
            Visit::Descend
        },
    );
    tied
}

/// Whether two members of one declared union tie: see [`tied_members`].
pub(super) fn ties(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    a: Handle,
    b: Handle,
) -> bool {
    let (at_least, at_most) = (
        member_at_least(types, scratch, a, b),
        member_at_least(types, scratch, b, a),
    );
    match (at_least, at_most) {
        (true, true) => true,
        (false, false) => {
            let (a, b) = (
                bound_above(types, scratch, a),
                bound_above(types, scratch, b),
            );
            meet_through_variables(types, scratch, a, b) != KType::NEVER.raw()
        }
        _ => false,
    }
}

/// The collecting [`Lockstep`] instance. It holds the caller's collector so a union's members can
/// be tried in turn — the declared side's at a covariant position, the carried side's at a
/// contravariant one — rolling back what a rejected one contributed and keeping the first that
/// admits.
struct Admits<'c, 's, T> {
    collector: &'c mut Collector<'s, T>,
}

type Admission = Result<(), UnifyFailure<Handle>>;

impl<T: TypeHandle> Admits<'_, '_, T> {
    /// Whether some member of `declared` admits the one carried type `one`, tried most determined
    /// first. A rejected member's contributions are rolled back; the admitting member's stay.
    fn admit_one(
        &mut self,
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
        declared: &[Handle],
        one: Handle,
        v: Variance,
        recurse: &mut dyn FnMut(&mut Self, Handle, Handle, Variance) -> Admission,
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

    /// Contravariant, the declared side lies under the carried one: every declared member must lie
    /// under some carried member, an exact match tried first and a rejected choice rolled back —
    /// except a declared variable, which lies under the carried side whole. Its one upper end is
    /// the union; a member chosen for it would narrow it to that member alone.
    fn under_carried(
        &mut self,
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
        declared: &[Handle],
        carried: &[Handle],
        recurse: &mut dyn FnMut(&mut Self, Handle, Handle, Variance) -> Admission,
    ) -> Admission {
        for one in declared {
            if matches!(types.node(*one), TypeNode::Quantified { .. }) {
                let whole = match carried {
                    [single] => *single,
                    _ => types.union_of(scratch, carried),
                };
                recurse(self, *one, whole, Variance::Contra)?;
                continue;
            }
            let exact = carried.iter().filter(|option| *option == one);
            let rest = carried.iter().filter(|option| *option != one);
            let admitted = exact.chain(rest).any(|option| {
                let mark = self.collector.mark();
                match recurse(self, *one, *option, Variance::Contra) {
                    Ok(()) => true,
                    Err(_) => {
                        self.collector.rollback(mark);
                        false
                    }
                }
            });
            if !admitted {
                return Err(UnifyFailure::Mismatch);
            }
        }
        Ok(())
    }
}

impl<T: TypeHandle> Lockstep for Admits<'_, '_, T> {
    type Out = Admission;

    fn enter(
        &mut self,
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
        declared: Handle,
        carried: Handle,
        v: Variance,
    ) -> Option<Admission> {
        if !types.contains_quantified(declared) {
            let admits = match v {
                Variance::Co => fits(types, scratch, carried, declared),
                Variance::Contra => fits(types, scratch, declared, carried),
            };
            // A verdict that holds over a variable holds at every binding; one that fails may not.
            if !admits && (types.contains_rigid(carried) || types.contains_rigid(declared)) {
                self.collector.bound_read = true;
            }
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
        declared: Handle,
        carried: Handle,
        v: Variance,
    ) -> Admission {
        // A carried rigid variable fills, at a covariant position, what its bound fills, and at a
        // contravariant one what its lower end fills: below one lie itself and what lies under its
        // lower end.
        let Some(ends) = types.node(carried).rigid_interval() else {
            return Err(UnifyFailure::Mismatch);
        };
        self.collector.bound_read = true;
        match v {
            Variance::Co => lockstep(types, scratch, declared, ends.upper.raw(), v, self),
            Variance::Contra if ends.lower != KType::NEVER => {
                lockstep(types, scratch, declared, ends.lower.raw(), v, self)
            }
            Variance::Contra => Err(UnifyFailure::Mismatch),
        }
    }

    fn set_wise(
        &mut self,
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
        declared: &[Handle],
        carried: &[Handle],
        v: Variance,
        recurse: &mut dyn FnMut(&mut Self, Handle, Handle, Variance) -> Admission,
    ) -> Admission {
        // A non-union side arrives as a one-element slice, so this covers a union on either side
        // and on both. A rejected choice rolls back, so it leaves no contribution behind; the
        // contributions of every member admitted accumulate in the one collector.
        if v == Variance::Contra {
            return self.under_carried(types, scratch, declared, carried, recurse);
        }
        // Covariant, the carried side lies under the declared one: every carried member must be
        // admitted by some declared member.
        for one in carried {
            if self.admit_one(types, scratch, declared, *one, v, recurse) {
                continue;
            }
            // A carried variable whose bound spans several declared members is admitted through
            // the bound's members, each by some declared member.
            let bound = types.node(*one).rigid_bound();
            self.collector.bound_read |= bound.is_some();
            let Some(bound) = bound.filter(|_| v == Variance::Co) else {
                return Err(UnifyFailure::Mismatch);
            };
            let TypeNode::Union { members } = types.node(bound.raw()) else {
                return Err(UnifyFailure::Mismatch);
            };
            let mark = self.collector.mark();
            for member in members.iter() {
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
