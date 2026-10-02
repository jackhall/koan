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
//! registry's verdict table under [`Relation::ClassAtLeast`]. It and *fits*' shape clause differ from
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
//! *maybe*, beside each variable's interval.

use crate::memory::{BumpAllocator, BumpVec};
use crate::symbols::TypeSymbol;

use super::handle::{Handle, KType, Parametric, TypeHandle};
use super::lattice::meet_through_variables;
use super::node::TypeNode;
use super::order::fits;
use super::registry::{Relation, TypeRegistry};
use super::shape::{DispatchTokenElement, class_of};
use super::substitute::{Side, Variable, bound_above, read_through};
use super::unify::{Collector, Interval, admits, intervals};
use super::walk::Variance;

/// An expression shape's parts, read once off its node for a class-by-class walk.
#[derive(Clone, Copy)]
pub(super) struct Ranked<'run> {
    pub(super) quantifiers: &'run [TypeSymbol],
    pub(super) bounds: &'run [KType],
    pub(super) elements: &'run [DispatchTokenElement],
    pub(super) classes: &'run [u8],
    pub(super) ret: Handle,
}

impl<'run> Ranked<'run> {
    /// `kt`'s shape parts, or `None` for anything that is not a shape.
    pub(super) fn of(types: &TypeRegistry<'run>, kt: Handle) -> Option<Self> {
        match types.node(kt) {
            TypeNode::ExpressionShape {
                quantifiers,
                bounds,
                elements,
                classes,
                ret,
            } => Some(Ranked {
                quantifiers,
                bounds,
                elements: elements.raw(),
                classes,
                ret,
            }),
            _ => None,
        }
    }

    /// The slot types, in slot order.
    pub(super) fn slots(self) -> impl Iterator<Item = Handle> + use<'run> {
        self.elements.iter().filter_map(|element| match element {
            DispatchTokenElement::Slot(kt) => Some(*kt),
            DispatchTokenElement::Keyword(_) => None,
        })
    }

    /// How many classes the ranking has: one per slot in written order, and otherwise one past
    /// the highest.
    pub(super) fn class_count(self) -> usize {
        match self.classes.iter().max() {
            Some(highest) => usize::from(*highest) + 1,
            None => self.slots().count(),
        }
    }

    /// The bound of the `index`-th variable of the shape's own group.
    fn bound(self, index: usize) -> KType {
        self.bounds.get(index).copied().unwrap_or(KType::ANY)
    }
}

/// The class-by-class walk's state over one declared shape: which class first mentions each
/// variable, and what each variable an earlier class fixed reads as.
struct ClassWalk<'s, 'run> {
    declared: Ranked<'run>,
    slots: BumpVec<'s, Handle>,
    /// Per variable, the lowest class whose slots mention it; `None` for one only the return does.
    first: BumpVec<'s, Option<u8>>,
    fixed: BumpVec<'s, Option<Handle>>,
}

impl<'s, 'run> ClassWalk<'s, 'run> {
    fn new(types: &TypeRegistry<'run>, scratch: BumpAllocator<'s>, declared: Ranked<'run>) -> Self {
        let mut slots = BumpVec::new_in(scratch);
        slots.extend(declared.slots());
        let arity = declared.quantifiers.len();
        let mut first = BumpVec::with_capacity_in(arity, scratch);
        first.extend((0..arity).map(|variable| {
            slots
                .iter()
                .enumerate()
                .filter(|(_, slot)| types.references_quantifier(scratch, **slot, variable))
                .map(|(index, _)| class_of(declared.classes, index))
                .min()
        }));
        let mut fixed = BumpVec::with_capacity_in(arity, scratch);
        fixed.resize(arity, None);
        ClassWalk {
            declared,
            slots,
            first,
            fixed,
        }
    }

    /// A collector with every fixed variable pinned to what it reads as.
    fn collector(&self, scratch: BumpAllocator<'s>) -> Collector<'s, Handle> {
        let mut collector = Collector::new(scratch, self.declared.bounds);
        for (index, fixed) in self.fixed.iter().enumerate() {
            if let Some(to) = fixed {
                collector.pin(index, self.declared.bound(index), *to);
            }
        }
        collector
    }

    /// Whether every slot in `class` admits its argument, jointly, against what the earlier
    /// classes fixed: the class's solution, or `None`.
    fn admit_class(
        &self,
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'s>,
        arguments: &[Handle],
        class: u8,
    ) -> Option<BumpVec<'s, Handle>> {
        let mut collector = self.collector(scratch);
        for (index, slot) in self.slots.iter().enumerate() {
            if class_of(self.declared.classes, index) == class
                && admits(
                    types,
                    scratch,
                    *slot,
                    arguments[index],
                    Variance::Co,
                    &mut collector,
                )
                .is_err()
            {
                return None;
            }
        }
        collector.solve(types).ok()
    }

    /// Fix each variable `class` first mentions to `read(variable)`.
    fn fix(&mut self, class: u8, mut read: impl FnMut(usize) -> Handle) {
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
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'s>,
        class: u8,
        solution: &[Handle],
    ) {
        let declared = self.declared;
        let mut positions = BumpVec::new_in(scratch);
        positions.extend(
            (0..self.slots.len())
                .filter(|slot| class_of(declared.classes, *slot) == class)
                .map(|slot| self.slots[slot]),
        );
        let reach = intervals(types, scratch, &positions, declared.bounds, solution, false);
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
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'s>,
        arguments: &[Handle],
    ) -> Option<()> {
        for class in 0..self.declared.class_count() {
            let class = u8::try_from(class).expect("a shape has fewer than 256 classes");
            let solution = self.admit_class(types, scratch, arguments, class)?;
            self.fix(class, |variable| solution[variable]);
        }
        Some(())
    }
}

/// Solve `declared`'s group against one argument type per slot, class by class — what a keyworded
/// call admits by. The solution in the shape's group order, a variable no slot mentions
/// reading as its bound; `None` when some class does not admit, when `declared` is not a shape,
/// or when the arguments are not one per slot.
pub(super) fn admit_by_class<'s>(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'s>,
    declared: Handle,
    arguments: &[Handle],
) -> Option<&'s [Handle]> {
    let declared = Ranked::of(types, declared)?;
    let mut walk = ClassWalk::new(types, scratch, declared);
    if walk.slots.len() != arguments.len() {
        return None;
    }
    walk.admit_all(types, scratch, arguments)?;
    Some(scratch.alloc_slice_fill_iter(
        (0..walk.fixed.len()).map(|index| walk.fixed[index].unwrap_or(declared.bound(index).raw())),
    ))
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
/// does: a slot naming its own class's variables where the class is exact — those arguments exact
/// and free of rigid variables, the earlier variables they name pinned — so the static solve is
/// the call's; a slot whose least instance, earlier variables read at theirs, lies above its
/// argument's upper end; or a bare variable of the class that no other slot of the class names,
/// whose one contribution lies under its bound.
pub(super) fn judge_by_class<'s>(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'s>,
    declared: Handle,
    arguments: &[Interval<Handle>],
) -> Judged<'s> {
    let unjudged = Judged {
        verdict: Verdict::Maybe,
        intervals: None,
    };
    let Some(ranked) = Ranked::of(types, declared) else {
        return unjudged;
    };
    let mut walk = ClassWalk::new(types, scratch, ranked);
    if walk.slots.len() != arguments.len() {
        return unjudged;
    }
    let arity = ranked.quantifiers.len();
    let mut known: BumpVec<'s, Option<Interval<Handle>>> =
        BumpVec::with_capacity_in(arity, scratch);
    known.resize(arity, None);
    let mut uppers = BumpVec::with_capacity_in(arguments.len(), scratch);
    uppers.extend(arguments.iter().map(|argument| argument.upper));
    let (mut always, mut solved) = (true, true);
    for class in 0..ranked.class_count() {
        let class = u8::try_from(class).expect("a shape has fewer than 256 classes");
        let own = |variable: usize| walk.first[variable] == Some(class);
        let in_class = |slot: usize| class_of(ranked.classes, slot) == class;
        let names = |slot: usize, variable: usize| {
            types.references_quantifier(scratch, walk.slots[slot], variable)
        };
        let pointed = |variable: usize| known[variable].is_some_and(Interval::is_exact);
        let earlier =
            |variable: usize, bound: KType| known[variable].unwrap_or(Interval::within(bound));
        for slot in (0..walk.slots.len()).filter(|slot| in_class(*slot)) {
            let greatest = read_through(
                types,
                scratch,
                walk.slots[slot],
                Side::Above,
                &mut |variable| match variable {
                    Variable::Quantified { index, bound } => Some(earlier(index, bound)),
                    _ => Some(variable.interval().raw()),
                },
            );
            let upper = bound_above(types, scratch, arguments[slot].upper);
            let lower = read_through(
                types,
                scratch,
                arguments[slot].lower,
                Side::Below,
                &mut |variable| Some(variable.interval().raw()),
            );
            if meet_through_variables(types, scratch, greatest, upper) == Handle::NEVER
                || !fits(types, scratch, lower, greatest)
            {
                return Judged {
                    verdict: Verdict::Never,
                    intervals: None,
                };
            }
        }
        let exact_class = (0..walk.slots.len())
            .filter(|slot| in_class(*slot) && (0..arity).any(|v| own(v) && names(*slot, v)))
            .all(|slot| {
                // A rigid variable the static type holds is read through its bound, where the
                // call reads the type the run binds it to: that solve is not the call's.
                arguments[slot].is_exact()
                    && !types.contains_rigid(arguments[slot].upper)
                    && (0..arity).all(|v| own(v) || !names(slot, v) || pointed(v))
            });
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
                &mut |variable| match variable {
                    Variable::Quantified { index, bound } if !own(index) => {
                        Some(earlier(index, bound))
                    }
                    _ => None,
                },
            );
            always = if !types.contains_quantified(least) {
                fits(types, scratch, arguments[slot].upper, least)
            } else if let TypeNode::Quantified { index, bound } = types.node(least)
                && own(index)
                && !(0..walk.slots.len())
                    .any(|other| other != slot && in_class(other) && names(other, index))
            {
                fits(types, scratch, arguments[slot].upper, bound.raw())
            } else {
                false
            };
        }
        match walk.admit_class(types, scratch, &uppers, class) {
            Some(solution) => {
                let mut positions = BumpVec::new_in(scratch);
                positions.extend(
                    (0..walk.slots.len())
                        .filter(|slot| in_class(*slot))
                        .map(|slot| walk.slots[slot]),
                );
                let reached = intervals(
                    types,
                    scratch,
                    &positions,
                    ranked.bounds,
                    &solution,
                    exact_class,
                );
                for variable in (0..arity).filter(|variable| own(*variable)) {
                    known[variable] = Some(reached[variable]);
                }
                walk.fix(class, |variable| solution[variable]);
            }
            None => {
                always = false;
                solved = false;
                walk.fix(class, |variable| ranked.bound(variable).raw());
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
                    .unwrap_or(Interval::point(ranked.bound(variable).raw()))
                    .typed()
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
    types: &TypeRegistry<'run>,
    scratch: BumpAllocator<'_>,
    declared: Ranked<'run>,
    candidate: Ranked<'run>,
) -> bool {
    let mut walk = ClassWalk::new(types, scratch, declared);
    let mut arguments = BumpVec::new_in(scratch);
    arguments.extend(candidate.slots());
    for class in 0..declared.class_count() {
        let class = u8::try_from(class).expect("a shape has fewer than 256 classes");
        let Some(solution) = walk.admit_class(types, scratch, &arguments, class) else {
            return false;
        };
        walk.fix_to_intervals(types, scratch, class, &solution);
    }
    let mut collector = walk.collector(scratch);
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

/// Whether `a` is at least as specific as `b` at `class`: `b`'s slots in that class admit `a`'s
/// own, jointly, with each variable an earlier class admitted read at its reach interval
/// ([`read_later`]) and each one an earlier class refused read as its bound. `a`'s variables are
/// rigid.
///
/// A function of the two shape types and the class alone, so it is recorded in the verdict table;
/// one pass computes every class's verdict for the pair and records them all.
pub(super) fn class_at_least(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    a: Handle,
    b: Handle,
    class: u8,
) -> bool {
    let relation = Relation::ClassAtLeast(class);
    if let Some(known) = types.verdict(a.digest(), b.digest(), relation) {
        return known;
    }
    let (Some(ranked_a), Some(ranked_b)) = (Ranked::of(types, a), Ranked::of(types, b)) else {
        return false;
    };
    let mut walk = ClassWalk::new(types, scratch, ranked_b);
    let mut arguments = BumpVec::new_in(scratch);
    arguments.extend(ranked_a.slots());
    let mut answer = false;
    for each in 0..ranked_b.class_count() {
        let each = u8::try_from(each).expect("a shape has fewer than 256 classes");
        let solution = walk.admit_class(types, scratch, &arguments, each);
        types.record_verdict(
            a.digest(),
            b.digest(),
            Relation::ClassAtLeast(each),
            solution.is_some(),
        );
        if each == class {
            answer = solution.is_some();
        }
        match solution {
            Some(solution) => walk.fix_to_intervals(types, scratch, each, &solution),
            None => walk.fix(each, |variable| ranked_b.bound(variable).raw()),
        }
    }
    answer
}

/// The level of a class walk's stand-in, which no lexical variable the elaborator mints shares.
pub(super) const STAND_IN_LEVEL: usize = usize::MAX;

/// What a variable an earlier class solved to `solution` reads as in a later one, over static
/// types: `solution` itself where the reach interval converged or `solution` names a rigid variable
/// of the other side — which stands for one unknown already — and otherwise a lexical variable
/// between the interval's ends.
fn read_later(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    shape: Ranked<'_>,
    variable: usize,
    reach: Interval<Handle>,
    solution: Handle,
) -> Handle {
    if reach.is_exact() || types.contains_rigid(solution) {
        return solution;
    }
    // Neither end holds a rigid variable: an end is the solution, a bound or `Never`, and the
    // solution holds none here.
    types
        .lexical_between(
            scratch,
            STAND_IN_LEVEL,
            shape.quantifiers[variable],
            super::handle::wrap(reach.lower),
            super::handle::wrap(reach.upper),
        )
        .raw()
}

/// The survivors of the class-by-class elimination over `shapes` — candidates under one key and
/// one ranking — as indices into `shapes`. At each class, a candidate some other survivor strictly
/// beats there drops out; a class that orders neither of two leaves both to the next.
pub(super) fn select_by_class<'s>(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'s>,
    shapes: &[Handle],
) -> BumpVec<'s, usize> {
    let mut survivors = BumpVec::with_capacity_in(shapes.len(), scratch);
    survivors.extend(0..shapes.len());
    let classes = shapes
        .first()
        .and_then(|shape| Ranked::of(types, *shape))
        .map_or(0, Ranked::class_count);
    for class in 0..classes {
        if survivors.len() <= 1 {
            break;
        }
        let class = u8::try_from(class).expect("a shape has fewer than 256 classes");
        let beats = |x: usize, y: usize| {
            class_at_least(types, scratch, shapes[x], shapes[y], class)
                && !class_at_least(types, scratch, shapes[y], shapes[x], class)
        };
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
