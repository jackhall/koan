//! Class-by-class admission and ranking: how an expression shape's priority classes order the
//! solving of its group and the comparison of two candidates under one bucket key.
//!
//! A shape's slots sit in dense priority classes ([`dense_classes`](super::shape::dense_classes));
//! written order puts each slot in a class of its own. [`admit_by_class`] solves a quantified group
//! one class at a time: the first class whose slots mention a variable solves it, jointly over that
//! class's slots, and every later class admits its arguments against that solution, which is
//! [pinned](super::unify::Collector::pin) in the class's collector. So `FOR ALL #[Elt] #(PAIR x
//! :Elt WITH y :Elt)` fixes `Elt` from `x` and then refuses a `y` that does not lie under it.
//! [`admits_shape`](super::sig_relations) runs the same loop with a candidate shape's slot types as
//! the arguments, so the order's instantiation clause and a keyworded call agree.
//!
//! [`class_at_least`] is the per-class specificity verdict dispatch ranks by, recorded in the
//! registry's verdict table under [`Relation::ClassAtLeast`]. It differs from admission in how an
//! earlier class's outcome reads later: a class that admitted leaves each variable it solved as an
//! **unknown type under that solution** — a fresh rigid variable bounded by it, since at a call the
//! variable solves to an argument's carried type, which can be anything under the candidate's slot
//! — and a class that did not admit leaves each as its bound. A solution already holding a rigid
//! variable of the other candidate stands for an unknown already and is kept. [`select_by_class`]
//! runs the elimination over a candidate list: at each class, every candidate another strictly
//! beats there drops out, and the rest go on.

use crate::memory::{BumpAllocator, BumpVec, ScopeId};
use crate::symbols::TypeSymbol;

use super::handle::KType;
use super::node::TypeNode;
use super::registry::{Relation, TypeRegistry};
use super::shape::{DispatchTokenElement, class_of};
use super::unify::{Collector, admits_with};
use super::walk::Variance;

/// An expression shape's parts, read once off its node for a class-by-class walk.
#[derive(Clone, Copy)]
pub(super) struct Ranked<'run> {
    pub(super) quantifiers: &'run [TypeSymbol],
    pub(super) bounds: &'run [KType],
    pub(super) elements: &'run [DispatchTokenElement],
    pub(super) classes: &'run [u8],
    pub(super) ret: KType,
}

impl<'run> Ranked<'run> {
    /// `kt`'s shape parts, or `None` for anything that is not a shape.
    pub(super) fn of(types: &TypeRegistry<'run>, kt: KType) -> Option<Self> {
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
                elements,
                classes,
                ret,
            }),
            _ => None,
        }
    }

    /// The slot types, in slot order.
    pub(super) fn slots(self) -> impl Iterator<Item = KType> + use<'run> {
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
    slots: BumpVec<'s, KType>,
    /// Per variable, the lowest class whose slots mention it; `None` for one only the return does.
    first: BumpVec<'s, Option<u8>>,
    fixed: BumpVec<'s, Option<KType>>,
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
    fn collector(&self, scratch: BumpAllocator<'s>) -> Collector<'s> {
        let mut collector = Collector::new(scratch, self.fixed.len());
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
        arguments: &[KType],
        class: u8,
    ) -> Option<BumpVec<'s, KType>> {
        let mut collector = self.collector(scratch);
        for (index, slot) in self.slots.iter().enumerate() {
            if class_of(self.declared.classes, index) == class
                && admits_with(
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
    fn fix(&mut self, class: u8, mut read: impl FnMut(usize) -> KType) {
        for variable in 0..self.fixed.len() {
            if self.first[variable] == Some(class) {
                self.fixed[variable] = Some(read(variable));
            }
        }
    }

    /// Admit every class in order, fixing each class's variables to its solution, or `None` at the
    /// first class that does not admit.
    fn admit_all(
        &mut self,
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'s>,
        arguments: &[KType],
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
/// call admits by. The solution in the shape's canonical group order, a variable no slot mentions
/// reading as its bound; `None` when some class does not admit, when `declared` is not a shape,
/// or when the arguments are not one per slot.
pub fn admit_by_class<'s>(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'s>,
    declared: KType,
    arguments: &[KType],
) -> Option<&'s [KType]> {
    let declared = Ranked::of(types, declared)?;
    let mut walk = ClassWalk::new(types, scratch, declared);
    if walk.slots.len() != arguments.len() {
        return None;
    }
    walk.admit_all(types, scratch, arguments)?;
    Some(scratch.alloc_slice_fill_iter(
        (0..walk.fixed.len()).map(|index| walk.fixed[index].unwrap_or(declared.bound(index))),
    ))
}

/// Whether `declared` admits `candidate`'s slot types class by class — `declared`'s variables
/// solved, `candidate`'s rigid — and `declared`'s return then lies under `candidate`'s. Both must be
/// shapes under one key and one ranking; the caller checks that.
pub(super) fn admits_by_class<'run>(
    types: &TypeRegistry<'run>,
    scratch: BumpAllocator<'_>,
    declared: Ranked<'run>,
    candidate: Ranked<'run>,
) -> bool {
    let mut walk = ClassWalk::new(types, scratch, declared);
    let mut arguments = BumpVec::new_in(scratch);
    arguments.extend(candidate.slots());
    if walk.admit_all(types, scratch, &arguments).is_none() {
        return false;
    }
    let mut collector = walk.collector(scratch);
    admits_with(
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
/// own, jointly, with each variable an earlier class admitted read as an unknown type under its
/// solution and each one an earlier class refused read as its bound. `a`'s variables are rigid.
///
/// A function of the two shape types and the class alone, so it is recorded in the verdict table;
/// one pass computes every class's verdict for the pair and records them all.
pub fn class_at_least(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    a: KType,
    b: KType,
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
            Some(solution) => walk.fix(each, |variable| {
                unknown_under(types, scratch, ranked_b, variable, solution[variable])
            }),
            None => walk.fix(each, |variable| ranked_b.bound(variable)),
        }
    }
    answer
}

/// What a variable an earlier class solved to `solution` reads as in a later one: an unknown type
/// under the solution, spelled as a rigid variable bounded by it. A solution that holds a rigid
/// variable already names an unknown and is kept. The stand-in's nonce keeps it apart from every
/// variable a signature member or an opaque ascription mints.
fn unknown_under(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    shape: Ranked<'_>,
    variable: usize,
    solution: KType,
) -> KType {
    if types.contains_rigid(solution) {
        return solution;
    }
    types.abstract_type(
        scratch,
        ScopeId::SENTINEL,
        shape.quantifiers[variable],
        &[],
        Some(ScopeId::SENTINEL),
        solution,
    )
}

/// The survivors of the class-by-class elimination over `shapes` — candidates under one key and
/// one ranking — as indices into `shapes`. At each class, a candidate some other survivor strictly
/// beats there drops out; a class that orders neither of two leaves both to the next.
pub fn select_by_class<'s>(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'s>,
    shapes: &[KType],
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
