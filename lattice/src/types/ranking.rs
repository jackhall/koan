//! Class-by-class admission and ranking: how an expression shape's priority classes order the
//! solving of its group and the comparison of two candidates under one bucket key.
//!
//! A shape's slots sit in dense priority classes ([`dense_classes`](super::shape::dense_classes));
//! written order puts each slot in a class of its own. [`admit_by_class`] solves a quantified group
//! one class at a time: the first class whose slots mention a variable solves it, jointly over that
//! class's slots, and every later class admits its arguments against that solution, which is
//! [pinned](super::unify::Collector::pin) in the class's collector. So `FOR ALL #[Elt] #(PAIR x
//! :Elt WITH y :Elt)` fixes `Elt` from `x` and then refuses a `y` that does not lie under it.
//! [`admits_shape`](super::sig_relations) runs the same classes with a candidate shape's slot types
//! as the arguments.
//!
//! [`class_at_least`] is the per-class specificity verdict dispatch ranks by, recorded in the
//! tiers' verdict tables under [`Relation::ClassAtLeast`]. It and *fits*' shape clause differ from
//! admission at a call in how an earlier class's outcome reads later: over static types, each
//! stands for every type a call carries under it, so a class that admitted fixes each variable it
//! solved to an **interval** of bindings, read later as a lexical variable between its ends
//! (README § Priority classes); a class that did not admit leaves each as its
//! bound. A solution already holding a rigid variable of the other candidate stands for an unknown
//! already and is kept. [`select_by_class`]
//! runs the elimination over a candidate list: at each class, every candidate another strictly
//! beats there drops out, and the rest go on.
//!
//! [`judge_by_class`] walks the same classes over arguments known only by their static types, each
//! an [`Interval`], and gives a candidate a [`Verdict`] the load can act on: *never*, *always* or
//! *maybe*, beside each variable's interval. [`solving_slots`] names the slots a solve reads: those
//! naming a variable their own class first mentions. A later class only admits against that
//! solution, so dispatch hands the solve an argument's static contribution at a solving slot alone.

use crate::bump::{BumpAllocator, BumpVec};

use super::chain::TypeChain;
use super::handle::{Handle, Parametric, TypeHandle};
use super::lattice::meet_through_variables;
use super::node::{TypeNode, Variable};
use super::order::fits;
use super::shape::Shape;
use super::stored::Linked;
use super::substitute::{Side, bound_above, read_through};
use super::tiers::Habitat;
use super::unify::{Collector, Interval, admits, intervals_in};
use super::verdicts::Relation;
use super::walk::Variance;

/// The class-by-class walk's state over one declared shape: which class first mentions each
/// variable, and what each variable an earlier class fixed reads as.
struct ClassWalk<'s, 'run> {
    declared: Shape<'run>,
    /// The group's bounds, each entered once: a group's bounds are handles.
    bounds: &'s [Linked<'run>],
    slots: BumpVec<'s, Linked<'run>>,
    /// Per variable, the lowest class whose slots mention it; `None` for one only the return does.
    first: BumpVec<'s, Option<u16>>,
    fixed: BumpVec<'s, Option<Linked<'run>>>,
}

impl<'s, 'run> ClassWalk<'s, 'run> {
    fn new(
        types: &TypeChain<'run, impl Habitat<'run>>,
        scratch: BumpAllocator<'s>,
        declared: Shape<'run>,
    ) -> Self {
        let mut slots = BumpVec::new_in(scratch);
        slots.extend(declared.slots());
        let arity = declared.quantifiers.len();
        let bounds = scratch.alloc_slice_fill_iter(
            (0..arity).map(|index| types.enter(declared.bound(index).raw())),
        );
        let mut first = BumpVec::with_capacity_in(arity, scratch);
        first.extend((0..arity).map(|variable| {
            slots
                .iter()
                .enumerate()
                .filter(|(_, slot)| types.references_quantifier(scratch, **slot, variable))
                .map(|(index, _)| declared.classes.get(index))
                .min()
        }));
        let mut fixed = BumpVec::with_capacity_in(arity, scratch);
        fixed.resize(arity, None);
        ClassWalk {
            declared,
            bounds,
            slots,
            first,
            fixed,
        }
    }

    /// A collector with every fixed variable pinned to what it reads as.
    fn collector(
        &self,
        types: &TypeChain<'run, impl Habitat<'run>>,
        scratch: BumpAllocator<'s>,
    ) -> Collector<'s, 'run, Linked<'run>> {
        let mut collector = Collector::over(scratch, self.bounds.iter().copied());
        for (index, fixed) in self.fixed.iter().enumerate() {
            if let Some(to) = fixed {
                collector.pin_node(types, index, self.bounds[index], to);
            }
        }
        collector
    }

    /// Whether every slot in `class` that `reads` picks admits its argument, jointly, against what
    /// the earlier classes fixed: the class's solution, or `None`, beside the collector it ran in.
    fn admit_class(
        &self,
        types: &TypeChain<'run, impl Habitat<'run>>,
        scratch: BumpAllocator<'s>,
        arguments: &[Linked<'run>],
        class: u16,
        reads: impl Fn(usize) -> bool,
    ) -> ClassSolve<'s, 'run> {
        let mut collector = self.collector(types, scratch);
        for (index, slot) in self.slots.iter().enumerate() {
            if self.declared.classes.get(index) == class
                && reads(index)
                && admits(
                    types,
                    scratch,
                    slot,
                    arguments[index],
                    Variance::Co,
                    &mut collector,
                )
                .is_err()
            {
                return ClassSolve {
                    solution: None,
                    collector,
                };
            }
        }
        ClassSolve {
            solution: collector.solve(types).ok(),
            collector,
        }
    }

    /// Fix each variable `class` first mentions to `read(variable)`.
    fn fix(&mut self, class: u16, mut read: impl FnMut(usize) -> Linked<'run>) {
        for variable in 0..self.fixed.len() {
            if self.first[variable] == Some(class) {
                self.fixed[variable] = Some(read(variable));
            }
        }
    }

    /// Fix each variable `class` first mentions to what a later class reads it as where the
    /// arguments are static types: the interval of bindings a call over types under them can make,
    /// read by [`read_later`].
    fn fix_to_intervals(
        &mut self,
        types: &TypeChain<'run, impl Habitat<'run>>,
        scratch: BumpAllocator<'s>,
        class: u16,
        solution: &[Linked<'run>],
    ) {
        let declared = self.declared;
        let mut positions = BumpVec::new_in(scratch);
        positions.extend(
            (0..self.slots.len())
                .filter(|slot| declared.classes.get(*slot) == class)
                .map(|slot| self.slots[slot]),
        );
        let reach = intervals_in(types, scratch, &positions, self.bounds, solution, false);
        self.fix(class, |variable| {
            read_later(
                types,
                scratch,
                declared,
                variable,
                reach[variable],
                solution[variable],
            )
        });
    }

    /// Admit every class in order, fixing each class's variables to its solution, or `None` at the
    /// first class that does not admit.
    fn admit_all(
        &mut self,
        types: &TypeChain<'run, impl Habitat<'run>>,
        scratch: BumpAllocator<'s>,
        arguments: &[Linked<'run>],
    ) -> Option<()> {
        for class in self.declared.class_indices() {
            let solution = self
                .admit_class(types, scratch, arguments, class, |_| true)
                .solution?;
            self.fix(class, |variable| solution[variable]);
        }
        Some(())
    }
}

/// Solve `declared`'s group against one argument type per slot, class by class — what a keyworded
/// call admits by. The solution in the shape's group order, a variable no slot mentions
/// reading as its bound; `None` when some class does not admit, when `declared` is not a shape,
/// or when the arguments are not one per slot.
pub(super) fn admit_by_class<'s, 'run>(
    types: &TypeChain<'run, impl Habitat<'run>>,
    scratch: BumpAllocator<'s>,
    declared: Linked<'run>,
    arguments: &[Linked<'run>],
) -> Option<&'s [Linked<'run>]> {
    let declared = Shape::of(declared)?;
    let mut walk = ClassWalk::new(types, scratch, declared);
    if walk.slots.len() != arguments.len() {
        return None;
    }
    walk.admit_all(types, scratch, arguments)?;
    Some(scratch.alloc_slice_fill_iter(
        (0..walk.fixed.len()).map(|index| walk.fixed[index].unwrap_or(walk.bounds[index])),
    ))
}

/// Which slots of `declared` its group's solve reads: slot `i` names a variable whose first class —
/// the lowest class whose slots name it — is `i`'s own. Empty for anything that is not a shape.
pub(super) fn solving_slots<'s, 'run>(
    types: &TypeChain<'run, impl Habitat<'run>>,
    scratch: BumpAllocator<'s>,
    declared: Linked<'run>,
) -> &'s [bool] {
    let Some(ranked) = Shape::of(declared) else {
        return &[];
    };
    let walk = ClassWalk::new(types, scratch, ranked);
    scratch.alloc_slice_fill_iter((0..walk.slots.len()).map(|slot| {
        let class = ranked.classes.get(slot);
        (0..walk.first.len()).any(|variable| {
            walk.first[variable] == Some(class)
                && types.references_quantifier(scratch, walk.slots[slot], variable)
        })
    }))
}

/// One class's static solve beside the collector it ran in, which answers whether a run
/// reproduces it ([`Collector::reproducible`]).
struct ClassSolve<'s, 'run> {
    solution: Option<BumpVec<'s, Linked<'run>>>,
    collector: Collector<'s, 'run, Linked<'run>>,
}

/// What the load knows of a candidate against arguments of known static types.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Some slot, read at its greatest instance, meets its argument's upper end at `Never` or does
    /// not lie above its lower end: no call admits.
    Never,
    /// Every call admits, whatever it carries within the static types.
    Always,
    /// A call may admit.
    Maybe,
}

/// A candidate's verdict, beside each variable of its group's interval where every class's static
/// solve succeeded.
#[derive(Clone, Copy, Debug)]
pub struct Judged<'s> {
    pub verdict: Verdict,
    pub intervals: Option<&'s [Interval<Parametric>]>,
}

/// Judge `declared` against one static type per slot, class by class: see README § Priority
/// classes. No argument's upper end is `Never`.
///
/// A class is *never* where some slot, read at its greatest instance, meets its argument's upper
/// end at `Never` or does not lie above its lower end, read below its rigid variables: the carried
/// type lies above that lower end, so the slot admits no call. It admits every call when each slot
/// does: a slot naming its own class's variables where the class is exact — those arguments exact,
/// the earlier variables they name pinned, and the solve over them reproducible
/// ([`Collector::reproducible`]) — so the static solve is the call's; a slot whose least instance,
/// earlier variables read at theirs, lies above its argument's upper end; or a bare variable of
/// the class that no other slot of the class names, whose one contribution lies under its bound.
/// An exact class whose static solve fails over the slots naming its own variables is *never*
/// too: that solve is the call's.
pub(super) fn judge_by_class<'s, 'run>(
    types: &TypeChain<'run, impl Habitat<'run>>,
    scratch: BumpAllocator<'s>,
    declared: Linked<'run>,
    arguments: &[Interval<Linked<'run>>],
) -> Judged<'s> {
    let unjudged = Judged {
        verdict: Verdict::Maybe,
        intervals: None,
    };
    let Some(ranked) = Shape::of(declared) else {
        return unjudged;
    };
    let mut walk = ClassWalk::new(types, scratch, ranked);
    if walk.slots.len() != arguments.len() {
        return unjudged;
    }
    let bounds = walk.bounds;
    let arity = ranked.quantifiers.len();
    let mut known: BumpVec<'s, Option<Interval<Linked<'run>>>> =
        BumpVec::with_capacity_in(arity, scratch);
    known.resize(arity, None);
    let mut uppers = BumpVec::with_capacity_in(arguments.len(), scratch);
    uppers.extend(arguments.iter().map(|argument| argument.upper));
    let (mut always, mut solved) = (true, true);
    for class in ranked.class_indices() {
        let own = |variable: usize| walk.first[variable] == Some(class);
        let in_class = |slot: usize| ranked.classes.get(slot) == class;
        let names = |slot: usize, variable: usize| {
            types.references_quantifier(scratch, walk.slots[slot], variable)
        };
        let pointed = |variable: usize| known[variable].is_some_and(Interval::is_exact);
        let earlier = |variable: usize, bound: Linked<'run>| {
            known[variable].unwrap_or(Interval {
                lower: types.never(),
                upper: bound,
            })
        };
        for slot in (0..walk.slots.len()).filter(|slot| in_class(*slot)) {
            // An earlier interval's end may name a lexical variable; the end above every instance
            // reads it at its ends.
            let greatest = bound_above(
                types,
                scratch,
                read_through(
                    types,
                    scratch,
                    walk.slots[slot],
                    Side::Above,
                    &mut |_, node| match Variable::of(node)? {
                        Variable::Quantified { index, bound } => Some(earlier(index, bound)),
                        variable => Some(variable.interval_in(types)),
                    },
                ),
            );
            let upper = bound_above(types, scratch, arguments[slot].upper);
            if meet_through_variables(types, scratch, greatest, upper) == Handle::NEVER
                || lower_end_outside(types, scratch, arguments[slot].lower, greatest)
            {
                return Judged {
                    verdict: Verdict::Never,
                    intervals: None,
                };
            }
        }
        let solves = |slot: usize| in_class(slot) && (0..arity).any(|v| own(v) && names(slot, v));
        let exact_arguments = (0..walk.slots.len())
            .filter(|slot| solves(*slot))
            .all(|slot| {
                arguments[slot].is_exact()
                    && (0..arity).all(|v| own(v) || !names(slot, v) || pointed(v))
            });
        let full = walk
            .admit_class(types, scratch, &uppers, class, |_| true)
            .solution;
        // Over exact arguments, the static solve is the call's own where a run binding each
        // lexical variable reproduces it, as a closed one always does. It is taken where a slot
        // or argument names one, or where the full solve failed and it decides *never*.
        let rigid = (0..walk.slots.len())
            .filter(|slot| solves(*slot))
            .any(|slot| {
                walk.slots[slot].contains_rigid() || arguments[slot].upper.contains_rigid()
            });
        let own_solve = (exact_arguments && (rigid || full.is_none()))
            .then(|| walk.admit_class(types, scratch, &uppers, class, solves));
        let exact_class = exact_arguments
            && own_solve
                .as_ref()
                .is_none_or(|own| own.collector.reproducible(types));
        if exact_class && own_solve.is_some_and(|own| own.solution.is_none()) {
            return Judged {
                verdict: Verdict::Never,
                intervals: None,
            };
        }
        for slot in (0..walk.slots.len()).filter(|slot| in_class(*slot)) {
            if !always {
                break;
            }
            if exact_class && (0..arity).any(|v| own(v) && names(slot, v)) {
                continue;
            }
            let least = read_through(
                types,
                scratch,
                walk.slots[slot],
                Side::Below,
                &mut |_, node| match Variable::of(node)? {
                    Variable::Quantified { index, bound } if !own(index) => {
                        Some(earlier(index, bound))
                    }
                    _ => None,
                },
            );
            always = if !least.contains_quantified() {
                fits(types, scratch, arguments[slot].upper, least)
            } else if let TypeNode::Quantified { index, bound } = least.view()
                && own(index)
                && !(0..walk.slots.len())
                    .any(|other| other != slot && in_class(other) && names(other, index))
            {
                fits(types, scratch, arguments[slot].upper, bound)
            } else {
                false
            };
        }
        match full {
            Some(solution) => {
                let mut positions = BumpVec::new_in(scratch);
                positions.extend(
                    (0..walk.slots.len())
                        .filter(|slot| in_class(*slot))
                        .map(|slot| walk.slots[slot]),
                );
                let reached =
                    intervals_in(types, scratch, &positions, bounds, &solution, exact_class);
                for variable in (0..arity).filter(|variable| own(*variable)) {
                    known[variable] = Some(reached[variable]);
                }
                walk.fix(class, |variable| solution[variable]);
            }
            None => {
                always = false;
                solved = false;
                walk.fix(class, |variable| bounds[variable]);
            }
        }
    }
    Judged {
        verdict: if always {
            Verdict::Always
        } else {
            Verdict::Maybe
        },
        // The intervals are the static solve's, over parametric arguments.
        intervals: solved.then(|| {
            &*scratch.alloc_slice_fill_iter((0..arity).map(|variable| {
                known[variable]
                    .unwrap_or(Interval::point(bounds[variable]))
                    .typed::<Parametric>()
            }))
        }),
    }
}

/// Whether `declared` admits `candidate`'s slot types class by class — `declared`'s variables
/// solved, `candidate`'s rigid — and `declared`'s return then lies under `candidate`'s. A later
/// class reads an earlier variable at its reach interval
/// (README § Priority classes). Both must be shapes under one key and one ranking;
/// the caller checks that.
pub(super) fn admits_by_class<'run>(
    types: &TypeChain<'run, impl Habitat<'run>>,
    scratch: BumpAllocator<'_>,
    declared: Shape<'run>,
    candidate: Shape<'run>,
) -> bool {
    let mut walk = ClassWalk::new(types, scratch, declared);
    let mut arguments = BumpVec::new_in(scratch);
    arguments.extend(candidate.slots());
    for class in declared.class_indices() {
        let Some(solution) = walk
            .admit_class(types, scratch, &arguments, class, |_| true)
            .solution
        else {
            return false;
        };
        walk.fix_to_intervals(types, scratch, class, &solution);
    }
    let mut collector = walk.collector(types, scratch);
    admits(
        types,
        scratch,
        declared.ret,
        candidate.ret,
        Variance::Contra,
        &mut collector,
    )
    .is_ok()
        && collector.solve(types).is_ok()
}

/// Whether an argument whose static lower end is `lower`, read below its variables, lies outside
/// `slot`: every type the run can carry there lies outside it too, so the candidate never admits.
pub(super) fn lower_end_outside<'run>(
    types: &TypeChain<'run, impl Habitat<'run>>,
    scratch: BumpAllocator<'_>,
    lower: Linked<'run>,
    slot: Linked<'run>,
) -> bool {
    let lower = read_through(types, scratch, lower, Side::Below, &mut |_, node| {
        Some(Variable::of(node)?.interval_in(types))
    });
    !fits(types, scratch, lower, slot)
}

/// Whether `a` strictly outranks `b` at `class`: at least as specific there, and `b` not at least
/// as specific as `a`. A survivor `a` outranks at a class eliminates `b` there.
pub(super) fn outranks<'run>(
    types: &TypeChain<'run, impl Habitat<'run>>,
    scratch: BumpAllocator<'_>,
    a: Linked<'run>,
    b: Linked<'run>,
    class: u16,
) -> bool {
    class_at_least(types, scratch, a, b, class) && !class_at_least(types, scratch, b, a, class)
}

/// Whether `a` is at least as specific as `b` at `class`: `b`'s slots in that class admit `a`'s
/// own, jointly, with each variable an earlier class admitted read at its reach interval
/// ([`read_later`]) and each one an earlier class refused read as its bound. `a`'s variables are
/// rigid.
///
/// A function of the two shape types and the class alone, so it is recorded in the verdict table;
/// one pass computes every class's verdict for the pair and records them all.
pub(super) fn class_at_least<'run>(
    types: &TypeChain<'run, impl Habitat<'run>>,
    scratch: BumpAllocator<'_>,
    a: Linked<'run>,
    b: Linked<'run>,
    class: u16,
) -> bool {
    let relation = Relation::ClassAtLeast(class);
    if let Some(known) = types.verdict(a.handle(), b.handle(), relation) {
        return known;
    }
    let (Some(ranked_a), Some(ranked_b)) = (Shape::of(a), Shape::of(b)) else {
        return false;
    };
    let mut walk = ClassWalk::new(types, scratch, ranked_b);
    let bounds = walk.bounds;
    let mut arguments = BumpVec::new_in(scratch);
    arguments.extend(ranked_a.slots());
    let mut answer = false;
    for each in ranked_b.class_indices() {
        let solution = walk
            .admit_class(types, scratch, &arguments, each, |_| true)
            .solution;
        types.record_verdict(
            a.handle(),
            b.handle(),
            Relation::ClassAtLeast(each),
            solution.is_some(),
        );
        if each == class {
            answer = solution.is_some();
        }
        match solution {
            Some(solution) => walk.fix_to_intervals(types, scratch, each, &solution),
            None => walk.fix(each, |variable| bounds[variable]),
        }
    }
    answer
}

/// The level of a class walk's stand-in, which no lexical variable the elaborator mints shares:
/// the highest level a stored node can hold.
pub(super) const STAND_IN_LEVEL: usize = u32::MAX as usize;

/// What a variable an earlier class solved to `solution` reads as in a later one, over static
/// types: `solution` itself where the reach interval converged, `solution` names a rigid variable
/// of the other side — which stands for one unknown already — or it holds an opaque carrier, an
/// atom no lexical variable's ends may hold; and otherwise a lexical variable between the
/// interval's ends.
fn read_later<'run>(
    types: &TypeChain<'run, impl Habitat<'run>>,
    scratch: BumpAllocator<'_>,
    shape: Shape<'run>,
    variable: usize,
    reach: Interval<Linked<'run>>,
    solution: Linked<'run>,
) -> Linked<'run> {
    if reach.is_exact() || solution.contains_rigid() || solution.contains_carrier() {
        return solution;
    }
    // Neither end holds a rigid variable: an end is the solution, a bound or `Never`, and the
    // solution holds none here.
    types.lexical_node(
        scratch,
        STAND_IN_LEVEL,
        shape.quantifiers[variable],
        reach.lower,
        reach.upper,
    )
}

/// The survivors of the class-by-class elimination over `shapes` — candidates under one key and
/// one ranking — as indices into `shapes`. At each class, a candidate some other survivor strictly
/// beats there drops out; a class that orders neither of two leaves both to the next.
pub(super) fn select_by_class<'s, 'run>(
    types: &TypeChain<'run, impl Habitat<'run>>,
    scratch: BumpAllocator<'s>,
    shapes: &[Linked<'run>],
) -> BumpVec<'s, usize> {
    let mut survivors = BumpVec::with_capacity_in(shapes.len(), scratch);
    survivors.extend(0..shapes.len());
    let first = shapes.first().and_then(|shape| Shape::of(shape));
    for class in first.into_iter().flat_map(Shape::class_indices) {
        if survivors.len() <= 1 {
            break;
        }
        let beats = |x: usize, y: usize| outranks(types, scratch, shapes[x], shapes[y], class);
        let mut kept = BumpVec::with_capacity_in(survivors.len(), scratch);
        kept.extend(
            survivors
                .iter()
                .copied()
                .filter(|s| !survivors.iter().any(|t| *t != *s && beats(*t, *s))),
        );
        survivors = kept;
    }
    survivors
}
