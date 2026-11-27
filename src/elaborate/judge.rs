//! The **judgement doors**: every judgement the walk and the elaborator make over a type that may
//! hold a run-bound name goes through one of these, which marks
//! ([stored questions](../build/README.md#stored-questions)).
//!
//! A door hands back what the lattice answers and marks the run-bound names of what it judged
//! **open** ([`Marks::open`]) unless its answer holds at every binding within their ends: a verdict
//! of *always* or *never* (the rigid rule), a *fits* that holds, a type that is closed. What a unit
//! only names stays named; what it judged open it poses whole, as a question a binding answers. A
//! door whose inputs are closed never marks. Marking is by default: a rule that skips a door is a
//! test failure (`the_walk_judges_through_its_doors`), never a judgement left unasked.
//!
//! A **discrete** door — one handing back a verdict, a bool, a stamp or a kind — answers rigidly
//! by [`rigid_answer`], the one rule per question kind the binding graph answers by too, so the
//! load and an instance cannot drift.
//!
//! Every discrete door **poses** its question whole before it marks ([`Marks::asked`]), beside its
//! rigid answer where it has one, wherever a type the question judges holds a variable: a walk
//! lays each in its unit's record, in order, the asks a stored site question decides from
//! ([`SiteQuestion`](crate::scope::SiteQuestion)).
//!
//! A type that depends on a question's answer goes through [`decided`], and a rule with no question
//! through [`ruled`]: each hands back the same type at load and under every binding — a type
//! written over names, or a derived name typed on the question's answers or by the rule — and
//! ([derived names](crate::scope::DerivedTerm)).
//!
//! Substitution and the constructors commute with a binding, so they are not doors; nor is any
//! judgement whose operands are [`KType`] alone, which holds no variable by its type.

use crate::memory::collect;
use crate::memory::{BumpAllocator, BumpVec};
pub use crate::scope::free_variables;
use crate::scope::{
    Answer, Decider, DerivedId, DerivedTerm, NeverSatisfied, Nominals, OpenKind, Question,
    Rebounds, Restamp, Rule, Shaped, Showing, Signed, Unwalked, bare_variable, constructed_upper,
    handed, is_module, is_variable, kind_varies, rigid_verdict, tag_union,
};
pub use crate::scope::{rigid_answer, rigid_within, variable_outside};
use crate::symbols::{BinderSymbol, Symbol, TypeSymbol, ValueSymbol};
use crate::type_lattice::{
    Collector, DeclaredType, Interval, Judged, KType, Parametric, PathClass, Scheme, Side,
    Solution, SolveRecord, Step, TypeNode, Variable, Variance, Verdict, admits_with,
    instance_under, join, read_through, select_by_class,
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

    /// Note `question` whole as a door judged it, beside `rigid`, its answer at every binding where
    /// the door has one: a walk lays each in its unit's record, in order. A door notes only a
    /// question over a type that holds a variable; one over closed types is no question.
    fn asked(&self, question: Question<'_>, rigid: Option<Answer>) {
        let _ = (question, rigid);
    }

    /// The term the derived name `id` names, where the walk's store holds it: `None` outside a
    /// walk.
    fn term(&self, id: DerivedId) -> Option<DerivedTerm<'_>> {
        let _ = id;
        None
    }

    /// Whether the lexical variable at `level` is a type binder the text declares and the load
    /// left unknown, which no bound describes: `false` outside a walk, which reads no shapes.
    fn declares_type(&self, level: usize) -> bool {
        let _ = level;
        false
    }
}

/// Where a door names what it hands back over names: marks that also name each
/// [derived name](crate::scope::DerivedName) by its content. A walk's naming is total, its error
/// [`Infallible`](std::convert::Infallible), so a door in a walk hands back a type over every name
/// a binding writes in; outside a walk no binding is written in, and the error is the
/// [`Unwalked`] witness that leaves the type unknown.
pub trait Derives: Marks {
    /// Why a term goes unnamed: never, in a walk.
    type Unwalked;

    /// The derived name for `term`, laid where the walk's store keeps the names it meets.
    fn derive(&self, term: DerivedTerm<'_>) -> Result<Parametric, Self::Unwalked>;
}

/// The marks of a judgement outside a walk: none is kept, and no term is named.
pub struct NoMarks;

impl Marks for NoMarks {
    fn open(&self, _: OpenKind, _: Option<DeclaredType<Parametric>>, _: DeclaredType<Parametric>) {}
}

impl Derives for NoMarks {
    type Unwalked = Unwalked;

    fn derive(&self, _: DerivedTerm<'_>) -> Result<Parametric, Unwalked> {
        Err(Unwalked::outside_a_walk())
    }
}

/// Whether `ty` may hold a variable: a scheme is read by the marks, which find what it holds free.
fn holds_variable(types: &Chain<'_, '_, '_, '_>, ty: DeclaredType<Parametric>) -> bool {
    match ty {
        DeclaredType::Type(ty) => types.concrete(ty).is_none(),
        DeclaredType::Scheme(_) => true,
    }
}

/// [`rigid_answer`] of `question`, each derived name's term read off the marks: the one rule per
/// question kind every door and the binding graph answer rigidly by.
pub fn rigid<M: Marks + ?Sized>(
    marks: &M,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    question: &Question<'_>,
) -> Option<Answer> {
    rigid_answer(types, scratch, &|id| marks.term(id), question)
}

/// Ask `question` at a door: its [`rigid`] answer, noted beside it ([`posed`]); then that.
fn ask(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    question: Question<'_>,
) -> Option<Answer> {
    let rigid = rigid(marks, types, scratch, &question);
    posed(marks, types, question, rigid)
}

/// `question`, its rigid answer `rigid`, noted whole where a type it judges holds a variable
/// ([`Marks::asked`]); then `rigid`.
fn posed(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    question: Question<'_>,
    rigid: Option<Answer>,
) -> Option<Answer> {
    let mut open = false;
    question.each_type(&mut |ty| open = open || holds_variable(types, ty));
    if open {
        marks.asked(question, rigid);
    }
    rigid
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

/// Mark `kind` over `judged`'s type and each bound its group writes over names.
fn mark_shaped(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    kind: OpenKind,
    slot: Option<DeclaredType<Parametric>>,
    judged: Shaped<'_>,
) {
    mark(marks, types, kind, slot, judged.ty);
    mark_bounds(marks, types, kind, judged);
}

/// Mark `kind` over each bound `judged`'s group writes over names.
fn mark_bounds(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    kind: OpenKind,
    judged: Shaped<'_>,
) {
    for (_, bound) in judged.rebounds.0 {
        mark(marks, types, kind, None, (*bound).into());
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
/// binding within the arguments' ends; else each argument, and the shape, marked
/// [`Verdict`](OpenKind::Verdict). The intervals are [`judged_intervals`]'s, the same at load and
/// under every binding. A *never* hands back none.
pub fn verdict<'x, U>(
    marks: &dyn Derives<Unwalked = U>,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'x>,
    shape: Shaped<'_>,
    arguments: &[Interval],
) -> Judged<'x> {
    let judged = crate::scope::judge_signed(types, scratch, shape.ty, arguments);
    judged_verdict(marks, types, scratch, (shape, arguments), judged)
}

/// [`judge_signed`](crate::scope::judge_signed) alone: nothing marked. *Always* and *never* hold at every
/// binding within the arguments' ends; a caller reading it judges through [`judged_verdict`]
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
/// reads the lattice's own judgement too, which every binding gives alike. Over bounds a binding
/// writes into the shape, the judgement is at their upper ends, so only its *never* holds at every
/// binding.
pub fn judged_verdict<'x, U>(
    marks: &dyn Derives<Unwalked = U>,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'x>,
    (shape, arguments): (Shaped<'_>, &[Interval]),
    judged: Judged<'x>,
) -> Judged<'x> {
    let mut verdict = judged.verdict;
    let question = Question::Verdict { shape, arguments };
    let rigid = rigid_verdict(types, &question, verdict);
    if rigid.is_none() {
        verdict = Verdict::Maybe;
    }
    let answered = posed(marks, types, question, rigid);
    if verdict == Verdict::Maybe {
        match answered {
            Some(Answer::Verdict(rigid)) => verdict = rigid,
            _ => {
                mark_shaped(marks, types, OpenKind::Verdict, None, shape);
                for argument in arguments {
                    mark_interval(marks, types, OpenKind::Verdict, Some(shape.ty), *argument);
                }
            }
        }
    }
    let solved = match verdict {
        Verdict::Never => None,
        _ => judged_intervals(marks, types, scratch, (shape.ty, arguments), judged).map(
            |intervals| {
                let types = scratch.alloc_slice_fill_iter(intervals.iter().map(|each| each.upper));
                Solution { types, intervals }
            },
        ),
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
pub fn judged_intervals<'x, U>(
    marks: &dyn Derives<Unwalked = U>,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'x>,
    (shape, arguments): (DeclaredType<Parametric>, &[Interval]),
    judged: Judged<'x>,
) -> Option<&'x [Interval]> {
    let bounds = match shape {
        DeclaredType::Scheme(scheme) => scheme.group(types).bounds,
        DeclaredType::Type(_) => &[],
    };
    if judged.solve.reproducible || bounds.is_empty() {
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
/// changes, save where `ty` is a variable or a union holding one, whose binding decides it: marked
/// [`Kind`](OpenKind::Kind).
pub fn kind_above(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    ty: Parametric,
) -> crate::type_lattice::KKind {
    match kind_decided(marks, types, scratch, ty) {
        Some(kind) => kind,
        None => crate::type_lattice::bound_above(types, scratch, ty).kind_of(types),
    }
}

/// The kind of the concrete type above every instance of `ty`, where a rule acts on it exactly:
/// its head's, which no binding changes; `None` for a variable or a union holding one, marked
/// [`Kind`](OpenKind::Kind), whose binding decides it.
pub fn kind_decided(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    ty: Parametric,
) -> Option<crate::type_lattice::KKind> {
    if let Some(Answer::Kind(kind)) = ask(marks, types, scratch, Question::Kind { ty }) {
        return Some(kind);
    }
    mark(marks, types, OpenKind::Kind, None, DeclaredType::Type(ty));
    None
}

/// The type of a type value of `ty`: the kind its head fixes, or for a variable the decided name
/// typed on the kind each binding gives it, so a type name's own value is one type at load and
/// under every binding. Outside a walk, `Any` type.
pub fn kind_value<U>(
    marks: &dyn Derives<Unwalked = U>,
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
/// `wanted`'s bound too. Else named where *fits* holds rigidly at the upper end, or marked
/// [`NeverSatisfies`](OpenKind::NeverSatisfies).
pub fn never_satisfies(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    value: Interval,
    wanted: Parametric,
) -> Option<NeverSatisfied> {
    let refused = Some(NeverSatisfied { value, wanted });
    let question = Question::NeverSatisfies { value, wanted };
    match ask(marks, types, scratch, question) {
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
/// overlapping none at its bounds; else marked with
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
    match ask(marks, types, scratch, question) {
        Some(Answer::Overlaps(at)) => at.map(usize::from),
        _ => {
            mark(marks, types, question.kind(), None, registered.into());
            None
        }
    }
}

/// *Fits*: named where it holds, which it does at every binding within the ends; else marked
/// [`Fits`](OpenKind::Fits) against `b`.
pub fn fits(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    a: impl Into<DeclaredType<Parametric>>,
    b: impl Into<DeclaredType<Parametric>>,
) -> bool {
    fit_decided(marks, types, scratch, a, b) == Some(true)
}

/// *Fits*, where a rule acts on its failure: `Some` where it holds, where neither type holds a
/// variable; else `None`, marked [`Fits`](OpenKind::Fits), since a
/// binding decides it. A rule refuses only on `Some(false)`, which holds at every binding.
pub fn fit_decided(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    a: impl Into<DeclaredType<Parametric>>,
    b: impl Into<DeclaredType<Parametric>>,
) -> Option<bool> {
    let (a, b) = (a.into(), b.into());
    if let Some(Answer::Bool(holds)) = ask(marks, types, scratch, Question::Fits { a: a.into(), b })
    {
        return Some(holds);
    }
    mark(marks, types, OpenKind::Fits, Some(b), a);
    None
}

/// Whether `a` lies under `b` in the order, where a rule acts on either answer: `Some` where both
/// are closed, where `a` is a variable whose declared bound lies under a closed `b` — every
/// binding of it does too; else `None`, marked
/// [`Under`](OpenKind::Under), since the order relates closed types alone and a binding decides.
pub fn under_decided(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    a: Parametric,
    b: Parametric,
) -> Option<bool> {
    if let Some(Answer::Bool(under)) = ask(marks, types, scratch, Question::Under { a, b }) {
        return Some(under);
    }
    mark(marks, types, OpenKind::Under, Some(b.into()), a.into());
    None
}

/// Whether the registered shape `a` strictly outranks `b` at `class`, one bit of a use's ranking
/// table: `Some` where both are closed; else `None`, marked with
/// [`Question::Outranks`]'s kind, since a binding ranks a rigid shape.
pub fn outranks(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    (a, b): (Shaped<'_>, Shaped<'_>),
    class: u16,
) -> Option<bool> {
    let question = Question::Outranks { a, b, class };
    if let Some(Answer::Bool(ranked)) = ask(marks, types, scratch, question) {
        return Some(ranked);
    }
    mark_shaped(marks, types, question.kind(), Some(b.ty), a);
    mark_bounds(marks, types, question.kind(), b);
    None
}

/// Whether `value`'s lower end lies outside `bound`: named where it does; else marked
/// [`LowerEndOutside`](OpenKind::LowerEndOutside).
pub fn lower_end_outside(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    value: Interval,
    bound: impl Into<DeclaredType<Parametric>>,
) -> bool {
    let bound = bound.into();
    let question = Question::LowerEndOutside { value, bound };
    if let Some(Answer::Bool(outside)) = ask(marks, types, scratch, question) {
        return outside;
    }
    mark_interval(marks, types, OpenKind::LowerEndOutside, Some(bound), value);
    false
}

/// [`class_step`](crate::type_lattice::class_step): a solve record, whose
/// [`reproducible`](SolveRecord::reproducible) says whether a run solves it alike. A caller stores
/// one it does not whole ([`stored_solve`]).
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

/// A solve the load cannot write over the names it reads, stored whole as [`Rule::Solved`]: the
/// callee's `scheme`, its group pinned as `pinned`, solved by one class step over `carried` — each
/// involved slot's position, in slot order, beside the type it contributes — over exact
/// contributions where `exact` says so. Whether it solves is [`Question::Defined`]: `None` where
/// every binding answers no. Else each variable's solution, as [`ruled`] hands it
/// back: closed, or the derived name the stored solve types, so a run solves it under its own
/// binding. Asked where the step is not one a run reproduces. `carried` and the group fit a
/// stored solve's bits, since the load refuses a quantified type too wide for them
/// ([`TooWide`](crate::scope::Elaboration::TooWide)).
pub fn stored_solve<'x, U>(
    marks: &dyn Derives<Unwalked = U>,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'x>,
    scheme: Scheme,
    pinned: &[Option<Parametric>],
    carried: &[(usize, Parametric)],
    exact: bool,
) -> Option<&'x [Parametric]> {
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
    match ask(marks, types, scratch, question) {
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

/// What `(head payload)` builds over a static payload type: a newtype's head where its payload
/// fits its representation, which the closed rule's own satisfaction is, or the
/// [`fits`](Question::Fits) question a binding decides it by; and a family's solve, [`admits_with`]
/// and the collector's solve, applied where the load writes it over names. Where a run may solve
/// the family otherwise, the operands of [`Rule::Constructed`] instead, whose closed rule the
/// end's type is. It judges nothing a caller does not name, so it never marks.
fn built<'x>(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'x>,
    (head, payload): (KType, Parametric),
) -> Result<Built<'x>, crate::values::ConstructionRefused<Parametric>> {
    use crate::type_lattice::NodeSchema;
    use crate::values::ConstructionRefused;
    match types.member_schema(head) {
        Some(NodeSchema::NewType(representation)) => {
            let misfit = ConstructionRefused::Misfit {
                identity: head,
                representation,
                payload,
            };
            let (a, b) = (DeclaredType::Type(payload), representation.into());
            if crate::type_lattice::fits(types, scratch, a, b) {
                return Ok(Built::Typed(head.into()));
            }
            match holds_variable(types, a) || holds_variable(types, b) {
                true => Ok(Built::Fitting(Question::Fits { a: a.into(), b })),
                false => Err(misfit),
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
            admits_with(
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
            let solved = collector.solve(types).map_err(|_| refused)?;
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

/// [`built`]'s answer: the type built; the operands of a construction a run may solve otherwise;
/// or a newtype's head where a binding decides its payload's fit, the question it asks.
enum Built<'x> {
    Typed(Parametric),
    Unreproduced(&'x [DeclaredType<Parametric>]),
    Fitting(Question<'x>),
}

/// The lower end of `(head payload)` over a static payload type ([`built`]). Where a run may solve
/// the family otherwise, the type on whether it solves ([`Question::Defined`] over
/// [`Rule::Constructed`]): the rule's output where it does, `Never` where it does not; where a
/// binding decides a newtype payload's fit, the head on whether it fits. So it is the same at load
/// and under every binding.
pub fn construction<U>(
    marks: &dyn Derives<Unwalked = U>,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    (head, payload): (KType, Parametric),
) -> Result<Parametric, crate::values::ConstructionRefused<Parametric>> {
    let refused = crate::values::ConstructionRefused::Unsolved {
        family: head,
        payload,
    };
    let never = Parametric::from(KType::NEVER);
    let operands = match built(types, scratch, (head, payload))? {
        Built::Typed(built) => return Ok(built),
        Built::Unreproduced(operands) => operands,
        Built::Fitting(question) => {
            let arms = [never, head.into()];
            return Ok(decided(marks, types, scratch, (question, &arms), never));
        }
    };
    let rule = Rule::Constructed;
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
pub fn construction_upper<U>(
    marks: &dyn Derives<Unwalked = U>,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    (head, payload): (KType, Parametric),
    owner: Option<KType>,
) -> Parametric {
    let through = owner.and_then(|owner| tag_union(types, scratch, owner));
    let built = match built(types, scratch, (head, payload)) {
        Ok(Built::Typed(built)) => Ok(built),
        // A misfit faults, so the upper end is the head whether or not the payload fits.
        Ok(Built::Fitting(_)) => Ok(head.into()),
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

/// The value member `typed` reads off a module the load cannot see, beside the type the module
/// carries: the derived name a member read is typed by ([`Rule::SignatureValue`]), or the one
/// decided on whether the module declares it, each read through its arrival guard. `None` for any
/// other type.
pub fn unseen_member(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    typed: Parametric,
) -> Option<(ValueSymbol, Parametric)> {
    let typed = arriving(marks, types, typed);
    let TypeNode::Derived { term, .. } = types.node(typed) else {
        return None;
    };
    let read = |rule, operands: &[DeclaredType<Parametric>]| match (rule, operands) {
        (Rule::SignatureValue(name), [DeclaredType::Type(carried)]) => Some((name, *carried)),
        _ => None,
    };
    match marks.term(term)? {
        DerivedTerm::Output { rule, operands, .. } => read(rule, operands),
        DerivedTerm::Decided {
            decision: Decider::Answer(Question::Defined { rule, operands }),
            ..
        } => read(rule, operands),
        _ => None,
    }
}

/// The instance of the value member `name` of a module the load cannot see, whose module carries
/// `carried`, read at an instance site wanted at `wanted` ([`Rule::MemberInstance`]): the question
/// whether it has one there, beside its rigid answer, and the instance's type — `None` where the
/// answer is no.
pub fn member_instance<'l, U>(
    marks: &dyn Derives<Unwalked = U>,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'l>,
    (name, carried): (ValueSymbol, Parametric),
    wanted: Option<Parametric>,
) -> (Question<'l>, Option<Answer>, Option<Parametric>) {
    let rule = Rule::MemberInstance {
        name,
        wanted: wanted.is_some(),
    };
    let mut operands = BumpVec::with_capacity_in(2, scratch);
    operands.push(DeclaredType::Type(carried));
    operands.extend(wanted.map(DeclaredType::Type));
    let operands = operands.leak();
    let question = Question::Defined { rule, operands };
    let rigid = ask(marks, types, scratch, question);
    if rigid == Some(Answer::Bool(false)) {
        return (question, rigid, None);
    }
    let instance = ruled_one(marks, types, scratch, rule, operands)
        .ok()
        .flatten();
    (question, rigid, instance)
}

/// [`Rule::InstanceAt`] over `scheme`, beside the bounds `rebounds` its group writes over names,
/// wanted at `wanted`: the rule beside its operands, the bounds after the scheme and the wanted
/// type by group position.
pub fn instance_at<'l>(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'l>,
    (scheme, rebounds): (Scheme, Rebounds<'_>),
    wanted: Parametric,
) -> (Rule, &'l [DeclaredType<Parametric>]) {
    let mut bounded = BumpVec::with_capacity_in(rebounds.0.len(), scratch);
    bounded.extend(rebounds.0.iter().copied());
    bounded.sort_unstable_by_key(|(at, _)| *at);
    let mut operands = BumpVec::with_capacity_in(2 + bounded.len(), scratch);
    operands.extend([DeclaredType::Scheme(scheme), DeclaredType::Type(wanted)]);
    operands.extend(bounded.iter().map(|(_, bound)| DeclaredType::Type(*bound)));
    let rule = Rule::InstanceAt {
        variables: scheme.group(types).bounds.len() as u8,
        bounded: (bounded.iter()).fold(0, |mask, (at, _)| mask | 1 << at),
    };
    (rule, operands.leak())
}

/// [`instance_under`]: named where its solve record is reproducible, so a binding written in gives
/// the run's. Where it is not, the instance is fixed at each variable's output of
/// [`Rule::InstanceAt`], which a binding instantiates again, whether or not the load's solve
/// succeeded: whether one is fixed is [`Question::Defined`], and a no at every binding is no
/// solve. Undecided, its types are marked with the question's kind.
pub fn instance<'l, 'run: 'l, U>(
    marks: &dyn Derives<Unwalked = U>,
    types: &Chain<'run, '_, '_, '_>,
    scratch: BumpAllocator<'l>,
    (scheme, rebounds): (Scheme, Rebounds<'_>),
    wanted: Parametric,
) -> SolveRecord<'l> {
    let record = instance_under(types, scratch, scheme, wanted);
    let bounds = scheme.group(types).bounds;
    // Over bounds a binding writes in, the solve at their upper ends is no binding's own.
    if record.reproducible && rebounds.is_empty() {
        return record;
    }
    let record = SolveRecord {
        reproducible: false,
        ..record
    };
    let (rule, operands) = instance_at(types, scratch, (scheme, rebounds), wanted);
    let unsolved = SolveRecord {
        solved: None,
        ..record
    };
    let question = Question::Defined { rule, operands };
    match ask(marks, types, scratch, question) {
        Some(Answer::Bool(false)) => return unsolved,
        Some(_) => {}
        None => {
            for operand in operands {
                mark(marks, types, question.kind(), Some(wanted.into()), *operand);
            }
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
/// at every binding alike; else marked with [`Question::IsFunction`]'s
/// kind and taken as none.
pub fn is_function(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    ty: Parametric,
) -> bool {
    let question = Question::IsFunction { ty };
    match ask(marks, types, scratch, question) {
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
pub fn piece<U>(
    marks: &dyn Derives<Unwalked = U>,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    ty: Parametric,
    step: Step,
) -> Option<Parametric> {
    let piece = types.projection(scratch, ty, step);
    if piece.is_some() || !is_variable(&types.node(ty)) {
        return piece;
    }
    let operand = [DeclaredType::Type(ty)];
    (ruled_one(marks, types, scratch, Rule::Piece(step), &operand).ok()).flatten()
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
/// variable bounded by no signature, noted beside its question; else its node, a variable's
/// marked [`ModuleBinder`](OpenKind::ModuleBinder).
pub fn module_node<'n>(
    marks: &dyn Marks,
    types: &Chain<'n, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    ty: Parametric,
) -> Option<TypeNode<'n, Parametric>> {
    if !bare_variable(types, DeclaredType::Type(ty)) {
        return node_unless_variable(types, ty);
    }
    // Only a bound that is a signature names the schema a module binder's reads go through: a
    // variable bounded by any other is no module binder, whatever a binding gives it, save a name
    // that is a signature at every binding.
    let bound = own_variable(types, ty).rigid_bound();
    if !is_signature_name(marks, types, ty) && !bound.is_some_and(|bound| is_module(types, bound)) {
        return None;
    }
    match ask(marks, types, scratch, Question::Module { ty }) {
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
/// variable whose bound meets no signature; else `None`, marked
/// [`ModuleBinder`](OpenKind::ModuleBinder).
pub fn module_decided(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    ty: Parametric,
) -> Option<bool> {
    if let Some(Answer::Bool(module)) = ask(marks, types, scratch, Question::Module { ty }) {
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
        if marks.term(term).is_some_and(|term| term.carries_a_module()))
}

/// `typed` as a solve reads it where every binder declared `Any` it names
/// ([`DerivedTerm::Carried`] under `Any`) were declared at no name: each such name written in at
/// its ends, `Never` into the lower end and `Any` into the upper, and each derived name over one
/// handed again. A name a question decides over one is read at its own ends, since the question
/// decides otherwise at each type between them. Declared `Any` is a spelling, so a solve over
/// such a name is what it is over `Any`. Bookkeeping on the names, so it never marks.
pub fn any_names_at_their_ends<U>(
    marks: &dyn Derives<Unwalked = U>,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    typed: Interval,
) -> Interval {
    let at = |ty: Parametric, end: KType| {
        let binds = AnyAt {
            marks,
            scratch,
            end,
        };
        match crate::scope::rebound(types, scratch, DeclaredType::Type(ty), &binds) {
            crate::scope::Closed::Type(DeclaredType::Type(closed)) => closed.into(),
            crate::scope::Closed::Partial(DeclaredType::Type(partial)) => partial,
            _ => ty,
        }
    };
    Interval {
        lower: at(typed.lower, KType::NEVER),
        upper: at(typed.upper, KType::ANY),
    }
}

/// A binding writing `end` in for each name of a binder declared `Any`, and the same end of each
/// name a question decides over one, and nothing else: the walk's names and their terms
/// otherwise.
struct AnyAt<'m, 'x, U> {
    marks: &'m dyn Derives<Unwalked = U>,
    scratch: BumpAllocator<'x>,
    end: KType,
}

impl<U> crate::scope::Binds for AnyAt<'_, '_, U> {
    fn level(&self, _: &Chain<'_, '_, '_, '_>, _: usize) -> Option<KType> {
        None
    }

    fn term(&self, id: DerivedId) -> Option<DerivedTerm<'_>> {
        self.marks.term(id)
    }

    fn name(
        &self,
        _: &Chain<'_, '_, '_, '_>,
        _: BumpAllocator<'_>,
        term: DerivedTerm<'_>,
    ) -> Option<Parametric> {
        self.marks.derive(term).ok()
    }

    fn written(&self, types: &Chain<'_, '_, '_, '_>, variable: Variable) -> Option<Parametric> {
        let Variable::Derived { term, .. } = variable else {
            return None;
        };
        match self.marks.term(term)? {
            DerivedTerm::Carried { bound, .. } if bound == Parametric::from(KType::ANY) => {
                Some(self.end.into())
            }
            // The decided name's lower end is `Never`, and its upper the join of its arms where
            // each is closed, else `Any`: no type between the name's ends picks one arm.
            decided @ DerivedTerm::Decided { .. } => {
                let (written, changed) = decided.substituted(types, self.scratch, self);
                if !changed {
                    return None;
                }
                let DerivedTerm::Decided { arms, .. } = written else {
                    unreachable!("a decided term stays decided")
                };
                let upper = || {
                    (arms.iter()).try_fold(KType::NEVER, |joined, arm| {
                        Some(join(types, self.scratch, joined, types.concrete(*arm)?))
                    })
                };
                Some(match self.end == KType::NEVER {
                    true => KType::NEVER.into(),
                    false => upper().unwrap_or(KType::ANY).into(),
                })
            }
            _ => None,
        }
    }
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
    crate::scope::signature_name(types, &|id| marks.term(id), ty)
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

/// What a type declared over run-bound names shows of itself in its declaration's text, the same
/// at every binding ([`declared`]).
#[derive(Clone, Copy)]
pub enum Declared<'a> {
    /// A `NEWTYPE`, a `UNION` or one of its variants, or a family, of the group `group`.
    Nominal {
        group: &'a Nominals<'a>,
        shown: Showing<'a>,
    },
    /// A `SIG`.
    Signature(Signed<'a>),
}

impl Declared<'_> {
    /// The names of a declared union's variants, in declared order: `None` for any other
    /// declaration.
    pub fn variants(&self) -> Option<impl Iterator<Item = Symbol> + '_> {
        match *self {
            Declared::Nominal {
                group,
                shown: Showing::Union { variants, .. },
            } => Some(variants.iter().map(|index| group.name(*index).symbol())),
            _ => None,
        }
    }

    /// Whether `Owner.name` over the declaration reads a member its text shows: a union's
    /// variant, or a field or a variant of a newtype's or a variant's representation.
    pub fn shows_member(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        scratch: BumpAllocator<'_>,
        name: BinderSymbol,
    ) -> bool {
        if let Some(mut variants) = self.variants() {
            return variants.any(|variant| variant == name.symbol());
        }
        match *self {
            Declared::Nominal {
                shown:
                    Showing::Member {
                        representation: Some(representation),
                        ..
                    },
                ..
            } => {
                types
                    .projection(scratch, representation, Step::Field(name))
                    .is_some()
                    || crate::scope::variant_named(types, representation, name.symbol()).is_some()
            }
            _ => false,
        }
    }

    /// The parameters a head spelling over the declaration names: a family's, or those every
    /// variant of a union takes. `None` for a declaration that takes none.
    pub fn params(&self) -> Option<&[TypeSymbol]> {
        match *self {
            Declared::Nominal {
                shown: Showing::Union { params, .. },
                ..
            } => params.filter(|params| !params.is_empty()),
            Declared::Nominal {
                shown: Showing::Member { params, .. },
                ..
            } => (!params.is_empty()).then_some(params),
            Declared::Signature(_) => None,
        }
    }
}

/// What `ty` shows where it is a type declared over run-bound names — a nominal or a signature,
/// whose derived name hands back the declaration a binding seals — read off its term: `None` for
/// any other type. A spelling over one is licensed by its declaration, never by a binding. It
/// reads the declaration, so it never marks.
pub fn declared<'m>(
    marks: &'m dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    ty: Parametric,
) -> Option<Declared<'m>> {
    let TypeNode::Derived { term, .. } = types.node(ty) else {
        return None;
    };
    match marks.term(term)? {
        DerivedTerm::Nominal {
            group,
            operands,
            output,
        } => (group.shown(output, operands)).map(|shown| Declared::Nominal { group, shown }),
        DerivedTerm::Signature { signed, .. } => Some(Declared::Signature(*signed)),
        _ => None,
    }
}

/// Whether the bound of `ty`, the static type of a value a field read `x.name` reads, licenses the
/// read at every binding: a field the bound guarantees, a member a signature bound declares, or a
/// member of a type value under a type bound. `None` where `ty` is no type over run-bound `FOR
/// ALL` names alone ([`over_for_all_names`]), whose read is judged as any other type's
/// ([what a body may ask of a `FOR ALL` name](../../design/quantified-types.md#what-a-body-may-ask-of-a-for-all-name)).
/// It reads the bound, never what a binding gives the name, so it never marks.
pub fn read_licensed(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    ty: Parametric,
    name: BinderSymbol,
) -> Option<bool> {
    if !over_for_all_names(marks, types, scratch, ty) {
        return None;
    }
    if types.projection(scratch, ty, Step::Field(name)).is_some() {
        return Some(true);
    }
    let bound = crate::type_lattice::bound_above(types, scratch, ty);
    Some(
        bound_declares(marks, types, scratch, ty, name)
            || crate::type_lattice::is_subtype_of(types, scratch, bound, KType::ANY_TYPE),
    )
}

/// Whether `ty` holds a run-bound `FOR ALL` name and every name it holds is one, a projection off
/// one, or a derived name over such names alone: a type whose bound licenses what a body asks of
/// it. A module's carried type, a view, a signature written over names and a path's variable are
/// given by a module the run builds, and a declaration the load left unknown is decided where it
/// is typed, so `false`. Bookkeeping on the names, so it never marks.
pub fn over_for_all_names(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    ty: Parametric,
) -> bool {
    if types.concrete(ty).is_some() {
        return false;
    }
    let mut alone = true;
    read_through(types, scratch, ty, Side::Above, &mut |variable| {
        alone &= match variable {
            // A declaration the load left unknown, past a quote's chain, is decided where it is
            // typed, not licensed by a bound.
            Variable::Lexical { level, .. } => !marks.declares_type(level),
            Variable::Projection { .. } => true,
            Variable::Derived { term, .. } => match marks.term(term) {
                Some(DerivedTerm::Output { rule, operands, .. }) if rule != Rule::View => {
                    (operands.iter()).all(|operand| match operand {
                        DeclaredType::Type(ty) => {
                            types.concrete(*ty).is_some()
                                || over_for_all_names(marks, types, scratch, *ty)
                        }
                        DeclaredType::Scheme(_) => false,
                    })
                }
                _ => false,
            },
            Variable::Path { .. } | Variable::Quantified { .. } | Variable::Parameter { .. } => {
                false
            }
        };
        None
    });
    alone
}

/// Whether every module of `ty`, a type a binding decides, declares the member `name`, by the
/// bound alone: an application declares what its signature does whatever its pins, a union where
/// each member does, a meet where one does, and a name where its bound does. It reads bounds,
/// which are closed, and derived names' terms, so it never marks.
pub fn bound_declares(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    ty: Parametric,
    name: BinderSymbol,
) -> bool {
    let declares = |ty: &DeclaredType<Parametric>| match ty {
        DeclaredType::Type(ty) => bound_declares(marks, types, scratch, *ty, name),
        DeclaredType::Scheme(_) => false,
    };
    match types.node(ty) {
        TypeNode::SignatureApply { signature, .. } => {
            super::members::applied_member(types, scratch, signature, &[], name).is_some()
        }
        TypeNode::Union { members } => {
            (members.iter()).all(|each| bound_declares(marks, types, scratch, each, name))
        }
        TypeNode::SignatureMeet { members } => {
            (members.iter()).any(|each| bound_declares(marks, types, scratch, each, name))
        }
        TypeNode::Derived { term, .. }
            if let Some(DerivedTerm::Output {
                rule: Rule::Meet,
                operands,
                ..
            }) = marks.term(term) =>
        {
            operands.iter().any(declares)
        }
        _ => {
            let bound = crate::type_lattice::bound_above(types, scratch, ty);
            super::members::signature_member(&NoMarks, types, scratch, bound.into(), name).is_some()
        }
    }
}

/// The bound a refusal names `ty` by, read from above: wording for a message, never a judgement of
/// what a binding gives it, so it never marks.
pub fn named_bound(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    ty: Parametric,
) -> KType {
    crate::type_lattice::bound_above(types, scratch, ty)
}

/// The level of `ty` where it is a run-bound name's own lexical variable: bookkeeping on the
/// variable, so it never marks.
pub fn lexical_level(types: &Chain<'_, '_, '_, '_>, ty: Parametric) -> Option<usize> {
    match own_variable(types, ty) {
        TypeNode::Lexical { level, .. } => Some(level),
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
pub fn retyped<U>(
    marks: &dyn Derives<Unwalked = U>,
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

/// How a retype to `declared` re-stamps, beside it: named where `declared` is closed; else marked
/// [`Retype`](OpenKind::Retype).
pub fn restamp(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    declared: Parametric,
) -> Option<Restamp> {
    let question = Question::Restamp { declared };
    if let Some(Answer::Restamps(how)) = ask(marks, types, scratch, question) {
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

/// The closed type above `ty` at every binding: each variable read at the end its position
/// needs, its bound where covariant and its lower end where contravariant. What a bound written
/// over an encloser's names is laid at; it reads bounds alone, so it never marks.
pub fn upper_end(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    ty: Parametric,
) -> KType {
    crate::type_lattice::bound_above(types, scratch, ty)
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

/// Whether `a` and `b` are one type: [`same_decided`], undecided taken as no.
pub fn same(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    a: impl Into<DeclaredType<Parametric>>,
    b: impl Into<DeclaredType<Parametric>>,
) -> bool {
    same_decided(marks, types, scratch, a, b) == Some(true)
}

/// Whether `a` and `b` are one type, where a rule acts on either answer: named where they are, or
/// where neither holds a variable; else `None`, marked
/// [`Unequal`](OpenKind::Unequal), since two non-variable heads over variables may come out one
/// under a binding.
pub fn same_decided(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    a: impl Into<DeclaredType<Parametric>>,
    b: impl Into<DeclaredType<Parametric>>,
) -> Option<bool> {
    let (a, b) = (a.into(), b.into());
    match ask(marks, types, scratch, Question::Same { a, b }) {
        Some(Answer::Bool(same)) => Some(same),
        _ => {
            mark(marks, types, OpenKind::Unequal, Some(b), a);
            None
        }
    }
}

/// `typed`'s type where it is exact and closed: a fold's reading, which is the load's alone, so it
/// never marks.
pub fn exactly(types: &Chain<'_, '_, '_, '_>, typed: Interval) -> Option<KType> {
    (typed.lower == typed.upper)
        .then(|| static_of(types, typed.upper))
        .flatten()
}

/// Whether `interval` is one type: [`same`] over its ends.
pub fn exact(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    interval: Interval,
) -> bool {
    same(marks, types, scratch, interval.lower, interval.upper)
}

/// Whether `interval` is one type, where a rule acts on either answer: [`same_decided`] over its
/// ends, beside the question it asks.
pub fn exact_decided<'x>(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    interval: Interval,
) -> (Question<'x>, Option<bool>) {
    let question = Question::Same {
        a: interval.lower.into(),
        b: interval.upper.into(),
    };
    (
        question,
        same_decided(marks, types, scratch, interval.lower, interval.upper),
    )
}

/// Whether `ty` is the closed type `constant`: [`is_decided`], undecided taken as no.
pub fn is(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    ty: impl Into<DeclaredType<Parametric>>,
    constant: KType,
) -> bool {
    is_decided(marks, types, scratch, ty, constant) == Some(true)
}

/// Whether `ty` is the closed type `constant`, where a rule acts on either answer: named where it
/// is, or where it holds a variable only below a head that is no variable — save `Never`, which a
/// union of names may be; else `None`, marked
/// [`Unequal`](OpenKind::Unequal).
pub fn is_decided(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    ty: impl Into<DeclaredType<Parametric>>,
    constant: KType,
) -> Option<bool> {
    let ty = ty.into();
    let constant = DeclaredType::Type(Parametric::from(constant));
    match ask(marks, types, scratch, Question::Same { a: ty, b: constant }) {
        Some(Answer::Bool(is)) => Some(is),
        _ => {
            mark(marks, types, OpenKind::Unequal, Some(constant), ty);
            None
        }
    }
}

/// The type on a yes-or-no `question`'s answer, `answered` as its door handed it back: `yes` or
/// `no` where the door answered, else the name [`decided`] types on both, so it is the same
/// at load and under every binding. Outside a walk an undecided question hands back `no`.
pub fn on_answer<U>(
    marks: &dyn Derives<Unwalked = U>,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    (question, answered): (Question<'_>, Option<bool>),
    (no, yes): (Parametric, Parametric),
) -> Parametric {
    match answered {
        Some(true) => yes,
        Some(false) => no,
        None => decided(marks, types, scratch, (question, &[no, yes]), no),
    }
}

/// Each type `rule` hands back over `operands`: [`handed`] of each output's term, a closed output,
/// one written over names, or the derived name standing for it. So a rule hands back the same types
/// at load and under every binding. Outside a walk, where no binding is written in, an output no
/// spelling gives is its own of `undecided`. `None` where the closed rule has no answer over
/// closed operands.
pub fn ruled<'x, U>(
    marks: &dyn Derives<Unwalked = U>,
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
        let name = &mut |term: DerivedTerm<'_>| Ok::<_, U>(marks.derive(term).unwrap_or(undecided));
        outputs.push(handed(types, scratch, term, name).ok().flatten()?);
    }
    Some(collect(scratch, outputs.iter().copied()))
}

/// The meet of `a` and `b` through [`Rule::Meet`]: closed where both are, `a` where they are one
/// type or `b` is `Any`, else the derived name of their meet, which a binding meets. `marks`'s
/// error over names outside a walk, where no binding is written in.
pub fn met<U>(
    marks: &dyn Derives<Unwalked = U>,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    a: Parametric,
    b: Parametric,
) -> Result<Parametric, U> {
    let operands = [DeclaredType::Type(a), DeclaredType::Type(b)];
    let term = DerivedTerm::Output {
        rule: Rule::Meet,
        operands: &operands,
        output: 0,
    };
    let met = handed(types, scratch, term, &mut |term: DerivedTerm<'_>| {
        marks.derive(term)
    })?;
    Ok(met.expect("two closed types meet"))
}

/// Whether the partial `rule` has an answer over `operands` ([`Question::Defined`]), as every
/// binding answers; `None` where it is undecided, its operands marked
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
    let rigid = rigid(marks, types, scratch, &question);
    match posed(marks, types, question, rigid) {
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
/// written over names, or the derived name standing for it. `None` where the closed rule has no
/// answer over closed operands; `marks`'s error outside a walk, where no binding is written in.
pub fn ruled_one<U>(
    marks: &dyn Derives<Unwalked = U>,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    rule: Rule,
    operands: &[DeclaredType<Parametric>],
) -> Result<Option<Parametric>, U> {
    let operands = collect(scratch, operands.iter().copied());
    let term = DerivedTerm::Output {
        rule,
        operands,
        output: 0,
    };
    handed(types, scratch, term, &mut |term| marks.derive(term))
}

/// Whether the partial `rule` spells over `operands`, and its type: the rule's closed output
/// where every operand is closed, else the decided name on [`defined`], `otherwise` where it does
/// not spell and the rule's output where it does, so it is the same at load and under every
/// binding; `None` where every binding answers no. `otherwise` is the read a site
/// falls back to where the rule has no answer, so the decided name closes as a fresh load's
/// fallback.
pub fn defined_else<U>(
    marks: &dyn Derives<Unwalked = U>,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    (rule, operands): (Rule, &[DeclaredType<Parametric>]),
    otherwise: Parametric,
) -> Option<Result<Parametric, U>> {
    let (spells, rigid) = defined(marks, types, scratch, rule, operands);
    if spells == Some(false) {
        return None;
    }
    // Closed operands the rule has no answer over spell nothing, as `defined` answers.
    let output = match ruled_one(marks, types, scratch, rule, operands) {
        Ok(output) => output?,
        Err(unwalked) => return Some(Err(unwalked)),
    };
    if rigid {
        return Some(Ok(output));
    }
    let operands = collect(scratch, operands.iter().copied());
    let question = Question::Defined { rule, operands };
    let arms = [otherwise, output];
    Some(Ok(decided(
        marks,
        types,
        scratch,
        (question, &arms),
        output,
    )))
}

/// The type on the answer `question` takes, `arms` the type on each answer, every one written over
/// names: [`handed`] of the [decided](DerivedTerm::Decided) term — the one arm where every arm is
/// it, the arm of the question's rigid answer, else the derived name typed on the arms, so it
/// hands back the same type at load and under every binding. Outside a walk, where no binding
/// is written in, it hands back `undecided`.
pub fn decided<U>(
    marks: &dyn Derives<Unwalked = U>,
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
    let name = &mut |term: DerivedTerm<'_>| Ok::<_, U>(marks.derive(term).unwrap_or(undecided));
    (handed(types, scratch, term, name).ok().flatten()).unwrap_or(undecided)
}

/// [`decided`] with each answer's type given by `arm`: only the rigid answer's where there is one,
/// so an arm no binding takes lays nothing.
pub fn decided_by<U>(
    marks: &dyn Derives<Unwalked = U>,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    question: Question<'_>,
    arm: &mut dyn FnMut(Answer) -> Parametric,
    undecided: Parametric,
) -> Parametric {
    if let Some(answer) = rigid(marks, types, scratch, &question) {
        return arm(answer);
    }
    let arms = (0..question.answer_count()).map(|index| arm(question.nth_answer(index)));
    let arms = collect(scratch, arms);
    decided(marks, types, scratch, (question, arms), undecided)
}

/// The type `then` gives where every one of `questions`, each a *yes* or *no* question, answers
/// *yes*, else `otherwise`: decided names chained one question deep each, so it is the same at
/// load and under every binding.
pub fn when_all<U>(
    marks: &dyn Derives<Unwalked = U>,
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
pub fn arrives<U>(
    marks: &dyn Derives<Unwalked = U>,
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
pub fn never_unless<U>(
    marks: &dyn Derives<Unwalked = U>,
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
/// `marks`'s error outside a walk, where no binding is written in.
pub fn signature<U>(
    marks: &dyn Derives<Unwalked = U>,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    signed: &crate::scope::Signed<'_>,
    operands: &[DeclaredType<Parametric>],
) -> Result<Parametric, U> {
    let term = DerivedTerm::Signature { signed, operands };
    let sealed = handed(types, scratch, term, &mut |term| marks.derive(term))?;
    Ok(sealed.expect("a signature seals at every operand"))
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
