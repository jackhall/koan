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
//! A type that depends on a question's answer goes through [`decided`], and a rule with no question
//! through [`ruled`]: each hands back the same type at load and under every binding — a type
//! written over names, or a derived name typed on the question's answers or by the rule — and
//! never reads the witness ([derived names](crate::scope::DerivedTerm)).
//!
//! Substitution and the constructors commute with a binding, so they are not doors; nor is any
//! judgement whose operands are [`KType`] alone, which holds no variable by its type.

use crate::memory::collect;
use crate::memory::{BumpAllocator, BumpVec};
pub use crate::scope::free_variables;
pub use crate::scope::rigid_answer;
use crate::scope::{
    Answer, Decider, DerivedId, DerivedTerm, NeverSatisfied, OpenKind, Question, Restamp, Rule,
    bare_variable, constructed_upper, handed, is_module, is_variable, kind_varies, may_be_never,
    tag_union,
};
use crate::type_lattice::{
    Collector, DeclaredType, Interval, Judged, KType, Parametric, PathClass, Scheme, Side,
    Solution, SolveRecord, Step, TypeHandle, TypeNode, UnifyFailure, Variable, Variance, Verdict,
    admits_with, instance_under, read_through, select_by_class,
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

    /// The [derived name](crate::scope::DerivedName) for `term`, named by its content and laid
    /// where the walk's store keeps the names it meets. `None` outside a walk, where no binding is
    /// written into what a door hands back.
    fn derive(&self, term: DerivedTerm<'_>) -> Option<Parametric> {
        let _ = term;
        None
    }

    /// The term the derived name `id` names, where the walk's store holds it: `None` outside a
    /// walk.
    fn term(&self, id: DerivedId) -> Option<DerivedTerm<'_>> {
        let _ = id;
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

/// [`judge_signed`](crate::scope::judge_signed): the verdict named where it is *always* or *never*, which holds at every
/// binding within the arguments' ends; else the witness's, or each argument, and the shape, marked
/// [`Verdict`](OpenKind::Verdict). The intervals are [`judged_intervals`]'s, the same at load and
/// under every binding. A *never* hands back none.
pub fn verdict<'x>(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'x>,
    shape: DeclaredType<Parametric>,
    arguments: &[Interval],
) -> Judged<'x> {
    let judged = crate::scope::judge_signed(types, scratch, shape, arguments);
    witnessed_verdict(marks, types, scratch, (shape, arguments), judged)
}

/// [`judge_signed`](crate::scope::judge_signed) alone: no witness read and nothing marked. *Always* and *never* hold at every
/// binding within the arguments' ends; a caller reading it judges through [`witnessed_verdict`]
/// too.
pub fn judged<'x>(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'x>,
    shape: DeclaredType<Parametric>,
    arguments: &[Interval],
) -> Judged<'x> {
    crate::scope::judge_signed(types, scratch, shape, arguments)
}

/// [`verdict`] past its lattice judgement `judged` of `shape` over `arguments`: for a caller that
/// reads the lattice's own judgement too, which every binding gives alike.
pub fn witnessed_verdict<'x>(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'x>,
    (shape, arguments): (DeclaredType<Parametric>, &[Interval]),
    judged: Judged<'x>,
) -> Judged<'x> {
    let mut verdict = judged.verdict;
    if verdict == Verdict::Maybe {
        match marks.witnessed(Question::Verdict { shape, arguments }) {
            Some(Answer::Verdict(witnessed)) => verdict = witnessed,
            _ => {
                mark(marks, types, OpenKind::Verdict, None, shape);
                for argument in arguments {
                    mark_interval(marks, types, OpenKind::Verdict, Some(shape), *argument);
                }
            }
        }
    }
    let solved = match verdict {
        Verdict::Never => None,
        _ => judged_intervals(marks, types, scratch, (shape, arguments), judged).map(|intervals| {
            let types = scratch.alloc_slice_fill_iter(intervals.iter().map(|each| each.upper));
            Solution { types, intervals }
        }),
    };
    let solve = SolveRecord {
        solved,
        ..judged.solve
    };
    Judged { verdict, solve }
}

/// The intervals `judged`, the lattice's judgement of `shape` over `arguments`, solves the group
/// to, the same at load and under every binding: the solve's own where its record is
/// reproducible, so a binding written in gives the run's; where it is not, each end the output of
/// [`Rule::Judged`] over the shape and the arguments, which a binding judges again, whether or not
/// the load's judgement solved. `None` where none solves.
pub fn judged_intervals<'x>(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'x>,
    (shape, arguments): (DeclaredType<Parametric>, &[Interval]),
    judged: Judged<'x>,
) -> Option<&'x [Interval]> {
    let bounds = match shape {
        DeclaredType::Scheme(scheme) => scheme.group(types).bounds,
        DeclaredType::Type(_) => &[],
    };
    if judged.solve.reproducible || bounds.is_empty() || bounds.len() > 127 {
        return judged.solve.intervals();
    }
    let mut operands = BumpVec::with_capacity_in(1 + 2 * arguments.len(), scratch);
    operands.push(shape);
    for argument in arguments {
        operands.push(DeclaredType::Type(argument.lower));
        operands.push(DeclaredType::Type(argument.upper));
    }
    let operands = collect(scratch, operands.iter().copied());
    let rule = Rule::Judged {
        variables: bounds.len() as u8,
    };
    let mut undecided = BumpVec::with_capacity_in(2 * bounds.len(), scratch);
    for bound in bounds {
        undecided.extend([KType::NEVER.into(), Parametric::from(*bound)]);
    }
    let ends = ruled(marks, types, scratch, (rule, operands), &undecided)?;
    let mut intervals = BumpVec::with_capacity_in(bounds.len(), scratch);
    intervals.extend(ends.chunks_exact(2).map(|pair| Interval {
        lower: pair[0],
        upper: pair[1],
    }));
    Some(intervals.leak())
}

/// Where `ty` is a variable no binding gives a type — a `FOR ALL` variable or a head parameter —
/// the concrete type above every instance of it, which is the same at every binding, so it never
/// marks; `None` for any other type, whose own node a rule reads, and for a run-bound name, whose
/// member a rule's question or a path reads.
pub fn variable_above(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    ty: Parametric,
) -> Option<KType> {
    (is_variable(&types.node(ty)) && names_held(types, scratch, ty) == Names::Unbound)
        .then(|| crate::type_lattice::bound_above(types, scratch, ty))
}

/// The kind of the concrete type above every instance of `ty`: its head's, which no binding
/// changes, save where `ty` is a variable or a union holding one, whose binding decides it: the
/// witness's, else marked [`Kind`](OpenKind::Kind).
pub fn kind_above(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    ty: Parametric,
) -> crate::type_lattice::KKind {
    if kind_varies(types, ty) {
        if let Some(Answer::Kind(kind)) = marks.witnessed(Question::Kind { ty }) {
            return kind;
        }
        mark(marks, types, OpenKind::Kind, None, DeclaredType::Type(ty));
    }
    crate::type_lattice::bound_above(types, scratch, ty).kind_of(types)
}

/// The kind of the concrete type above every instance of `ty`, where a rule acts on it exactly:
/// its head's, which no binding changes, or for a variable or a union holding one the witness's;
/// `None` for one no witness answers, marked [`Kind`](OpenKind::Kind), whose binding decides it.
pub fn kind_decided(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    ty: Parametric,
) -> Option<crate::type_lattice::KKind> {
    if !kind_varies(types, ty) {
        return Some(crate::type_lattice::bound_above(types, scratch, ty).kind_of(types));
    }
    if let Some(Answer::Kind(kind)) = marks.witnessed(Question::Kind { ty }) {
        return Some(kind);
    }
    mark(marks, types, OpenKind::Kind, None, DeclaredType::Type(ty));
    None
}

/// The type of a type value of `ty`: the kind its head fixes, or for a variable the decided name
/// typed on the kind each binding gives it, so a type name's own value is one type at load and
/// under every binding. Outside a walk, `Any` type.
pub fn kind_value(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    ty: Parametric,
) -> Parametric {
    let mut arm = |answer| match answer {
        Answer::Kind(kind) => KType::of_kind(kind).into(),
        _ => unreachable!("a kind question answers a kind"),
    };
    let question = Question::Kind { ty };
    decided_by(
        marks,
        types,
        scratch,
        question,
        &mut arm,
        KType::ANY_TYPE.into(),
    )
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

/// Where no value of the static type `value` can ever satisfy `wanted`, the refusal over both,
/// worded where it renders ([`NeverSatisfied`]): its upper end, read through its bound, meets
/// `wanted` at `Never`, or else its lower end, read below its variables, lies outside it. `None`
/// where one might.
///
/// A rigid refusal marks nothing: it holds at every binding. A binding `B` of the upper end lies
/// under its bound and one of `wanted` under its own, so their meet is `Never` too; and `B` of the
/// lower end lies above the lower end read below its variables, so it lies outside a type under
/// `wanted`'s bound too. Else named where *fits* holds rigidly at the upper end, or the witness's
/// answer, or marked [`NeverSatisfies`](OpenKind::NeverSatisfies).
pub fn never_satisfies(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    value: Interval,
    wanted: Parametric,
) -> Option<NeverSatisfied> {
    let refused = Some(NeverSatisfied { value, wanted });
    if crate::scope::unsatisfiable(types, scratch, value, wanted).is_some() {
        return refused;
    }
    if crate::type_lattice::fits(types, scratch, value.upper, wanted) {
        return None;
    }
    match marks.witnessed(Question::NeverSatisfies { value, wanted }) {
        Some(Answer::Satisfiable) => None,
        Some(Answer::Never) => refused,
        _ => {
            let slot = Some(DeclaredType::Type(wanted));
            mark_interval(marks, types, OpenKind::NeverSatisfies, slot, value);
            None
        }
    }
}

/// Whether no value of the static type `value` can ever satisfy `wanted`: [`never_satisfies`] for
/// a rule that words no refusal.
pub fn never_satisfied(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    value: Interval,
    wanted: Parametric,
) -> bool {
    never_satisfies(marks, types, scratch, value, wanted).is_some()
}

/// The first of `against`, the closed builtin overloads at a registration's key, that the
/// unquantified `registered` overlaps, by position: named where every binding agrees, closed or
/// overlapping none at its bounds; else the witness's answer, or marked with
/// [`Question::Overlaps`]'s kind, since a binding may make it overlap.
pub fn overlapping(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    registered: Parametric,
    against: &[KType],
) -> Option<usize> {
    let question = Question::Overlaps {
        registered,
        against,
    };
    let answered = match rigid_answer(types, scratch, &question) {
        Some(answered) => Some(answered),
        None => marks.witnessed(question),
    };
    match answered {
        Some(Answer::Overlaps(at)) => at.map(usize::from),
        _ => {
            mark(marks, types, question.kind(), None, registered.into());
            None
        }
    }
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

/// *Fits*, where a rule acts on its failure: `Some` where it holds, where neither type holds a
/// variable, or where the witness answers; else `None`, marked [`Fits`](OpenKind::Fits), since a
/// binding decides it. A rule refuses only on `Some(false)`, which holds at every binding.
pub fn fit_decided(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    a: impl Into<DeclaredType<Parametric>>,
    b: impl Into<DeclaredType<Parametric>>,
) -> Option<bool> {
    let (a, b) = (a.into(), b.into());
    if crate::type_lattice::fits(types, scratch, a, b) {
        return Some(true);
    }
    if !holds_variable(types, a) && !holds_variable(types, b) {
        return Some(false);
    }
    if let Some(Answer::Bool(holds)) = marks.witnessed(Question::Fits { a, b }) {
        return Some(holds);
    }
    mark(marks, types, OpenKind::Fits, Some(b), a);
    None
}

/// Whether `a` lies under `b` in the order, where a rule acts on either answer: `Some` where both
/// are closed, where `a` is a variable whose declared bound lies under a closed `b` — every
/// binding of it does too — or where the witness answers; else `None`, marked
/// [`Under`](OpenKind::Under), since the order relates closed types alone and a binding decides.
pub fn under_decided(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    a: Parametric,
    b: Parametric,
) -> Option<bool> {
    match (types.concrete(a), types.concrete(b)) {
        (Some(a), Some(b)) => {
            return Some(crate::type_lattice::is_subtype_of(types, scratch, a, b));
        }
        (None, Some(b)) if rigid_within(types, scratch, a, b) => return Some(true),
        _ => {}
    }
    if let Some(Answer::Bool(under)) = marks.witnessed(Question::Under { a, b }) {
        return Some(under);
    }
    mark(marks, types, OpenKind::Under, Some(b.into()), a.into());
    None
}

/// Whether the registered shape `a` strictly outranks `b` at `class`, one bit of a use's ranking
/// table: `Some` where both are closed or where the witness answers; else `None`, marked with
/// [`Question::Outranks`]'s kind, since a binding ranks a rigid shape.
pub fn outranks(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    (a, b): (DeclaredType<Parametric>, DeclaredType<Parametric>),
    class: u16,
) -> Option<bool> {
    if let (Some(a), Some(b)) = (types.concrete_declared(a), types.concrete_declared(b)) {
        let ranked = crate::type_lattice::outranks(types, scratch, a.into(), b.into(), class);
        return Some(ranked);
    }
    let question = Question::Outranks { a, b, class };
    if let Some(Answer::Bool(ranked)) = marks.witnessed(question) {
        return Some(ranked);
    }
    mark(marks, types, question.kind(), Some(b), a);
    None
}

/// Whether `a` lies under the closed `b` at every binding: the order where `a` is closed, and
/// where it is a variable, whether its declared bound does. It reads nothing a binding decides, so
/// it never marks: where a binding alone would put `a` under `b`, it is no.
pub fn under_every_binding(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    a: Parametric,
    b: KType,
) -> bool {
    match types.concrete(a) {
        Some(a) => crate::type_lattice::is_subtype_of(types, scratch, a, b),
        None => rigid_within(types, scratch, a, b),
    }
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
    // A signature the lattice reads below as `Never` still declares its member names at every pin.
    let outside = crate::type_lattice::lower_end_outside(types, scratch, value, bound)
        || (bound.as_type().and_then(|bound| types.concrete(bound)))
            .is_some_and(|asked| crate::scope::signature_outside(types, value.lower, asked));
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
/// contribution of `involved` marked [`Unreproduced`](OpenKind::Unreproduced).
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
                OpenKind::Unreproduced,
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

/// [`class_step`](crate::type_lattice::class_step): a solve record, whose
/// [`reproducible`](SolveRecord::reproducible) says whether a run solves it alike. A caller marks
/// one it does not store whole ([`stored_solve`]) through [`unreproduced`].
pub fn class_step<'l, 'run: 'l>(
    types: &Chain<'run, '_, '_, '_>,
    scratch: BumpAllocator<'l>,
    bounds: &[KType],
    pinned: &[Option<Parametric>],
    slots: &[(Parametric, Parametric)],
    exact: bool,
) -> SolveRecord<'l> {
    crate::type_lattice::class_step(types, scratch, bounds, pinned, slots, exact)
}

/// Mark each of `slots` (a slot beside the type admitted there)
/// [`Unreproduced`](OpenKind::Unreproduced): a solve a run may not reproduce, which no stored solve
/// stands for.
pub fn unreproduced(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    slots: &[(Parametric, Parametric)],
) {
    for (slot, contribution) in slots {
        let slot = Some(DeclaredType::Type(*slot));
        mark(
            marks,
            types,
            OpenKind::Unreproduced,
            slot,
            DeclaredType::Type(*contribution),
        );
    }
}

/// A solve the load cannot write over the names it reads, stored whole as [`Rule::Solved`]: the
/// callee's `scheme`, its group pinned as `pinned`, solved by one class step over `carried` — each
/// involved slot's position, in slot order, beside the type it contributes — over exact
/// contributions where `exact` says so. Whether it solves is [`Question::Defined`]: `None` where
/// every binding, or the witness, answers no. Else each variable's solution, as [`ruled`] hands it
/// back: closed, or the derived name the stored solve types, so a run solves it under its own
/// binding. Asked where the step is not one a run reproduces; `carried` and the group fit a
/// stored solve's bits.
pub fn stored_solve<'x>(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'x>,
    scheme: Scheme,
    pinned: &[Option<Parametric>],
    carried: &[(usize, Parametric)],
    exact: bool,
) -> Option<&'x [Parametric]> {
    debug_assert!(pinned.len() < 64 && carried.iter().all(|(position, _)| *position < 64));
    let mut operands = BumpVec::with_capacity_in(1 + carried.len() + pinned.len(), scratch);
    operands.push(DeclaredType::Scheme(scheme));
    let mut involved = 0u64;
    for (position, contribution) in carried {
        involved |= 1 << position;
        operands.push(DeclaredType::Type(*contribution));
    }
    let mut pins = 0u64;
    for (variable, pin) in pinned.iter().enumerate() {
        if let Some(pin) = pin {
            pins |= 1 << variable;
            operands.push(DeclaredType::Type(*pin));
        }
    }
    let rule = Rule::Solved {
        involved,
        pinned: pins,
        exact,
        variables: pinned.len() as u8,
    };
    let operands = collect(scratch, operands.iter().copied());
    let question = Question::Defined { rule, operands };
    let answered = rigid_answer(types, scratch, &question).or_else(|| marks.witnessed(question));
    match answered {
        Some(Answer::Bool(false)) => return None,
        Some(_) => {}
        None => {
            for operand in &operands[1..] {
                mark(marks, types, question.kind(), None, *operand);
            }
        }
    }
    let bounds = scheme.group(types).bounds;
    let mut undecided = BumpVec::with_capacity_in(bounds.len(), scratch);
    undecided.extend(bounds.iter().map(|bound| Parametric::from(*bound)));
    ruled(marks, types, scratch, (rule, operands), &undecided)
}

/// What `(head payload)` builds over a static payload type, through the doors: a newtype's fit is
/// the [`fits`] question of the payload against its representation, which the closed rule's own
/// satisfaction is, and a family's solve is [`admits`] and [`solve`], applied where the load writes
/// it over names. Where a run may solve the family otherwise, the operands of
/// [`Rule::Constructed`] instead, whose closed rule the end's type is.
fn built<'x>(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'x>,
    (head, payload): (KType, Parametric),
) -> Result<Built<'x>, crate::values::ConstructionRefused<Parametric>> {
    use crate::type_lattice::NodeSchema;
    use crate::values::ConstructionRefused;
    match types.member_schema(head) {
        Some(NodeSchema::NewType(representation)) => {
            match fits(marks, types, scratch, payload, representation) {
                true => Ok(Built::Typed(head.into())),
                false => Err(ConstructionRefused::Misfit {
                    identity: head,
                    representation,
                    payload,
                }),
            }
        }
        Some(NodeSchema::TypeConstructor {
            representation: Some(representation),
            param_names,
        }) => {
            let mut collector = Collector::<Parametric>::least(types, scratch, param_names.len());
            let refused = ConstructionRefused::Unsolved {
                family: head,
                payload,
            };
            admits(
                types,
                scratch,
                representation,
                payload,
                Variance::Co,
                &mut collector,
            )
            .map_err(|_| refused)?;
            if !collector.reproducible(types) {
                let operands = [DeclaredType::Type(head.into()), DeclaredType::Type(payload)];
                return Ok(Built::Unreproduced(collect(
                    scratch,
                    operands.iter().copied(),
                )));
            }
            let involved = [(representation, payload)];
            let solved = solve(marks, types, &collector, &involved).map_err(|_| refused)?;
            let mut arguments = BumpVec::with_capacity_in(param_names.len(), scratch);
            arguments.extend(
                (param_names.iter().zip(solved.iter()))
                    .map(|(name, solved)| (crate::symbols::BinderSymbol::Type(*name), *solved)),
            );
            Ok(Built::Typed(
                types.constructor_apply(scratch, head, &arguments),
            ))
        }
        _ => Err(ConstructionRefused::NotConstructible(head)),
    }
}

/// [`built`]'s answer: the type built, or the operands of a construction a run may solve otherwise.
enum Built<'x> {
    Typed(Parametric),
    Unreproduced(&'x [DeclaredType<Parametric>]),
}

/// The lower end of `(head payload)` over a static payload type ([`built`]). Where a run may solve
/// the family otherwise, the type on whether it solves ([`Question::Defined`] over
/// [`Rule::Constructed`]): the rule's output where it does, `Never` where it does not, so it is the
/// same at load and under every binding.
pub fn construction(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    (head, payload): (KType, Parametric),
) -> Result<Parametric, crate::values::ConstructionRefused<Parametric>> {
    let refused = crate::values::ConstructionRefused::Unsolved {
        family: head,
        payload,
    };
    let operands = match built(marks, types, scratch, (head, payload))? {
        Built::Typed(built) => return Ok(built),
        Built::Unreproduced(operands) => operands,
    };
    let rule = Rule::Constructed;
    let never = Parametric::from(KType::NEVER);
    let constructed = ruled(marks, types, scratch, (rule, operands), &[head.into()]);
    let constructed = constructed.ok_or(refused)?[0];
    let question = Question::Defined { rule, operands };
    Ok(decided(
        marks,
        types,
        scratch,
        (question, &[never, constructed]),
        never,
    ))
}

/// The upper end of `(head payload)` over a static payload type, reached through the union `owner`
/// peels to where the head is its tag projection ([`constructed_upper`]). Where a run may solve the
/// family otherwise, [`Rule::Construction`]'s upper output, the closed rule over both outcomes, so
/// it is the same at load and under every binding.
pub fn construction_upper(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    (head, payload): (KType, Parametric),
    owner: Option<KType>,
) -> Parametric {
    let through = owner.and_then(|owner| tag_union(types, scratch, owner));
    let built = match built(marks, types, scratch, (head, payload)) {
        Ok(Built::Typed(built)) => Ok(built),
        Err(refused) => Err(refused),
        Ok(Built::Unreproduced(_)) => {
            let mut operands = BumpVec::with_capacity_in(3, scratch);
            operands.extend([DeclaredType::Type(head.into()), DeclaredType::Type(payload)]);
            operands.extend(owner.map(|owner| DeclaredType::Type(owner.into())));
            let rule = Rule::Construction {
                upper: true,
                projected: owner.is_some(),
            };
            let bare = Parametric::from(through.unwrap_or(head));
            return ruled(marks, types, scratch, (rule, &operands), &[bare])
                .map_or(bare, |upper| upper[0]);
        }
    };
    constructed_upper(types, scratch, (head, through), built)
}

/// [`instance_under`]: named where its solve record is reproducible, so a binding written in gives
/// the run's. Where it is not, the instance is fixed at each variable's output of
/// [`Rule::InstanceAt`], which a binding instantiates again, whether or not the load's solve
/// succeeded: whether one is fixed is [`Question::Defined`], and a no — at every binding or the
/// witness's — is no solve. Undecided, its types are marked with the question's kind.
pub fn instance<'l, 'run: 'l>(
    marks: &dyn Marks,
    types: &Chain<'run, '_, '_, '_>,
    scratch: BumpAllocator<'l>,
    scheme: Scheme,
    wanted: Parametric,
) -> SolveRecord<'l> {
    let record = instance_under(types, scratch, scheme, wanted);
    let bounds = scheme.group(types).bounds;
    if record.reproducible || bounds.len() > usize::from(u8::MAX) {
        return record;
    }
    let rule = Rule::InstanceAt {
        variables: bounds.len() as u8,
    };
    let operands = [DeclaredType::Scheme(scheme), DeclaredType::Type(wanted)];
    let operands = collect(scratch, operands.iter().copied());
    let unsolved = SolveRecord {
        solved: None,
        ..record
    };
    let question = Question::Defined { rule, operands };
    match rigid_answer(types, scratch, &question).or_else(|| marks.witnessed(question)) {
        Some(Answer::Bool(false)) => return unsolved,
        Some(_) => {}
        None => {
            let scheme = DeclaredType::Scheme(scheme);
            mark(marks, types, question.kind(), Some(wanted.into()), scheme);
            mark(marks, types, question.kind(), None, wanted.into());
        }
    }
    let mut undecided = BumpVec::with_capacity_in(bounds.len(), scratch);
    undecided.extend(bounds.iter().map(|bound| Parametric::from(*bound)));
    let Some(solution) = ruled(marks, types, scratch, (rule, operands), &undecided) else {
        return unsolved;
    };
    let intervals =
        scratch.alloc_slice_fill_iter(solution.iter().map(|each| Interval::point(*each)));
    let reached = scratch.alloc_slice_fill_iter(bounds.iter().map(|_| true));
    SolveRecord {
        solved: Some(Solution {
            types: solution,
            intervals,
        }),
        reached,
        reproducible: false,
    }
}

/// Whether `ty` is a function type, as an instance is wanted at one: named where its head says,
/// at every binding alike; else the witness's answer, or marked with [`Question::IsFunction`]'s
/// kind and taken as none.
pub fn is_function(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    ty: Parametric,
) -> bool {
    let question = Question::IsFunction { ty };
    match rigid_answer(types, scratch, &question).or_else(|| marks.witnessed(question)) {
        Some(Answer::Bool(function)) => function,
        _ => {
            mark(marks, types, question.kind(), None, ty.into());
            false
        }
    }
}

/// [`select_by_class`] over `shapes`, each closed, which no binding changes, so it never marks.
pub fn select<'l>(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'l>,
    shapes: &[DeclaredType<Parametric>],
) -> BumpVec<'l, usize> {
    select_by_class(types, scratch, shapes)
}

/// A piece of a lexical variable, or of a projection of one, that the variable's bound guarantees.
#[derive(Clone, Copy, Debug)]
pub struct Piece {
    /// The piece: a projection, or the type it reduces to.
    pub ty: Parametric,
    /// The piece of the base's declared bound: the closed type above every binding's piece, which
    /// a *never* refusal over the piece reads, since what it refuses every binding refuses.
    pub bound: KType,
}

/// The `step` piece of whatever `base` is bound to, where `base` is a lexical variable, or a
/// projection of one, whose bound guarantees the piece; `None` for any other `base`, a concrete
/// one included, which its caller reads structurally. A constructor over the variable's declared
/// ends, which no binding changes (the piece of a binding is the projection substituted), so it
/// never marks.
pub fn projection(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    base: Parametric,
    step: Step,
) -> Option<Piece> {
    if !matches!(
        types.node(base),
        TypeNode::Lexical { .. } | TypeNode::Derived { .. } | TypeNode::Projection { .. }
    ) {
        return None;
    }
    let ty = types.projection(scratch, base, step)?;
    let bound = crate::type_lattice::bound_above(types, scratch, ty);
    Some(Piece { ty, bound })
}

/// The `step` piece of `ty` through the projection door, the one closed rules read pieces by: a
/// list's element, a dict's key or value, a record's field, a union's the union of its members',
/// and a variable's projection where its bound guarantees the piece. `None` where it has none.
/// Substitution commutes with the door, so it marks nothing. A variable whose bound does not
/// guarantee the piece has the derived name of the piece a binding reads ([`Rule::Piece`]), the
/// same at load and under every binding; outside a walk, where no binding is written in, none.
pub fn piece(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    ty: Parametric,
    step: Step,
) -> Option<Parametric> {
    let piece = types.projection(scratch, ty, step);
    if piece.is_some() || !is_variable(&types.node(ty)) {
        return piece;
    }
    ruled_one(
        marks,
        types,
        scratch,
        Rule::Piece(step),
        &[DeclaredType::Type(ty)],
    )
}

/// Whether `end` is a lexical variable, or a projection of one, whose declared bound lies under
/// `ceiling` in the order. It reads the declared bound, which no binding changes, and every binding
/// lies under it, so it never marks.
pub fn rigid_within(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    end: Parametric,
    ceiling: KType,
) -> bool {
    match types.node(end) {
        TypeNode::Lexical { bound, .. }
        | TypeNode::Derived { bound, .. }
        | TypeNode::Projection { bound, .. } => {
            crate::type_lattice::is_subtype_of(types, scratch, bound, ceiling)
        }
        _ => false,
    }
}

/// Whether `ty` is a variable whose declared bound meets `other` at `Never` alone, so that no
/// binding of it lies under `other`: a binding lies under the bound, or is a view's carrier, which
/// lies under nothing but `Any` and a union holding it. It reads the declared bound, which no
/// binding changes, so it never marks.
pub fn variable_outside(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    ty: Parametric,
    other: KType,
) -> bool {
    (types.node(ty).rigid_bound()).is_some_and(|bound| {
        crate::type_lattice::meet(types, scratch, bound, other) == KType::NEVER
    })
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
/// variable bounded by no signature, or one the witness binds to no signature, noted beside its
/// question; else its node, a variable's marked [`ModuleBinder`](OpenKind::ModuleBinder) where no
/// witness answers.
pub fn module_node<'n>(
    marks: &dyn Marks,
    types: &Chain<'n, '_, '_, '_>,
    ty: Parametric,
) -> Option<TypeNode<'n, Parametric>> {
    if !bare_variable(types, DeclaredType::Type(ty)) {
        return node_unless_variable(types, ty);
    }
    // A view's name, the type a `:Module` binder carries, or a signature over names, is a
    // signature at every binding.
    if is_signature_name(marks, types, ty) {
        return Some(own_variable(types, ty));
    }
    // Only a bound that is a signature names the schema a module binder's reads go through: a
    // variable bounded by any other is no module binder, whatever a binding gives it.
    let bound = own_variable(types, ty).rigid_bound();
    if !bound.is_some_and(|bound| is_module(types, bound)) {
        return None;
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

/// Whether `ty` is a signature or an application of one, as the view door takes: its head's
/// answer where it is closed or its head is no variable, which no binding changes; no for a
/// variable whose bound meets no signature; else the witness's answer, or `None`, marked
/// [`ModuleBinder`](OpenKind::ModuleBinder).
pub fn module_decided(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    ty: Parametric,
) -> Option<bool> {
    if let Some(closed) = types.concrete(ty) {
        return Some(is_module(types, closed));
    }
    if let Some(node) = node_unless_variable(types, ty) {
        return Some(matches!(
            node,
            TypeNode::Signature { .. } | TypeNode::SignatureApply { .. }
        ));
    }
    if variable_outside(types, scratch, ty, KType::EMPTY_SIGNATURE) {
        return Some(false);
    }
    if let Some(Answer::Bool(module)) = marks.witnessed(Question::Module { ty }) {
        return Some(module);
    }
    mark(
        marks,
        types,
        OpenKind::ModuleBinder,
        None,
        DeclaredType::Type(ty),
    );
    None
}

/// Whether `ty` is the type a `:Module` binder carries ([`DerivedTerm::Carried`]): a signature at
/// every binding, whose members a binding of the binder gives. Bookkeeping on the name, so it
/// never marks.
pub fn is_carried(marks: &dyn Marks, types: &Chain<'_, '_, '_, '_>, ty: Parametric) -> bool {
    matches!(types.node(ty), TypeNode::Derived { term, .. }
        if matches!(marks.term(term), Some(DerivedTerm::Carried { .. })))
}

/// Whether `ty` is a view's name ([`Rule::View`]): at every binding a signature pinning every head
/// parameter. Bookkeeping on the name, so it never marks.
pub fn is_view(marks: &dyn Marks, types: &Chain<'_, '_, '_, '_>, ty: Parametric) -> bool {
    matches!(types.node(ty), TypeNode::Derived { term, .. }
        if matches!(marks.term(term), Some(DerivedTerm::Output { rule: Rule::View, .. })))
}

/// Whether `ty` is a name that is a signature at every binding: a view's ([`is_view`]), the type
/// a `:Module` binder carries ([`is_carried`]), or a signature written over names
/// ([`DerivedTerm::Signature`]). Bookkeeping on the name, so it never marks.
pub fn is_signature_name(marks: &dyn Marks, types: &Chain<'_, '_, '_, '_>, ty: Parametric) -> bool {
    is_view(marks, types, ty)
        || is_carried(marks, types, ty)
        || matches!(types.node(ty), TypeNode::Derived { term, .. }
            if matches!(marks.term(term), Some(DerivedTerm::Signature { .. })))
}

/// The member `name` a module's own signature written over names declares, read off its
/// skeleton: the type the member is written at, which a binding closes alike. `None` where `ty`
/// is no such name; `Some(None)` where it declares no such member at any binding.
pub fn signed_member(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    ty: Parametric,
    name: crate::symbols::BinderSymbol,
) -> Option<Option<DeclaredType<Parametric>>> {
    let TypeNode::Derived { term, .. } = types.node(ty) else {
        return None;
    };
    match marks.term(term) {
        Some(DerivedTerm::Signature { signed, operands }) if !signed.declared => {
            Some(signed.member(operands, name))
        }
        _ => None,
    }
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

/// What a retype at `declared` re-stamps at, as an interval up to `declared`: its lower end the
/// closed rule's where `declared` is closed, `declared` itself where the retype is a point at every
/// binding, else the derived name for the lower end ([`Rule::Retyped`]), which a binding gives the
/// lower end a load with it declared retypes to. So the interval is the same at load and under
/// every binding, and the retype asks nothing.
pub fn retyped(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    declared: Parametric,
) -> Interval {
    // Outside a walk, the lower end every binding lies above.
    let undecided = crate::values::retyped_to(types, scratch, declared).lower;
    let operands = [DeclaredType::Type(declared)];
    let lower = ruled(
        marks,
        types,
        scratch,
        (Rule::Retyped, &operands),
        &[undecided],
    )
    .map_or(undecided, |outputs| outputs[0]);
    Interval {
        lower,
        upper: declared,
    }
}

/// Whether `lower` is the lower end [`retyped`] gives a retype to `declared` over names: the
/// retype rule's name, which every binding closes to what a retype to `declared` re-stamps at.
pub fn retyped_lower(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    lower: Parametric,
    declared: Parametric,
) -> bool {
    match types.node(lower) {
        TypeNode::Derived { term, .. } => matches!(
            marks.term(term),
            Some(DerivedTerm::Output {
                rule: Rule::Retyped,
                operands: [DeclaredType::Type(at)],
                ..
            }) if *at == declared
        ),
        _ => false,
    }
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

/// `typed`'s type where it is exact and closed: a fold's reading, which is the load's alone, so it
/// never marks.
pub fn exactly(types: &Chain<'_, '_, '_, '_>, typed: Interval) -> Option<KType> {
    (typed.lower == typed.upper)
        .then(|| static_of(types, typed.upper))
        .flatten()
}

/// Whether `interval` is one type: [`same`] over its ends.
pub fn exact(marks: &dyn Marks, types: &Chain<'_, '_, '_, '_>, interval: Interval) -> bool {
    same(marks, types, interval.lower, interval.upper)
}

/// Whether `ty` is the closed type `constant`: named where it is, or where it holds a variable
/// only below a head that is no variable — save `Never`, which a union of names may be; else the
/// witness's answer, or marked [`Unequal`](OpenKind::Unequal).
pub fn is(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    ty: impl Into<DeclaredType<Parametric>>,
    constant: KType,
) -> bool {
    let ty = ty.into();
    let never = constant == KType::NEVER;
    let constant = DeclaredType::Type(Parametric::from(constant));
    let is = ty == constant;
    let open = match never {
        true => types.concrete_declared(ty).is_none() && may_be_never(types, ty),
        false => bare_variable(types, ty),
    };
    if !is && open {
        if let Some(Answer::Bool(is)) = marks.witnessed(Question::Same { a: ty, b: constant }) {
            return is;
        }
        mark(marks, types, OpenKind::Unequal, Some(constant), ty);
    }
    is
}

/// Each type `rule` hands back over `operands`: [`handed`] of each output's term, a closed output,
/// one written over names, or the derived name standing for it. So a rule hands back the same types
/// at load and under every binding. Outside a walk, where no binding is written in, an output no
/// spelling gives is its own of `undecided`. `None` where the closed rule has no answer over
/// closed operands.
pub fn ruled<'x>(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'x>,
    (rule, operands): (Rule, &[DeclaredType<Parametric>]),
    undecided: &[Parametric],
) -> Option<&'x [Parametric]> {
    debug_assert_eq!(undecided.len(), usize::from(rule.outputs()));
    let mut outputs = BumpVec::with_capacity_in(usize::from(rule.outputs()), scratch);
    for output in 0..rule.outputs() {
        let term = DerivedTerm::Output {
            rule,
            operands,
            output,
        };
        let undecided = undecided[usize::from(output)];
        let name = &mut |term: DerivedTerm<'_>| Some(marks.derive(term).unwrap_or(undecided));
        outputs.push(handed(types, scratch, term, name)?);
    }
    Some(collect(scratch, outputs.iter().copied()))
}

/// The meet of `a` and `b` through [`Rule::Meet`]: closed where both are, `a` where they are one
/// type or `b` is `Any`, else the derived name of their meet, which a binding meets. `None` over
/// names outside a walk, where no binding is written in.
pub fn met(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    a: Parametric,
    b: Parametric,
) -> Option<Parametric> {
    let operands = [DeclaredType::Type(a), DeclaredType::Type(b)];
    let term = DerivedTerm::Output {
        rule: Rule::Meet,
        operands: &operands,
        output: 0,
    };
    handed(types, scratch, term, &mut |term: DerivedTerm<'_>| {
        marks.derive(term)
    })
}

/// Whether the partial `rule` has an answer over `operands` ([`Question::Defined`]), as every
/// binding or the witness answers; `None` where it is undecided, its operands marked
/// [`Spells`](OpenKind::Spells). Beside it, whether the answer is every binding's.
pub fn defined(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    rule: Rule,
    operands: &[DeclaredType<Parametric>],
) -> (Option<bool>, bool) {
    let operands = collect(scratch, operands.iter().copied());
    let question = Question::Defined { rule, operands };
    let rigid = rigid_answer(types, scratch, &question);
    match rigid.or_else(|| marks.witnessed(question)) {
        Some(answer) => (Some(answer == Answer::Bool(true)), rigid.is_some()),
        None => {
            for operand in operands {
                mark(marks, types, OpenKind::Spells, None, *operand);
            }
            (None, false)
        }
    }
}

/// The output of the partial `rule` over `operands`, one output: [`handed`] of its term, closed,
/// written over names, or the derived name standing for it. `None` outside a walk, where no
/// binding is written in, or where the closed rule has no answer over closed operands.
pub fn ruled_one(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    rule: Rule,
    operands: &[DeclaredType<Parametric>],
) -> Option<Parametric> {
    let operands = collect(scratch, operands.iter().copied());
    let term = DerivedTerm::Output {
        rule,
        operands,
        output: 0,
    };
    handed(types, scratch, term, &mut |term| marks.derive(term))
}

/// What the spelling over a head `rule` spells over `operands` ([`Rule::Member`],
/// [`Rule::Applied`] and the rest of [`crate::scope`]'s heads): the rule's closed output where every
/// operand is closed. Else whether it spells is [`defined`], refused (`None`) where every binding,
/// or the witness, answers no, and the type is the decided name on that answer, `Never` where it
/// does not spell and the rule's output where it does, so it is the same at load and under every
/// binding. `Some(None)` over names outside a walk, where no binding is written in.
pub fn spelled(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    rule: Rule,
    operands: &[DeclaredType<Parametric>],
) -> Option<Option<Parametric>> {
    defined_else(marks, types, scratch, (rule, operands), KType::NEVER.into())
}

/// [`spelled`] for a partial rule whose type where it has no answer is `otherwise`, not `Never`:
/// the read a site falls back to there, so the decided name closes as a fresh load's fallback.
pub fn defined_else(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    (rule, operands): (Rule, &[DeclaredType<Parametric>]),
    otherwise: Parametric,
) -> Option<Option<Parametric>> {
    let (spells, rigid) = defined(marks, types, scratch, rule, operands);
    if spells == Some(false) {
        return None;
    }
    let Some(output) = ruled_one(marks, types, scratch, rule, operands) else {
        return Some(None);
    };
    if rigid {
        return Some(Some(output));
    }
    let operands = collect(scratch, operands.iter().copied());
    let question = Question::Defined { rule, operands };
    let arms = [otherwise, output];
    Some(Some(decided(
        marks,
        types,
        scratch,
        (question, &arms),
        output,
    )))
}

/// The type on the answer `question` takes, `arms` the type on each answer, every one written over
/// names: [`handed`] of the [decided](DerivedTerm::Decided) term — the one arm where every arm is
/// it, the arm of the question's rigid answer, else the derived name typed on the arms. It never
/// reads the witness, so it hands back the same type at load and under every binding; a site asks
/// the witness apart for any discrete outcome the answer chooses. Outside a walk, where no binding
/// is written in, it hands back `undecided`.
pub fn decided(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    (question, arms): (Question<'_>, &[Parametric]),
    undecided: Parametric,
) -> Parametric {
    debug_assert_eq!(arms.len() as u32, question.answer_count());
    let term = DerivedTerm::Decided {
        decision: Decider::Answer(question),
        arms,
    };
    let name = &mut |term: DerivedTerm<'_>| Some(marks.derive(term).unwrap_or(undecided));
    handed(types, scratch, term, name).unwrap_or(undecided)
}

/// [`decided`] with each answer's type given by `arm`: only the rigid answer's where there is one,
/// so an arm no binding takes lays nothing.
pub fn decided_by(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    question: Question<'_>,
    arm: &mut dyn FnMut(Answer) -> Parametric,
    undecided: Parametric,
) -> Parametric {
    if let Some(answer) = rigid_answer(types, scratch, &question) {
        return arm(answer);
    }
    let arms = (0..question.answer_count()).map(|index| arm(question.nth_answer(index)));
    let arms = collect(scratch, arms);
    decided(marks, types, scratch, (question, arms), undecided)
}

/// The type `then` gives where every one of `questions`, each a *yes* or *no* question, answers
/// *yes*, else `otherwise`: decided names chained one question deep each, so it is the same at
/// load and under every binding.
pub fn when_all(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    questions: &[Question<'_>],
    then: &mut dyn FnMut() -> Parametric,
    otherwise: Parametric,
) -> Parametric {
    let [first, rest @ ..] = questions else {
        return then();
    };
    let mut on = |answer| match answer {
        Answer::Bool(true) => when_all(marks, types, scratch, rest, then, otherwise),
        _ => otherwise,
    };
    decided_by(marks, types, scratch, *first, &mut on, otherwise)
}

/// `result`, the type of a rule over values of the types `uppers`, where every one of them
/// arrives, and `Never` where one never does: each end the decided name over whether each is
/// `Never` ([`decided_by`]). So it is the same at load and under every binding, and a site that
/// guards on [`is`] `Never` reads the guard only to skip its refusals.
pub fn arrives(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    uppers: &[Parametric],
    result: Interval,
) -> Interval {
    // Outside a walk, the ends every binding lies within.
    Interval {
        lower: never_unless(
            marks,
            types,
            scratch,
            uppers,
            result.lower,
            KType::NEVER.into(),
        ),
        upper: never_unless(marks, types, scratch, uppers, result.upper, result.upper),
    }
}

/// `end` where none of `uppers` is `Never`, else `Never`, one question deep each; `undecided`
/// outside a walk. An end that is `Never` or the upper itself is `Never` with it, so asks nothing.
pub fn never_unless(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    uppers: &[Parametric],
    end: Parametric,
    undecided: Parametric,
) -> Parametric {
    let never = Parametric::from(KType::NEVER);
    let [first, rest @ ..] = uppers else {
        return end;
    };
    // A signature is `Never` at no binding, so asks nothing.
    if never_with(marks, types, end, *first) || is_signature_name(marks, types, *first) {
        return never_unless(marks, types, scratch, rest, end, undecided);
    }
    let question = Question::Same {
        a: DeclaredType::Type(*first),
        b: DeclaredType::Type(never),
    };
    let mut arm = |answer| match answer {
        Answer::Bool(true) => never,
        _ => never_unless(marks, types, scratch, rest, end, undecided),
    };
    decided_by(marks, types, scratch, question, &mut arm, undecided)
}

/// Whether `end` is `Never` at every binding that makes `upper` `Never`: `Never` itself, `upper`, a
/// projection rooted at it (a piece of `Never` is `Never`), a union of such, or a derived name whose
/// term is: a piece, a retype or a meet over one, a decided term every arm of which is, or one
/// over whether such a type is `Never` whose *yes* arm is.
fn never_with(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    end: Parametric,
    upper: Parametric,
) -> bool {
    if end == upper || end == Parametric::from(KType::NEVER) {
        return true;
    }
    let operand = |operand: &DeclaredType<Parametric>| match operand {
        DeclaredType::Type(operand) => never_with(marks, types, *operand, upper),
        DeclaredType::Scheme(_) => false,
    };
    match types.node(end) {
        TypeNode::Projection { base, .. } => never_with(marks, types, base, upper),
        TypeNode::Union { members } => {
            (members.iter()).all(|member| never_with(marks, types, member, upper))
        }
        TypeNode::Derived { term, .. } => match marks.term(term) {
            Some(DerivedTerm::Output {
                rule: Rule::Piece(_) | Rule::Retyped,
                operands: [only],
                ..
            }) => operand(only),
            Some(DerivedTerm::Output {
                rule: Rule::Meet,
                operands,
                ..
            }) => operands.iter().any(operand),
            // A question whether a type `Never` with `upper` is `Never` answers *yes*.
            Some(DerivedTerm::Decided {
                decision: Decider::Answer(question @ Question::Same { a, b }),
                arms,
            }) if b == DeclaredType::Type(KType::NEVER.into()) && operand(&a) => {
                let yes = (0..question.answer_count())
                    .position(|index| question.nth_answer(index) == Answer::Bool(true));
                yes.is_some_and(|yes| never_with(marks, types, arms[yes], upper))
            }
            Some(DerivedTerm::Decided { arms, .. }) => {
                (arms.iter()).all(|arm| never_with(marks, types, *arm, upper))
            }
            _ => false,
        },
        _ => false,
    }
}

/// The signature the skeleton `signed` seals at `operands`, each written over names: closed where
/// every operand is, else the derived name whose term a binding seals as a fresh load does.
/// `None` outside a walk, where no binding is written in.
pub fn signature(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    signed: &crate::scope::Signed<'_>,
    operands: &[DeclaredType<Parametric>],
) -> Option<Parametric> {
    let term = DerivedTerm::Signature { signed, operands };
    handed(types, scratch, term, &mut |term| marks.derive(term))
}

/// Whether a binding decides `ty`'s head: a variable, or a union holding one. It reads the type's
/// shape alone, so it never marks.
pub fn varies(types: &Chain<'_, '_, '_, '_>, ty: Parametric) -> bool {
    kind_varies(types, ty)
}

/// The type an arrival guard ([`never_unless`]) is where its operand arrives, read through each
/// guard it holds: `ty` itself for any other type. A rule reading off a guard reads off this arm
/// and guards its own result again on the operand ([`arrives`]); a structural read of the term
/// the guard's question is over, so it never marks.
pub fn arriving(marks: &dyn Marks, types: &Chain<'_, '_, '_, '_>, ty: Parametric) -> Parametric {
    let TypeNode::Derived { term, .. } = types.node(ty) else {
        return ty;
    };
    let Some(DerivedTerm::Decided {
        decision: Decider::Answer(question @ Question::Same { b, .. }),
        arms,
    }) = marks.term(term)
    else {
        return ty;
    };
    let at = |answer| {
        (0..question.answer_count()).position(|index| question.nth_answer(index) == answer)
    };
    match (at(Answer::Bool(true)), at(Answer::Bool(false))) {
        (Some(yes), Some(no))
            if b == DeclaredType::Type(KType::NEVER.into())
                && arms[yes] == Parametric::from(KType::NEVER) =>
        {
            arriving(marks, types, arms[no])
        }
        _ => ty,
    }
}

/// What names a type holds, which says how a spelling over it is typed at load.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Names {
    /// None: the spelling is closed.
    Closed,
    /// Only lexical variables and projections, each a run-bound name, a derived name or a piece
    /// of one a binding gives a type: a spelling over them is a derived name.
    RunBound,
    /// A path's variable too, which a binding of its module closes.
    Path,
    /// A `FOR ALL` variable or a head parameter, which no binding gives a type at load.
    Unbound,
}

/// What names `ty` holds ([`Names`]). It reads each variable for what it is alone, so it never
/// marks.
pub fn names_held(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    ty: Parametric,
) -> Names {
    if types.concrete(ty).is_some() {
        return Names::Closed;
    }
    let mut held = Names::RunBound;
    read_through(types, scratch, ty, Side::Above, &mut |variable| {
        match variable {
            Variable::Lexical { .. } | Variable::Derived { .. } | Variable::Projection { .. } => {}
            Variable::Path { .. } if held == Names::RunBound => held = Names::Path,
            Variable::Path { .. } => {}
            Variable::Quantified { .. } | Variable::Parameter { .. } => held = Names::Unbound,
        }
        None
    });
    held
}
