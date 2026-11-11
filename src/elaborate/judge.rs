//! The **judgement doors**: every judgement the walk and the elaborator make over a type that may
//! hold a run-bound name goes through one of these, which marks
//! ([build/README.md § Resumption](../build/README.md#resumption)).
//!
//! A door hands back what the lattice answers and marks the run-bound names of what it judged
//! **open** ([`Marks::open`]) unless its answer holds at every binding within their ends: a verdict
//! of *always* or *never* (the rigid rule), a *fits* that holds, a type that is closed. What a unit
//! only names stays named, so a binding reads its unit record with the bound type written in; what
//! it judged open re-runs. A door whose inputs are closed never marks. Marking is by default:
//! a rule that skips a door is a test failure (`the_walk_judges_through_its_doors`), never a unit
//! record wrongly shared.
//!
//! A **discrete** door — one handing back a verdict, a bool, a stamp or a kind — whose rigid
//! answer is undecided asks the marks first ([`Marks::witnessed`]): a witnessing walk answers it
//! under its witness, by the closed door, and notes the question beside the answer instead of
//! marking it undecided. The answer holds no type, and the door hands it back in rigid terms. A
//! door handing back a type never asks.
//!
//! Substitution and the constructors commute with a binding, so they are not doors; nor is any
//! judgement whose operands are [`KType`] alone, which holds no variable by its type.

use crate::memory::{BumpAllocator, BumpVec};
pub use crate::scope::free_variables;
use crate::scope::{Answer, OpenKind, Question, Restamp};
use crate::type_lattice::InstanceFailure;
use crate::type_lattice::{
    Collector, DeclaredType, Interval, Judged, KType, Parametric, PathClass, Scheme, Side,
    TypeHandle, TypeNode, UnifyFailure, Variable, Variance, Verdict, admits_with, instance_under,
    judge_by_class, read_through, select_by_class,
};
use crate::values::Chain;

/// Where a door marks: the walk's gathering, which maps each run-bound name a marked type holds
/// to its unknown and notes it open.
pub trait Marks {
    /// Mark open each run-bound name `judged` or `slot` holds: `kind` left undecided over them.
    fn open(
        &self,
        kind: OpenKind,
        slot: Option<DeclaredType<Parametric>>,
        judged: DeclaredType<Parametric>,
    );

    /// The witness's answer to `question`, noted beside it: `None` outside a witnessing walk, or
    /// where its witness cannot answer in the question's rigid terms.
    fn witnessed(&self, question: Question<'_>) -> Option<Answer> {
        let _ = question;
        None
    }
}

/// The marks of a judgement outside a walk: none is kept.
pub struct NoMarks;

impl Marks for NoMarks {
    fn open(&self, _: OpenKind, _: Option<DeclaredType<Parametric>>, _: DeclaredType<Parametric>) {}
}

/// Whether `ty` may hold a variable: a scheme is read by the marks, which find what it holds free.
fn holds_variable(types: &Chain<'_, '_, '_, '_>, ty: DeclaredType<Parametric>) -> bool {
    match ty {
        DeclaredType::Type(ty) => types.concrete(ty).is_none(),
        DeclaredType::Scheme(_) => true,
    }
}

/// Mark `kind` over `judged` against `slot`, where either may hold a variable.
fn mark(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    kind: OpenKind,
    slot: Option<DeclaredType<Parametric>>,
    judged: DeclaredType<Parametric>,
) {
    if holds_variable(types, judged) || slot.is_some_and(|slot| holds_variable(types, slot)) {
        marks.open(kind, slot, judged);
    }
}

/// Mark `kind` over both ends of `judged`.
fn mark_interval(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    kind: OpenKind,
    slot: Option<DeclaredType<Parametric>>,
    judged: Interval,
) {
    mark(marks, types, kind, slot, DeclaredType::Type(judged.upper));
    if judged.lower != judged.upper {
        mark(marks, types, kind, slot, DeclaredType::Type(judged.lower));
    }
}

/// [`judge_by_class`]: named where the verdict is *always* or *never*, which holds at every
/// binding within the arguments' ends; else the witness's verdict where it gives one over the
/// same intervals, or each argument, and the shape, marked [`Verdict`].
///
/// [`Verdict`]: OpenKind::Verdict
pub fn verdict<'x>(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'x>,
    shape: DeclaredType<Parametric>,
    arguments: &[Interval],
) -> Judged<'x> {
    let judged = judge_by_class(types, scratch, shape, arguments);
    if judged.verdict == Verdict::Maybe {
        let question = Question::Verdict {
            shape,
            arguments,
            intervals: judged.intervals,
        };
        match marks.witnessed(question) {
            Some(Answer::Verdict(verdict)) => {
                return Judged {
                    verdict,
                    intervals: judged.intervals.filter(|_| verdict != Verdict::Never),
                };
            }
            Some(Answer::Solved(verdict)) => {
                return Judged {
                    verdict,
                    intervals: Some(&[]),
                };
            }
            _ => {}
        }
        mark(marks, types, OpenKind::Verdict, None, shape);
        for argument in arguments {
            mark_interval(marks, types, OpenKind::Verdict, Some(shape), *argument);
        }
    }
    judged
}

/// The concrete type above every instance of `ty`: an end read, which no binding keeps, so it
/// marks [`EndRead`](OpenKind::EndRead).
pub fn bound_above(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    ty: Parametric,
) -> KType {
    mark(
        marks,
        types,
        OpenKind::EndRead,
        None,
        DeclaredType::Type(ty),
    );
    crate::type_lattice::bound_above(types, scratch, ty)
}

/// Where `ty` is a variable, the concrete type above every instance of it, marked as
/// [`bound_above`] marks; `None` for any other type, whose own node a rule reads.
pub fn variable_above(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    ty: Parametric,
) -> Option<KType> {
    is_variable(&types.node(ty)).then(|| bound_above(marks, types, scratch, ty))
}

/// [`bound_above`] for a scheme, its own group kept.
pub fn scheme_bound_above(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    scheme: Scheme,
) -> Scheme {
    mark(
        marks,
        types,
        OpenKind::EndRead,
        None,
        DeclaredType::Scheme(scheme),
    );
    crate::type_lattice::scheme_bound_above(types, scratch, scheme)
}

/// The kind of the concrete type above every instance of `ty`: its head's, which no binding
/// changes, save where `ty` is a variable, whose binding decides it: the witness's, else marked
/// [`Kind`](OpenKind::Kind).
pub fn kind_above(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    ty: Parametric,
) -> crate::type_lattice::KKind {
    if bare_variable(types, DeclaredType::Type(ty)) {
        if let Some(Answer::Kind(kind)) = marks.witnessed(Question::Kind { ty }) {
            return kind;
        }
        mark(marks, types, OpenKind::Kind, None, DeclaredType::Type(ty));
    }
    crate::type_lattice::bound_above(types, scratch, ty).kind_of(types)
}

/// The witness's kind for `ty`, a type name's own variable, noted beside its question: `None`
/// outside a witnessing walk. A witness-only door: no rigid kind stands for a type name a binding
/// gives a type value of its own, so its caller marks the type value instead (law 6).
pub fn witnessed_kind(marks: &dyn Marks, ty: Parametric) -> Option<crate::type_lattice::KKind> {
    match marks.witnessed(Question::Kind { ty })? {
        Answer::Kind(kind) => Some(kind),
        _ => None,
    }
}

/// `ty` read through `interval`'s answers at each variable's ends: an end read, marked
/// [`EndRead`](OpenKind::EndRead) where it reads any.
pub fn read_ends(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    ty: Parametric,
    side: Side,
    interval: &mut impl FnMut(Variable) -> Option<Interval>,
) -> Parametric {
    // A path variable's module binder is open already ([`Marks`]): only a lexical one marks.
    let mut read = false;
    let found = read_through(types, scratch, ty, side, &mut |variable| {
        let answer = interval(variable);
        read |= answer.is_some() && matches!(variable, Variable::Lexical { .. });
        answer
    });
    if read {
        mark(
            marks,
            types,
            OpenKind::EndRead,
            None,
            DeclaredType::Type(ty),
        );
    }
    found
}

/// `ty` read through `interval`'s answers where each answer is a point a binding substitutes:
/// substitution commutes, so it marks nothing.
pub fn read_points(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    ty: Parametric,
    side: Side,
    interval: &mut impl FnMut(Variable) -> Option<Interval>,
) -> Parametric {
    read_through(types, scratch, ty, side, interval)
}

/// `value` shown by the end that decides and `wanted` read through its bound, where no value of
/// the static type `value` can ever satisfy `wanted`: its upper end, read through its bound, meets
/// `wanted` at `Never`, or else its lower end, read below its variables, lies outside it. `None`
/// where one might. Named where `None` and *fits* holds rigidly at the upper end, or where neither
/// input holds a variable; noted answered where the witness may satisfy it too.
pub fn never_satisfies(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    value: Interval,
    wanted: Parametric,
) -> Option<(crate::scope::Shown, KType)> {
    let found = crate::scope::unsatisfiable(types, scratch, value, wanted);
    let slot = Some(DeclaredType::Type(wanted));
    match found {
        None if crate::type_lattice::fits(types, scratch, value.upper, wanted) => {}
        None if marks.witnessed(Question::NeverSatisfies { value, wanted })
            == Some(Answer::Satisfiable) => {}
        None => mark_interval(marks, types, OpenKind::NeverSatisfies, slot, value),
        Some(_) => mark_interval(marks, types, OpenKind::EndRead, slot, value),
    }
    found
}

/// *Fits*: named where it holds, which it does at every binding within the ends; else the
/// witness's answer, or marked [`Fits`](OpenKind::Fits) against `b`.
pub fn fits(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    a: impl Into<DeclaredType<Parametric>>,
    b: impl Into<DeclaredType<Parametric>>,
) -> bool {
    let (a, b) = (a.into(), b.into());
    let holds = crate::type_lattice::fits(types, scratch, a, b);
    if !holds {
        if let Some(Answer::Bool(holds)) = marks.witnessed(Question::Fits { a, b }) {
            return holds;
        }
        mark(marks, types, OpenKind::Fits, Some(b), a);
    }
    holds
}

/// Whether `value`'s lower end lies outside `bound`: named where it does; else the witness's
/// answer, or marked [`LowerEndOutside`](OpenKind::LowerEndOutside).
pub fn lower_end_outside(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    value: Interval,
    bound: impl Into<DeclaredType<Parametric>>,
) -> bool {
    let bound = bound.into();
    let outside = crate::type_lattice::lower_end_outside(types, scratch, value, bound);
    if !outside {
        if let Some(Answer::Bool(outside)) =
            marks.witnessed(Question::LowerEndOutside { value, bound })
        {
            return outside;
        }
        mark_interval(marks, types, OpenKind::LowerEndOutside, Some(bound), value);
    }
    outside
}

/// [`admits_with`]: what it collects is judged where the solve is read ([`solve`],
/// [`reproducible`]), which marks.
pub fn admits<'run, T: TypeHandle>(
    types: &Chain<'run, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    declared: Parametric,
    carried: T,
    variance: Variance,
    collector: &mut Collector<'_, 'run, T>,
) -> Result<(), UnifyFailure<T>> {
    admits_with(types, scratch, declared, carried, variance, collector)
}

/// Whether a run reproduces `collector`'s solve: named where it does; else every slot and
/// contribution of `involved` marked [`Solve`](OpenKind::Solve).
pub fn reproducible<'run, T: TypeHandle>(
    marks: &dyn Marks,
    types: &Chain<'run, '_, '_, '_>,
    collector: &Collector<'_, 'run, T>,
    involved: &[(Parametric, Parametric)],
) -> bool {
    let reproducible = collector.reproducible(types);
    if !reproducible {
        for (slot, contribution) in involved {
            let slot = Some(DeclaredType::Type(*slot));
            mark(
                marks,
                types,
                OpenKind::Solve,
                slot,
                DeclaredType::Type(*contribution),
            );
        }
    }
    reproducible
}

/// `collector`'s solve, marked as [`reproducible`] marks.
pub fn solve<'l, 'run>(
    marks: &dyn Marks,
    types: &Chain<'run, '_, '_, '_>,
    collector: &Collector<'l, 'run, Parametric>,
    involved: &[(Parametric, Parametric)],
) -> Result<BumpVec<'l, Parametric>, UnifyFailure<Parametric>> {
    reproducible(marks, types, collector, involved);
    collector.solve(types)
}

/// [`intervals`](crate::type_lattice::intervals): each end a solution or a bound, read where the
/// solve that gave it was marked.
pub fn intervals<'l, D: TypeHandle, T: TypeHandle>(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'l>,
    declared: &[D],
    bounds: &[KType],
    solution: &[T],
    exact: bool,
) -> BumpVec<'l, Interval<T>> {
    crate::type_lattice::intervals(types, scratch, declared, bounds, solution, exact)
}

/// [`instance_under`]: never named, since the instance a scheme takes under a type over a
/// variable depends on the binding; marked [`Instance`](OpenKind::Instance).
pub fn instance<'l, 'run: 'l>(
    marks: &dyn Marks,
    types: &Chain<'run, '_, '_, '_>,
    scratch: BumpAllocator<'l>,
    scheme: Scheme,
    wanted: Parametric,
) -> Result<BumpVec<'l, Parametric>, InstanceFailure<'l>> {
    let slot = Some(DeclaredType::Type(wanted));
    mark(
        marks,
        types,
        OpenKind::Instance,
        slot,
        DeclaredType::Scheme(scheme),
    );
    mark(
        marks,
        types,
        OpenKind::Instance,
        None,
        DeclaredType::Type(wanted),
    );
    instance_under(types, scratch, scheme, wanted)
}

/// [`select_by_class`]: named where one survives; else each shape marked
/// [`Settles`](OpenKind::Settles).
pub fn select<'l>(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'l>,
    shapes: &[DeclaredType<Parametric>],
) -> BumpVec<'l, usize> {
    let survivors = select_by_class(types, scratch, shapes);
    if survivors.len() != 1 {
        for shape in shapes {
            mark(marks, types, OpenKind::Settles, None, *shape);
        }
    }
    survivors
}

/// `ty`'s node, where a rule matches on its structure: named where it is no variable; else marked
/// [`TypeNodeOfVariable`](OpenKind::TypeNodeOfVariable).
pub fn type_node<'n>(
    marks: &dyn Marks,
    types: &Chain<'n, '_, '_, '_>,
    ty: Parametric,
) -> TypeNode<'n, Parametric> {
    let node = types.node(ty);
    if is_variable(&node) {
        mark(
            marks,
            types,
            OpenKind::TypeNodeOfVariable,
            None,
            DeclaredType::Type(ty),
        );
    }
    node
}

/// `ty`'s node where it is no variable, where a rule declines a variable rather than judges what
/// a binding gives it, so it never marks: `None` for a variable.
pub fn node_unless_variable<'n>(
    types: &Chain<'n, '_, '_, '_>,
    ty: Parametric,
) -> Option<TypeNode<'n, Parametric>> {
    let node = types.node(ty);
    (!is_variable(&node)).then_some(node)
}

/// The class of `ty` where it is a path variable's own node. A class is named by its content, so
/// reading it judges nothing a binding could change.
pub fn path_class(types: &Chain<'_, '_, '_, '_>, ty: Parametric) -> Option<PathClass> {
    match types.node(ty) {
        TypeNode::Path { class, .. } => Some(class),
        _ => None,
    }
}

/// `ty`'s node, where a rule reads off it whether a binder declared at `ty` is a module binder —
/// a discrete judgement, as a variable's node is read for its bound alone: `None` where `ty` is a
/// variable the witness binds to no signature, noted beside its question; else its node, a
/// variable's marked [`ModuleBinder`](OpenKind::ModuleBinder) where no witness answers.
pub fn module_node<'n>(
    marks: &dyn Marks,
    types: &Chain<'n, '_, '_, '_>,
    ty: Parametric,
) -> Option<TypeNode<'n, Parametric>> {
    if !bare_variable(types, DeclaredType::Type(ty)) {
        return Some(type_node(marks, types, ty));
    }
    match marks.witnessed(Question::Module { ty }) {
        Some(Answer::Bool(false)) => return None,
        Some(_) => {}
        None => mark(
            marks,
            types,
            OpenKind::ModuleBinder,
            None,
            DeclaredType::Type(ty),
        ),
    }
    Some(own_variable(types, ty))
}

/// The node of a variable a rule built itself — or of the type a binding gave it in its place —
/// read for its own level or bound: bookkeeping on the variable, not a judgement of what a binding
/// gives it, so it never marks.
pub fn own_variable<'n>(
    types: &Chain<'n, '_, '_, '_>,
    variable: Parametric,
) -> TypeNode<'n, Parametric> {
    types.node(variable)
}

/// A closed type's node: it holds no variable, so it never marks.
pub fn closed_type_node<'n>(types: &Chain<'n, '_, '_, '_>, ty: KType) -> TypeNode<'n, KType> {
    types.node(ty)
}

/// What a retype at `declared` re-stamps at, as an interval: named where it is the point at
/// `declared`, which a binding substitutes; else the point where the witness stamps at exactly
/// `declared`, or marked [`Retype`](OpenKind::Retype).
pub fn retyped(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    declared: Parametric,
) -> Interval {
    let interval = crate::values::retyped_to(types, scratch, declared);
    if interval != Interval::point(declared) {
        if marks.witnessed(Question::Retype { declared }) == Some(Answer::Stamps) {
            return Interval::point(declared);
        }
        mark(
            marks,
            types,
            OpenKind::Retype,
            None,
            DeclaredType::Type(declared),
        );
    }
    interval
}

/// How a retype to `declared` re-stamps, beside it: named where `declared` is closed; else the
/// witness's answer over the rigid `declared`, which a binding's table closes, or marked
/// [`Retype`](OpenKind::Retype).
pub fn restamp(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    declared: Parametric,
) -> Option<Restamp> {
    if let Some(closed) = types.concrete(declared) {
        let how = crate::values::Restamping::of(types, scratch, closed);
        return Some(Restamp { how, at: declared });
    }
    if let Some(Answer::Restamps(how)) = marks.witnessed(Question::Restamp { declared }) {
        return Some(Restamp { how, at: declared });
    }
    mark(
        marks,
        types,
        OpenKind::Retype,
        None,
        DeclaredType::Type(declared),
    );
    None
}

/// `ty` as a closed type, where a rule branches on whether it is: named where it is; else marked
/// [`Unclosed`](OpenKind::Unclosed).
pub fn closed(marks: &dyn Marks, types: &Chain<'_, '_, '_, '_>, ty: Parametric) -> Option<KType> {
    let closed = types.concrete(ty);
    if closed.is_none() {
        mark(
            marks,
            types,
            OpenKind::Unclosed,
            None,
            DeclaredType::Type(ty),
        );
    }
    closed
}

/// [`closed`] for a declared type: a scheme is closed as it is.
pub fn closed_declared(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    ty: DeclaredType<Parametric>,
) -> Option<DeclaredType<KType>> {
    let closed = types.concrete_declared(ty);
    if closed.is_none() {
        mark(marks, types, OpenKind::Unclosed, None, ty);
    }
    closed
}

/// `ty` as a closed type, where a rule keeps it rigid otherwise: a binding substitutes the rigid
/// type into what the closed one would give, so it never marks.
pub fn static_of(types: &Chain<'_, '_, '_, '_>, ty: Parametric) -> Option<KType> {
    types.concrete(ty)
}

/// [`static_of`] for a declared type.
pub fn static_declared(
    types: &Chain<'_, '_, '_, '_>,
    ty: DeclaredType<Parametric>,
) -> Option<DeclaredType<KType>> {
    types.concrete_declared(ty)
}

/// Whether `a` and `b` are one type: named where they are, or where their heads are no variable
/// and differ, which no binding makes alike; else the witness's answer, or marked
/// [`Unequal`](OpenKind::Unequal).
pub fn same(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    a: impl Into<DeclaredType<Parametric>>,
    b: impl Into<DeclaredType<Parametric>>,
) -> bool {
    let (a, b) = (a.into(), b.into());
    let same = a == b;
    // Two non-variable heads over variables may come out one under a binding.
    if !same && (holds_variable(types, a) || holds_variable(types, b)) {
        if let Some(Answer::Bool(same)) = marks.witnessed(Question::Same { a, b }) {
            return same;
        }
        mark(marks, types, OpenKind::Unequal, Some(b), a);
    }
    same
}

/// Whether `ty` is a variable itself.
fn bare_variable(types: &Chain<'_, '_, '_, '_>, ty: DeclaredType<Parametric>) -> bool {
    match ty {
        DeclaredType::Type(ty) => types.concrete(ty).is_none() && is_variable(&types.node(ty)),
        DeclaredType::Scheme(_) => false,
    }
}

/// `typed`'s type where it is exact and closed: named where it is; else its ends marked, where a
/// binding may make it so.
pub fn exactly(marks: &dyn Marks, types: &Chain<'_, '_, '_, '_>, typed: Interval) -> Option<KType> {
    if exact(marks, types, typed) {
        return closed(marks, types, typed.upper);
    }
    None
}

/// Whether `interval` is one type: [`same`] over its ends.
pub fn exact(marks: &dyn Marks, types: &Chain<'_, '_, '_, '_>, interval: Interval) -> bool {
    same(marks, types, interval.lower, interval.upper)
}

/// Whether `ty` is the closed type `constant`: named where it is, or where it holds a variable
/// only below a head that is no variable; else the witness's answer, or marked
/// [`Unequal`](OpenKind::Unequal).
pub fn is(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    ty: impl Into<DeclaredType<Parametric>>,
    constant: KType,
) -> bool {
    let ty = ty.into();
    let constant = DeclaredType::Type(Parametric::from(constant));
    let is = ty == constant;
    if !is && bare_variable(types, ty) {
        if let Some(Answer::Bool(is)) = marks.witnessed(Question::Same { a: ty, b: constant }) {
            return is;
        }
        mark(marks, types, OpenKind::Unequal, Some(constant), ty);
    }
    is
}

/// Whether `node` is a variable's own node.
fn is_variable<H: TypeHandle>(node: &TypeNode<'_, H>) -> bool {
    matches!(
        node,
        TypeNode::Quantified { .. }
            | TypeNode::Lexical { .. }
            | TypeNode::Path { .. }
            | TypeNode::Parameter { .. }
    )
}
