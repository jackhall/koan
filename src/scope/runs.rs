//! **Which of a keyworded use's candidates runs**, as every binding agrees: the closed choice a
//! [`Question::Runs`](super::Question::Runs) asks, beside the two pure steps of selection the run
//! shares with it ([`winner`], [`never_runs`]).
//!
//! The load narrows a use's candidates (`dispatch::walk`'s `narrow`): each candidate's verdict, a
//! builtin's need dropping it, a surfaced key's head at most *maybe*, a *maybe* an *always* one
//! outranks at the first class dropped, then the ranking among *always* ones. [`runs`] restates that
//! choice over what every binding fixes alike — each verdict the lattice gives over the arguments
//! written over names, never a witness's — so a call's type, one decided name per candidate on its
//! answer ([`Run`]), is the same at load and under every binding, and a binding closing the names
//! gives what a fresh load selects. `None` where some binding could choose otherwise.
//!
//! A `MATCH`'s or `TRY`'s arms are chosen alike ([`arms_run`], asked by
//! [`Question::Arms`](super::Question::Arms)): each arm's verdict against the scrutinee, an arm an
//! *always* one outranks dropped, then the arm the *always* ones rank first ([`first_ranked`] over
//! [`guard_outranks`]).
//!
//! A binding here binds lexical names, and a derived name over them: a path variable is a module
//! binder's, which no such binding closes, so a choice over path variables alone is the load's.

use crate::memory::{BumpAllocator, BumpVec, Lays, collect};
use crate::type_lattice::{
    DeclaredType, Interval, KType, Parametric, Variable, Verdict, bound_above, fits, is_subtype_of,
    lower_end_outside, outranks, select_by_class, shape_slots,
};
use crate::values::Chain;

use super::question::{free_variables, unsatisfiable};
use super::typed::Table;

/// How a `USING … SCOPE` block's surfaced key holds a candidate.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Surfacing {
    /// No surfaced key holds it: its verdict is its own.
    No,
    /// The key's one head, a body definition: exactly that head.
    Body,
    /// The key's one head, a signature head: exactly that head where the head is closed, whose
    /// frame retypes its value to the head's return.
    Signature,
    /// Any other surfaced key: it may hold functions admitting more than its head, so at most
    /// *maybe*.
    Loose,
}

/// One candidate of a keyworded use, as the choice of which runs reads it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Running<'a> {
    /// Its registered shape, written over names where its registration reads them.
    pub shape: DeclaredType<Parametric>,
    /// Each argument's static type, as its slot holds it.
    pub arguments: &'a [Interval],
    /// The slots its solve reads an argument's upper end at, a bit per position: what
    /// [`solve_read`] reads.
    pub pinned: u64,
    pub builtin: bool,
    /// Whether it is one function, which a table ranks; a spread is not.
    pub one: bool,
    pub surfacing: Surfacing,
}

impl Running<'_> {
    /// This candidate with its arguments laid again through `lay`.
    pub fn laid<'to>(&self, lay: impl Lays<'to>) -> Running<'to> {
        Running {
            arguments: collect(lay, self.arguments.iter().copied()),
            ..*self
        }
    }
}

/// What a candidate does at a call: [`runs`]'s answer for it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Run {
    /// It never runs: it never admits, a need drops it, or another is selected or outranks it.
    Never,
    /// It may run: the call admits at run among those that may.
    May,
    /// The call runs it whenever it runs.
    Selected,
}

/// What each of `kept`, the candidates of a use in list order, does at its call ([`Run`]), as every
/// binding of the names they are written over agrees: `None` where some binding could choose
/// otherwise. `unfilled` says the use is in unfilled code, where no surfaced head is retyped to.
/// Every candidate answers alike where several always admit and none outranks the rest: the call
/// is refused, and each answers [`May`](Run::May).
pub fn runs<'x>(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'x>,
    kept: &[Running<'_>],
    unfilled: bool,
) -> Option<BumpVec<'x, Run>> {
    let closed = |shape: DeclaredType<Parametric>| types.concrete_declared(shape).is_some();
    let bound = |ty: DeclaredType<Parametric>| bindable(types, scratch, ty);
    let mut verdicts = BumpVec::with_capacity_in(kept.len(), scratch);
    let mut surfacings = BumpVec::with_capacity_in(kept.len(), scratch);
    for each in kept {
        let (surfacing, verdict) = judged(types, scratch, each)?;
        surfacings.push(surfacing);
        verdicts.push(verdict);
    }
    let mut judged = BumpVec::with_capacity_in(kept.len(), scratch);
    judged.extend((0..kept.len()).filter(|at| verdicts[*at] != Verdict::Never));
    // A binding closing a shape may rank it, or let it outrank a *maybe*.
    if judged.len() > 1 && judged.iter().any(|at| bound(kept[*at].shape)) {
        return None;
    }
    let always = || {
        (judged.iter())
            .filter(|at| verdicts[**at] == Verdict::Always)
            .map(|at| kept[*at].shape)
    };
    let mut left = BumpVec::with_capacity_in(judged.len(), scratch);
    left.extend(judged.iter().copied().filter(|at| {
        !(verdicts[*at] == Verdict::Maybe && never_runs(types, scratch, kept[*at].shape, always()))
    }));
    let judged = left;

    let mut answers = BumpVec::with_capacity_in(kept.len(), scratch);
    answers.resize(kept.len(), Run::Never);
    let builtin = |at: usize| kept[judged[at]].builtin;
    if !judged.is_empty() && judged.iter().all(|at| verdicts[*at] == Verdict::Always) {
        let rankable = (judged.iter()).all(|at| kept[*at].one && closed(kept[*at].shape));
        let decided = match judged.len() {
            1 => Some(Ok(0)),
            _ if rankable => {
                let mut shapes = BumpVec::with_capacity_in(judged.len(), scratch);
                shapes.extend(judged.iter().map(|at| kept[*at].shape));
                Some(
                    match Table::ranked(scratch, types, scratch, &shapes, builtin, &mut || {}) {
                        Some(table) => table.decide(u64::MAX >> (64 - judged.len())),
                        None => winner(&select_by_class(types, scratch, &shapes), builtin),
                    },
                )
            }
            _ => None,
        };
        if let Some(Ok(winner)) = decided {
            answers[judged[winner]] = Run::Selected;
            return Some(answers);
        }
    }
    // A lone surfaced signature head is retyped to, so its return is exactly the head's.
    if let [only] = judged[..]
        && surfacings[only] == Surfacing::Signature
        && !unfilled
    {
        answers[only] = Run::Selected;
        return Some(answers);
    }
    for at in judged.iter() {
        answers[*at] = Run::May;
    }
    Some(answers)
}

/// `each`'s verdict as every binding agrees, beside how a surfaced key holds it: a signature head
/// over names is retyped to once a binding closes it, and not before, and one no binding closes is
/// never retyped to; a *maybe* over names a binding closes may become either other verdict, while
/// *always* and *never* hold at every binding. `None` where some binding could judge otherwise.
fn judged(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    each: &Running<'_>,
) -> Option<(Surfacing, Verdict)> {
    let closed = |shape: DeclaredType<Parametric>| types.concrete_declared(shape).is_some();
    let bound = |ty: DeclaredType<Parametric>| bindable(types, scratch, ty);
    let surfacing = match each.surfacing {
        Surfacing::Signature if closed(each.shape) => Surfacing::Signature,
        Surfacing::Signature if bound(each.shape) => return None,
        Surfacing::Signature => Surfacing::Loose,
        surfacing => surfacing,
    };
    let (arguments, read) = solve_read(types, scratch, each);
    if !read {
        return None;
    }
    let mut verdict = super::judge_signed(types, scratch, each.shape, arguments).verdict;
    if each.builtin && verdict != Verdict::Never && dropped(types, scratch, each) {
        verdict = Verdict::Never;
    }
    let open = verdict == Verdict::Maybe
        && (bound(each.shape)
            || (arguments.iter()).any(|at| bound(at.lower.into()) || bound(at.upper.into())));
    let verdict = match surfacing {
        Surfacing::No if open => return None,
        Surfacing::No => verdict,
        Surfacing::Loose => Verdict::Maybe,
        Surfacing::Body | Surfacing::Signature if open => return None,
        Surfacing::Body | Surfacing::Signature => match verdict {
            Verdict::Never => Verdict::Never,
            _ => Verdict::Maybe,
        },
    };
    Some((surfacing, verdict))
}

/// Whether `each` never runs at every binding of the names it is written over, whatever the other
/// candidates do: its own verdict is *never* there. What a [`Question::Runs`](super::Question::Runs)
/// [`runs`] leaves undecided still answers for such a candidate.
pub(super) fn never_admits(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    each: &Running<'_>,
) -> bool {
    judged(types, scratch, each).is_some_and(|(_, verdict)| verdict == Verdict::Never)
}

/// What `running`'s verdict judges at each slot: at a slot its solve reads ([`Running::pinned`]),
/// exactly the argument's upper end, which the call solves from, unless that is `Any`, where the
/// call reads the carried type and the argument is judged whole; at every other slot the argument.
/// Beside it, whether every binding reads the same: a slot whose upper end some binding makes
/// `Any`, and no binding is written in, is read at its upper end, as the load reads it, and is
/// not.
pub fn solve_read<'x>(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'x>,
    running: &Running<'_>,
) -> (&'x [Interval], bool) {
    let mut decided = true;
    let mut read = BumpVec::with_capacity_in(running.arguments.len(), scratch);
    read.extend(
        running
            .arguments
            .iter()
            .enumerate()
            .map(|(position, argument)| {
                if position >= 64 || running.pinned & 1 << position == 0 {
                    return *argument;
                }
                match types.concrete(argument.upper) {
                    Some(closed) if closed == KType::ANY => *argument,
                    Some(_) => Interval::point(argument.upper),
                    None => {
                        decided &= !bindable(types, scratch, argument.upper.into())
                            || bound_above(types, scratch, argument.upper) != KType::ANY;
                        Interval::point(argument.upper)
                    }
                }
            }),
    );
    (read.leak(), decided)
}

/// Whether a builtin's need drops `builtin`: an argument's lower end lies outside the slot it
/// fills. Over a use of several candidates a builtin's needs are its declared slots.
fn dropped(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    builtin: &Running<'_>,
) -> bool {
    let (arguments, _) = solve_read(types, scratch, builtin);
    let DeclaredType::Type(shape) = builtin.shape else {
        return false;
    };
    let Some(shape) = types.concrete(shape) else {
        return false;
    };
    (arguments.iter().zip(shape_slots(shape, types))).any(|(argument, slot)| {
        lower_end_outside(
            types,
            scratch,
            *argument,
            DeclaredType::Type(Parametric::from(slot)),
        )
    })
}

/// Whether a binding of lexical names may change `ty`: it holds a lexical variable or a derived
/// name. A path variable is a module binder's, which no such binding closes.
fn bindable(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    ty: DeclaredType<Parametric>,
) -> bool {
    (free_variables(types, scratch, ty).iter()).any(|variable| {
        matches!(
            variable,
            Variable::Lexical { .. } | Variable::Derived { .. }
        )
    })
}

/// The candidate a call runs among `survivors` — the indices `select_by_class` kept — where
/// `is_builtin` says which are builtins: the lone survivor, or else the first builtin among
/// several; `Err` with the survivors' count where several survive and none is a builtin. The run's
/// selection and the load's both end here.
pub fn winner(survivors: &[usize], is_builtin: impl Fn(usize) -> bool) -> Result<usize, usize> {
    match survivors {
        [only] => Ok(*only),
        _ => (survivors.iter().copied())
            .find(|survivor| is_builtin(*survivor))
            .ok_or(survivors.len()),
    }
}

/// Whether a candidate that may admit, registered at `maybe`, never runs beside candidates that
/// always admit, registered at `always`: one outranks it at the first class, and wherever it admits
/// that one admits too and eliminates it there. Only the first class is safe: past it, a survivor
/// that would eliminate it may itself be gone. Shapes over names outrank nothing.
pub fn never_runs(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    maybe: DeclaredType<Parametric>,
    always: impl IntoIterator<Item = DeclaredType<Parametric>>,
) -> bool {
    let closed = |shape: DeclaredType<Parametric>| types.concrete_declared(shape).is_some();
    closed(maybe)
        && (always.into_iter())
            .any(|always| closed(always) && outranks(types, scratch, always, maybe, 0))
}

/// What kind of guard admits a value to a `MATCH` or `TRY` arm.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GuardKind {
    /// A type, written as a type expression.
    Type,
    /// A scalar literal.
    Value,
    /// A variant or error-kind label.
    Label,
    /// `_`.
    Default,
}

/// One arm of a `MATCH` or `TRY`, as the choice of which runs reads it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ArmRunning {
    pub guard: GuardKind,
    /// The type the guard admits by: `None` for `_`, and for a guard the load cannot read.
    pub admits: Option<Parametric>,
}

/// What each of `arms` does where a scrutinee of the static type `scrutinee` meets them, under the
/// union `clause` where the arms are a `MATCH … UNDER`'s ([`Run`]), as every binding agrees: an arm
/// whose guard can never admit the scrutinee, or that an *always* guarded arm outranks, never runs,
/// nor does `_` beside an *always* guarded one; where every arm left always admits, the one they
/// rank first is selected. `None` where some binding could choose otherwise.
pub fn arms_run<'x>(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'x>,
    scrutinee: Interval,
    clause: Option<Parametric>,
    arms: &[ArmRunning],
) -> Option<BumpVec<'x, Run>> {
    let bound = |ty: Parametric| bindable(types, scratch, ty.into());
    let open = bound(scrutinee.lower) || bound(scrutinee.upper);
    // *Never* where no value of the scrutinee satisfies the guard, *always* where every value does
    // and the guard may always admit, *maybe* otherwise: each at every binding where it is decided.
    let judge = |admits: Parametric, may_always: bool| {
        if unsatisfiable(types, scratch, scrutinee, admits).is_some() {
            return Some(Verdict::Never);
        }
        if fits(types, scratch, scrutinee.upper, admits) {
            return Some(match may_always {
                true => Verdict::Always,
                false => Verdict::Maybe,
            });
        }
        (!open && !bound(admits)).then_some(Verdict::Maybe)
    };
    let mut verdicts = BumpVec::with_capacity_in(arms.len(), scratch);
    for arm in arms {
        verdicts.push(match (arm.guard, arm.admits) {
            (GuardKind::Default, _) => match clause {
                Some(clause) => judge(clause, true)?,
                None => Verdict::Always,
            },
            (GuardKind::Value, Some(admits)) => judge(admits, false)?,
            (_, Some(admits)) => judge(admits, true)?,
            (_, None) => Verdict::Maybe,
        });
    }
    // How each pair ranks: decided where both guards' types are closed, else left to the run,
    // which a binding of a name either one holds may decide.
    let mut ranks = BumpVec::with_capacity_in(arms.len() * arms.len(), scratch);
    for a in arms {
        for b in arms {
            ranks.push(match (a.guard, b.guard, a.admits, b.admits) {
                _ if core::ptr::eq(a, b) => Some(false),
                (GuardKind::Type, GuardKind::Type, Some(x), Some(y)) => {
                    match (types.concrete(x), types.concrete(y)) {
                        (Some(x), Some(y)) => Some(guard_outranks(
                            types,
                            scratch,
                            (a.guard, Some(x)),
                            (b.guard, Some(y)),
                        )),
                        _ if bound(x) || bound(y) => return None,
                        _ => None,
                    }
                }
                _ => Some(guard_outranks(
                    types,
                    scratch,
                    (a.guard, None),
                    (b.guard, None),
                )),
            });
        }
    }
    let closed = ranks.iter().all(Option::is_some);
    let outranks = |a: usize, b: usize| ranks[a * arms.len() + b] == Some(true);
    let mut kept = BumpVec::with_capacity_in(arms.len(), scratch);
    for (at, verdict) in verdicts.iter().enumerate() {
        let dropped = match verdict {
            Verdict::Never => true,
            Verdict::Maybe | Verdict::Always => (0..arms.len()).any(|always| {
                verdicts[always] == Verdict::Always
                    && arms[always].guard != GuardKind::Default
                    && (arms[at].guard == GuardKind::Default
                        || (*verdict == Verdict::Maybe && closed && outranks(always, at)))
            }),
        };
        if !dropped {
            kept.push(at);
        }
    }
    let mut answers = BumpVec::with_capacity_in(arms.len(), scratch);
    answers.resize(arms.len(), Run::Never);
    let always = (kept.iter()).all(|at| verdicts[*at] == Verdict::Always);
    let chosen = match &kept[..] {
        [only] if always => Some(*only),
        [_] => None,
        _ if always && closed => first_ranked(&kept, outranks).ok(),
        _ => None,
    };
    match chosen {
        Some(chosen) => answers[chosen] = Run::Selected,
        None => {
            for at in kept.iter() {
                answers[*at] = Run::May;
            }
        }
    }
    Some(answers)
}

/// Whether the arm guarded by `a`, admitting by the closed type beside it, strictly outranks the
/// one guarded by `b`: a value outranks every type, a type outranks another it lies strictly
/// under, and nothing else outranks — labels admit disjoint variants, and `_` ranks below every
/// guarded arm. The load's ranking and the run's both read it.
pub fn guard_outranks(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    (a, a_type): (GuardKind, Option<KType>),
    (b, b_type): (GuardKind, Option<KType>),
) -> bool {
    match (a, b, a_type, b_type) {
        (GuardKind::Value, GuardKind::Type, _, _) => true,
        (GuardKind::Type, GuardKind::Type, Some(a), Some(b)) => {
            is_subtype_of(types, scratch, a, b) && !is_subtype_of(types, scratch, b, a)
        }
        _ => false,
    }
}

/// The one arm of `admitted`, by position, that no other one there `outranks`; `Err` with how many
/// none outranks where that is not one. What an arm table decides, without a table's bound on how
/// many arms it ranks.
pub fn first_ranked(
    admitted: &[usize],
    outranks: impl Fn(usize, usize) -> bool,
) -> Result<usize, usize> {
    let mut first =
        (admitted.iter().copied()).filter(|at| !admitted.iter().any(|by| outranks(*by, *at)));
    match (first.next(), first.count()) {
        (Some(only), 0) => Ok(only),
        (Some(_), more) => Err(more + 1),
        (None, _) => Err(0),
    }
}
