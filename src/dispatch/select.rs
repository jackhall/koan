//! Admission and selection: which of a keyworded call's candidates runs.
//!
//! Each candidate's registered expression shape admits the operands' carried types class by class
//! ([`admit_by_class`]), solving a quantified candidate's group as it goes. At a slot the group's
//! solve reads ([`solving_slots`]), an operand the load recorded a **contribution** for is read as
//! that type instead — its static type's upper end, resolved where the call runs — so a call
//! solves from what the load knows of each argument and from the carried type only where it knows
//! nothing. The admitting candidates are eliminated class by class over a [`Table`] of the
//! lattice's per-class verdicts ([`outranks`]), which the registry records the first time a pair
//! meets, so dispatch compares no slot types of its own. A lone survivor runs; where several
//! survive, a builtin among them wins, and otherwise the call is ambiguous, whichever scopes the
//! survivors were declared in.
//!
//! Where the load [narrowed](super::statics) a use's candidates, a call selects among those it
//! kept: it admits each *maybe* one, takes each *always* one as admitted, and ranks. A kept list
//! of closed functions carries the table the load recorded, so the call ranks nothing; any other
//! list's table is built at the call over what admitted. Where the load selected one, the call
//! reads that candidate ([`chosen`]) and admits nothing. An *always* or selected candidate's group
//! is solved by the load's [points](crate::scope::Points): each variable's closed type, a type
//! over the run's lexical variables substituted as a contribution is, or the type an argument
//! carries; a debug build checks it against admission.
//!
//! A selected function is called by keyword: its argument record binds each slot to the parameter
//! its registration names for it — or packs every slot into `operands` — and carries each of its
//! type parameters, by name, as the type the call solved it to.

use crate::knot::module::layout;
use crate::knot::{BuiltinFunction, KValue, Knotted};
use crate::memory::{Bump, BumpVec, Lays, Writer};
use crate::scope::{Candidate, Coordinate, Elimination, IMPLICIT, Points, Table};
use crate::scope::{ParameterBinding, Registered, ShapeGroupMap};
use crate::symbols::BinderSymbol;
use crate::type_lattice::{
    DeclaredType, KType, Parametric, Verdict, admit_by_class, outranks, satisfied_by, scheme_slots,
    select_by_class, shape_slots, solving_slots, substitute_quantified,
};
use crate::values::{Chain, List, Record, TypeValue, Value};

use super::{Evaluation, Operand};

/// What a call selected.
pub(super) enum Selection<'x, 'graph, 'here> {
    Builtin(&'graph BuiltinFunction),
    /// A function a registration binds, beside the solution its shape's group took in the shape's
    /// group order — empty for an unquantified one.
    Function {
        callee: KValue<'graph, 'here>,
        registered: Registered<'here, KType>,
        solution: &'x [KType],
    },
    NoOverload,
    Ambiguous(usize),
}

/// One candidate that admitted the call's operands, beside its position on the list it was kept
/// on.
#[derive(Clone, Copy)]
struct Admitted<'x, 'graph, 'here> {
    callee: KValue<'graph, 'here>,
    shape: DeclaredType<KType>,
    solution: &'x [KType],
    position: usize,
}

/// Select among `candidates`, read through `at`'s view, for operands of the types `arguments`, each
/// solving slot reading its operand's entry of `contributed` where it holds one: each candidate
/// beside the load's verdict, where an *always* one is taken as admitted. Those that admit are
/// ranked by `elimination`, which names them by their position among `candidates`, or, where it is
/// open, by a table of the admitted ones built here.
pub(super) fn selected<'x, 'graph, 'here>(
    at: &Evaluation<'_, 'graph, 'here, '_, '_, '_>,
    candidates: impl IntoIterator<Item = (Candidate, Verdict)>,
    points: &[Points<'_>],
    elimination: Elimination<'_>,
    arguments: &[KType],
    contributed: &[Option<KType>],
    scratch: &'x Bump,
) -> Selection<'x, 'graph, 'here> {
    let types = &at.types;
    let mut admitted: BumpVec<'_, Admitted<'x, 'graph, 'here>> = BumpVec::new_in(scratch);
    let mut positions = 0u64;
    let mut consider = |callee: KValue<'graph, 'here>, verdict: Verdict, position: usize| {
        let Some(shape) = registered_shape(callee) else {
            return;
        };
        let solution = match (verdict, shape) {
            (Verdict::Always, DeclaredType::Type(_)) => Some(&[][..]),
            (Verdict::Always, DeclaredType::Scheme(_)) => {
                let points = points.get(position).unwrap_or(&Points::NONE);
                match pointed(points, arguments, scratch) {
                    Some(solution) => {
                        debug_assert!(agrees(at, shape, arguments, contributed, solution));
                        Some(solution)
                    }
                    None => admitted_at_run(types, scratch, shape, arguments, contributed),
                }
            }
            _ => {
                #[cfg(test)]
                ADMITTED.with(|admitted| admitted.set(admitted.get() + 1));
                let solved = solved_from(types, scratch, shape, arguments, contributed);
                admit_by_class(types, scratch, shape, solved)
            }
        };
        match solution {
            Some(solution) => {
                positions |= 1 << position.min(63);
                admitted.push(Admitted {
                    callee,
                    shape,
                    solution,
                    position,
                })
            }
            None => debug_assert!(verdict != Verdict::Always, "an always candidate admits"),
        }
    };
    for (position, (candidate, verdict)) in candidates.into_iter().enumerate() {
        match candidate {
            Candidate::One(coordinate) => consider(at.view.read(coordinate), verdict, position),
            Candidate::Spread(coordinate) => {
                let spread = at.view.read(coordinate);
                if let (Some(_), Some(functions)) =
                    (spread.as_list(), spread.surface(types, scratch))
                {
                    for at in 0..functions.len() {
                        let function = functions.child(at, types, scratch).value();
                        consider(function, Verdict::Maybe, position);
                    }
                }
            }
        }
    }
    if admitted.is_empty() {
        return Selection::NoOverload;
    }
    let decided = match elimination {
        Elimination::Table(table) => table.decide(positions).map(|position| {
            (admitted
                .iter()
                .position(|admitted| admitted.position == position))
            .expect("a table's winner admitted")
        }),
        Elimination::Open => {
            let mut shapes: BumpVec<'_, DeclaredType<Parametric>> =
                BumpVec::with_capacity_in(admitted.len(), scratch);
            shapes.extend((admitted.iter()).map(|admitted| DeclaredType::from(admitted.shape)));
            let builtin = |index: usize| {
                (admitted[index].callee.as_callable())
                    .and_then(Knotted::builtin)
                    .is_some()
            };
            // More than a table ranks: the lattice's own elimination.
            match table(scratch, types, scratch, &shapes, builtin) {
                Some(table) => table.decide(u64::MAX >> (64 - admitted.len())),
                None => winner(&select_by_class(types, scratch, &shapes), builtin),
            }
        }
    };
    let chosen = match decided {
        Ok(index) => admitted[index],
        Err(count) => return Selection::Ambiguous(count),
    };
    let member = chosen
        .callee
        .as_callable()
        .expect("a candidate with a shape is callable");
    if let Some(builtin) = member.builtin() {
        return Selection::Builtin(builtin);
    }
    Selection::Function {
        callee: chosen.callee,
        registered: registration(chosen.callee)
            .expect("a candidate with a shape is a builtin or a registration's function"),
        solution: chosen.solution,
    }
}

/// The candidate a call runs among `survivors` — the indices `select_by_class` kept — where
/// `is_builtin` says which are builtins: the lone survivor, or else the first builtin among
/// several; `Err` with the survivors' count where several survive and none is a builtin. The run's
/// selection and the load's both end here.
pub(super) fn winner(
    survivors: &[usize],
    is_builtin: impl Fn(usize) -> bool,
) -> Result<usize, usize> {
    match survivors {
        [only] => Ok(*only),
        _ => (survivors.iter().copied())
            .find(|survivor| is_builtin(*survivor))
            .ok_or(survivors.len()),
    }
}

/// The elimination table of `shapes`, registered shapes under one key and one ranking, laid
/// through `lay`; `builtin` says which are builtins. `None` for more shapes than a table ranks.
pub(super) fn table<'l>(
    lay: impl Lays<'l>,
    types: &Chain<'_, '_, '_, '_>,
    scratch: &Bump,
    shapes: &[DeclaredType<Parametric>],
    builtin: impl Fn(usize) -> bool,
) -> Option<Table<'l>> {
    #[cfg(test)]
    let mut ranked = || RANKED.with(|ranked| ranked.set(ranked.get() + 1));
    #[cfg(not(test))]
    let mut ranked = || {};
    Table::ranked(lay, types, scratch, shapes, builtin, &mut ranked)
}

#[cfg(test)]
thread_local! {
    /// How many pairs of shapes were ranked: what a test reads to see a call rank nothing.
    pub(crate) static RANKED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many subsets of a recorded table's list [`checked`] held to the lattice's elimination,
    /// `None` while no law asks.
    pub(crate) static SUBSETS: std::cell::Cell<Option<usize>> =
        const { std::cell::Cell::new(None) };
}

/// The ranking law's check at a call over `entries`, a kept list the load recorded `table` for:
/// at every subset of the list as admitted, the table decides what the lattice's class-by-class
/// elimination and [`winner`] decide over those shapes.
#[cfg(test)]
pub(super) fn checked(
    at: &Evaluation<'_, '_, '_, '_, '_, '_>,
    entries: &[(Candidate, Verdict)],
    table: &Table<'_>,
    scratch: &Bump,
) {
    let Some(count) = SUBSETS.with(std::cell::Cell::get) else {
        return;
    };
    let types = &at.types;
    let read = |candidate: &(Candidate, Verdict)| match candidate.0 {
        Candidate::One(coordinate) => at.view.read(coordinate),
        Candidate::Spread(_) => unreachable!("a table ranks no spread"),
    };
    let mut shapes: BumpVec<'_, DeclaredType<Parametric>> = BumpVec::new_in(scratch);
    shapes.extend((entries.iter()).map(|entry| {
        DeclaredType::from(registered_shape(read(entry)).expect("a ranked candidate has a shape"))
    }));
    let builtin = |at: usize| (read(&entries[at]).as_callable()).and_then(Knotted::builtin);
    for mask in 1u64..1 << shapes.len() {
        let mut subset = BumpVec::new_in(scratch);
        subset.extend((0..shapes.len()).filter(|at| mask & 1 << at != 0));
        let mut picked = BumpVec::new_in(scratch);
        picked.extend(subset.iter().map(|at| shapes[*at]));
        let survivors = select_by_class(types, scratch, &picked);
        let expected = winner(&survivors, |k| builtin(subset[k]).is_some()).map(|k| subset[k]);
        assert_eq!(table.decide(mask), expected, "over the subset {subset:?}");
    }
    SUBSETS.with(|subsets| subsets.set(Some(count + 1)));
}

/// Whether a candidate that may admit, registered at `maybe`, never runs beside candidates that
/// always admit, registered at `always`: one outranks it at the first class, and wherever it admits
/// that one admits too and eliminates it there. Only the first class is safe: past it, a survivor
/// that would eliminate it may itself be gone.
pub(super) fn never_runs(
    types: &Chain<'_, '_, '_, '_>,
    scratch: &Bump,
    maybe: DeclaredType<Parametric>,
    always: impl IntoIterator<Item = DeclaredType<Parametric>>,
) -> bool {
    (always.into_iter()).any(|always| outranks(types, scratch, always, maybe, 0))
}

/// What a keyworded call of `callee` binds its arguments by: a registration's function's own
/// registration, or — for a function behind a view's barriers or a header — the shape shown its
/// caller, its slots bound to the names the function behind it registers. Such a call is that
/// function's by name, which solves its own group, so it binds no type parameter.
fn registration<'here>(callee: KValue<'_, 'here>) -> Option<Registered<'here, KType>> {
    let member = callee.as_callable()?;
    if member.coerced().is_none() && !matches!(callee, Value::Retyped(_)) {
        return member.function()?.registered();
    }
    Some(Registered {
        shape: layout::registered_shape(callee)?,
        quantifier_map: ShapeGroupMap::default(),
        parameters: member
            .behind_barriers()
            .function()?
            .registered()?
            .parameters,
    })
}

/// The candidate the load selected, read at `coordinate` through `at`'s view, for operands of the
/// types `arguments`: run without admitting, a quantified one's group solved by `points`, the
/// load's, which a debug build checks against admitting it over `arguments` and `contributed`.
pub(super) fn chosen<'x, 'graph, 'here>(
    at: &Evaluation<'_, 'graph, 'here, '_, '_, '_>,
    coordinate: Coordinate,
    points: &Points<'_>,
    arguments: &[KType],
    contributed: &[Option<KType>],
    scratch: &'x Bump,
) -> Selection<'x, 'graph, 'here> {
    let callee = at.view.read(coordinate);
    let member = callee
        .as_callable()
        .expect("a selected candidate is callable");
    if let Some(builtin) = member.builtin() {
        return Selection::Builtin(builtin);
    }
    let registered = registration(callee)
        .expect("a selected candidate is a builtin or a registration's function");
    let solution = match registered.shape {
        DeclaredType::Type(_) => &[][..],
        DeclaredType::Scheme(_) => match pointed(points, arguments, scratch) {
            Some(solution) => {
                debug_assert!(agrees(
                    at,
                    registered.shape,
                    arguments,
                    contributed,
                    solution
                ));
                solution
            }
            None => admitted_at_run(&at.types, scratch, registered.shape, arguments, contributed)
                .expect("a selected candidate admits its call's arguments"),
        },
    };
    Selection::Function {
        callee,
        registered,
        solution,
    }
}

/// The solution `points` give where a call over operands carrying `arguments` runs: `None` where
/// one is rigid, which no run reads — the layer binding its variables closes it.
fn pointed<'x>(points: &Points<'_>, arguments: &[KType], scratch: &'x Bump) -> Option<&'x [KType]> {
    let solved = points.solved(scratch, arguments);
    debug_assert!(solved.is_some(), "a rigid point reached the run");
    Some(solved?.leak())
}

/// `shape`'s group solved by admitting `arguments` where the call runs, as a candidate the load
/// left to the call is.
fn admitted_at_run<'x>(
    types: &Chain<'_, '_, '_, '_>,
    scratch: &'x Bump,
    shape: DeclaredType<KType>,
    arguments: &[KType],
    contributed: &[Option<KType>],
) -> Option<&'x [KType]> {
    let solved = solved_from(types, scratch, shape, arguments, contributed);
    admit_by_class(types, scratch, shape, solved)
}

/// The points law's check: whether the solution the load's points give is what admitting `shape`
/// over the arguments' types solves its group to. Not gated on `debug_assertions`: its callers are
/// `debug_assert!`s, which type-check in every profile.
fn agrees(
    at: &Evaluation<'_, '_, '_, '_, '_, '_>,
    shape: DeclaredType<KType>,
    arguments: &[KType],
    contributed: &[Option<KType>],
    solution: &[KType],
) -> bool {
    #[cfg(test)]
    POINTED.with(|pointed| pointed.set(pointed.get() + 1));
    let (types, scratch) = (&at.types, Bump::new());
    let carried = scratch.alloc_slice_copy(arguments);
    let solved = solved_from(types, &scratch, shape, carried, contributed);
    admit_by_class(types, &scratch, shape, solved) == Some(solution)
}

#[cfg(test)]
thread_local! {
    /// How many candidates a call admitted, solving any group by admission.
    pub(crate) static ADMITTED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many calls read their solution off the load's points.
    pub(crate) static POINTED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The types `shape`'s group is solved from: at each slot the solve reads, the argument's
/// contribution where the load recorded one, and its carried type everywhere else.
fn solved_from<'x>(
    types: &Chain<'_, '_, '_, '_>,
    scratch: &'x Bump,
    shape: DeclaredType<KType>,
    carried: &'x [KType],
    contributed: &[Option<KType>],
) -> &'x [KType] {
    if contributed.is_empty() || matches!(shape, DeclaredType::Type(_)) {
        return carried;
    }
    let solving = solving_slots(types, scratch, shape.into());
    if solving.len() != carried.len() {
        return carried;
    }
    scratch.alloc_slice_fill_iter((0..carried.len()).map(|slot| {
        match (solving[slot], contributed.get(slot).copied().flatten()) {
            (true, Some(contribution)) => contribution,
            _ => carried[slot],
        }
    }))
}

/// Whether each of `carried` lies under its slot of `shape` at `solution`. Not gated on
/// `debug_assertions`: its one caller is a `debug_assert!`, which type-checks in every profile.
pub(super) fn carried_fit(
    types: &Chain<'_, '_, '_, '_>,
    scratch: &Bump,
    shape: DeclaredType<KType>,
    solution: &[KType],
    carried: &[KType],
) -> bool {
    let slots: BumpVec<'_, KType> = match shape {
        DeclaredType::Type(shape) => {
            let mut slots = BumpVec::new_in(scratch);
            slots.extend(shape_slots(shape, types));
            slots
        }
        DeclaredType::Scheme(scheme) => {
            let mut slots = BumpVec::new_in(scratch);
            slots.extend(scheme_slots(scheme, types).map(|slot| {
                types
                    .concrete(substitute_quantified(types, scratch, slot, solution))
                    .expect("a run-time solution is concrete")
            }));
            slots
        }
    };
    slots.len() == carried.len()
        && slots
            .iter()
            .zip(carried)
            .all(|(slot, argument)| satisfied_by(types, scratch, *slot, *argument))
}

/// The expression shape `candidate` is registered at: a builtin's, a registration's function's, or
/// the one a barrier shows its caller. `None` for anything else a spread list holds.
fn registered_shape(candidate: KValue<'_, '_>) -> Option<DeclaredType<KType>> {
    let member = candidate.as_callable()?;
    match member.builtin() {
        Some(builtin) => Some(builtin.ktype().into()),
        None => layout::registered_shape(candidate),
    }
}

/// The argument record of a keyworded call of `registered`'s function over `operands`, with each
/// type parameter the shape's group solved to `solution` — or, for a header or a barrier, each
/// name of the shape's group — built in `writer`'s region.
pub(super) fn arguments<'graph, 'here>(
    types: &Chain<'_, 'here, '_, '_>,
    writer: Writer<'here, '_>,
    registered: Registered<'_, KType>,
    operands: &[Operand<'graph, 'here>],
    solution: &[KType],
    scratch: &Bump,
) -> KValue<'graph, 'here> {
    let value = |operand: &Operand<'graph, 'here>| {
        operand.value().expect("a registration's slot is evaluated")
    };
    let mut fields = BumpVec::with_capacity_in(operands.len() + solution.len(), scratch);
    match registered.parameters {
        ParameterBinding::Named(names) => {
            fields.extend(names.iter().copied().zip(operands.iter().map(value)));
        }
        ParameterBinding::Operands => {
            let packed = List::new(writer, operands.iter().map(value), types, scratch);
            fields.push((
                BinderSymbol::Value(IMPLICIT.operands.symbol()),
                Value::List(packed),
            ));
        }
    }
    for (name, index) in registered.quantifier_map.iter() {
        let solved = TypeValue::new(writer, solution[index], types);
        fields.push((BinderSymbol::Type(name), Value::Type(solved)));
    }
    // A header's or a barrier's shape binds no type parameter, but its frame retypes the value to
    // the shape's return under this solution: it travels by the shape's own names, as a value, so
    // a copied birth adopts it.
    if registered.quantifier_map.is_empty() && !solution.is_empty() {
        let names = registered.shape.view(types).group.names;
        for (name, solved) in names.iter().zip(solution) {
            let solved = TypeValue::new(writer, *solved, types);
            fields.push((BinderSymbol::Type(*name), Value::Type(solved)));
        }
    }
    Value::Record(Record::new(writer, &fields, types, scratch))
}
