//! **What a candidate or an arm does at a call** ([`Run`]), beside the pure steps of selection the
//! run shares with the load ([`winner`], [`never_runs`], [`solve_read`]). Which of a keyworded
//! use's candidates runs is its selection decision's ([`Selection::runs`](super::Selection::runs)).
//!
//! Which arms of a `MATCH` or `TRY` run is its arms question's
//! ([`ArmChoice::runs`](super::ArmChoice::runs)); the load ranks closed arms here
//! ([`first_ranked`] over [`guard_outranks`]).

use crate::memory::{BumpAllocator, BumpVec};
use crate::type_lattice::{DeclaredType, Interval, KType, Parametric, is_subtype_of, outranks};
use crate::values::Chain;

/// What a candidate or an arm does at a call.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Run {
    /// It never runs: it never admits, a need drops it, or another is selected or outranks it.
    Never,
    /// It may run: the call admits at run among those that may.
    May,
    /// The call runs it whenever it runs.
    Selected,
}

/// What a candidate's verdict judges at each slot of `arguments`: at a slot its solve reads, a
/// bit each of `pinned`, exactly the argument's upper end, which the call solves from, unless that
/// is `Any`, where the call reads the carried type and the argument is judged whole; at every
/// other slot the argument. An upper end some binding makes `Any` is read as the load reads it.
pub fn solve_read<'x>(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'x>,
    arguments: &[Interval],
    pinned: u64,
) -> &'x [Interval] {
    let mut read = BumpVec::with_capacity_in(arguments.len(), scratch);
    read.extend(arguments.iter().enumerate().map(|(position, argument)| {
        let pinned = position < 64 && pinned & 1 << position != 0;
        match types.concrete(argument.upper) {
            _ if !pinned => *argument,
            Some(closed) if closed == KType::ANY => *argument,
            _ => Interval::point(argument.upper),
        }
    }));
    read.leak()
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
