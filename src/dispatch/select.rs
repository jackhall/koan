//! Admission and selection: which of a keyworded call's candidates runs.
//!
//! Each candidate's registered expression shape admits the operands' carried types class by class
//! ([`admit_by_class`]), solving a quantified candidate's group as it goes. At a slot the group's
//! solve reads ([`solving_slots`]), an operand the load recorded a **contribution** for is read as
//! that type instead — its static type's upper end, resolved where the call runs — so a call
//! solves from what the load knows of each argument and from the carried type only where it knows
//! nothing. The admitting candidates are ranked by the lattice's per-class verdicts
//! ([`select_by_class`]), which the registry records the first time a pair meets, so dispatch
//! compares no slot types of its own. A lone survivor runs; where several survive, a builtin among
//! them wins, and otherwise the call is ambiguous, whichever scopes the survivors were declared in.
//!
//! Where the load [narrowed](super::statics) a use's candidates, a call selects among those it
//! kept: it admits each *maybe* one, takes each *always* one as admitted — solving only a
//! quantified one's group — and ranks. Where it selected one, the call reads that candidate
//! ([`chosen`]) and admits nothing, save to solve a quantified one's group.
//!
//! A selected function is called by keyword: its argument record binds each slot to the parameter
//! its registration names for it — or packs every slot into `operands` — and carries each of its
//! type parameters, by name, as the type the call solved it to.

use crate::knot::module::layout;
use crate::knot::{BuiltinFunction, KValue, Knotted};
use crate::memory::{Bump, BumpVec, Writer};
use crate::program::Contract;
use crate::scope::{Candidate, Coordinate, IMPLICIT};
use crate::scope::{ParameterBinding, Registered, ShapeGroupMap};
use crate::symbols::BinderSymbol;
use crate::type_lattice::{
    DeclaredType, KType, Parametric, TypeRegistry, Verdict, admit_by_class, satisfied_by,
    scheme_return, scheme_slots, select_by_class, shape_return, shape_slots, solving_slots,
    substitute_quantified,
};
use crate::values::{List, Record, TypeValue, Value};

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

/// One candidate that admitted the call's operands.
#[derive(Clone, Copy)]
struct Admitted<'x, 'graph, 'here> {
    callee: KValue<'graph, 'here>,
    shape: DeclaredType<KType>,
    solution: &'x [KType],
}

/// Select among `candidates`, read through `at`'s view, for operands of the types `arguments`, each
/// solving slot reading its operand's entry of `contributed` where it holds one: each candidate
/// beside the load's verdict, where an *always* one is taken as admitted.
pub(super) fn selected<'x, 'graph, 'here>(
    at: &Evaluation<'graph, 'here>,
    candidates: impl IntoIterator<Item = (Candidate, Verdict)>,
    arguments: &[KType],
    contributed: &[Option<KType>],
    scratch: &'x Bump,
) -> Selection<'x, 'graph, 'here> {
    let types = at.program.types();
    let mut admitted: BumpVec<'_, Admitted<'x, 'graph, 'here>> = BumpVec::new_in(scratch);
    let mut consider = |callee: KValue<'graph, 'here>, verdict: Verdict| {
        let Some(shape) = registered_shape(callee) else {
            return;
        };
        let solution = if verdict == Verdict::Always && matches!(shape, DeclaredType::Type(_)) {
            Some(&[][..])
        } else {
            let solved = solved_from(types, scratch, shape, arguments, contributed);
            admit_by_class(types, scratch, shape, solved)
        };
        match solution {
            Some(solution) => admitted.push(Admitted {
                callee,
                shape,
                solution,
            }),
            None => debug_assert!(verdict != Verdict::Always, "an always candidate admits"),
        }
    };
    for (candidate, verdict) in candidates {
        match candidate {
            Candidate::One(coordinate) => consider(at.view.read(coordinate), verdict),
            Candidate::Spread(coordinate) => {
                let spread = at.view.read(coordinate);
                if let (Some(_), Some(functions)) =
                    (spread.as_list(), spread.surface(types, scratch))
                {
                    for at in 0..functions.len() {
                        consider(functions.child(at, types, scratch).value(), Verdict::Maybe);
                    }
                }
            }
        }
    }
    if admitted.is_empty() {
        return Selection::NoOverload;
    }
    let mut shapes: BumpVec<'_, DeclaredType<Parametric>> =
        BumpVec::with_capacity_in(admitted.len(), scratch);
    shapes.extend(
        admitted
            .iter()
            .map(|admitted| DeclaredType::<Parametric>::from(admitted.shape)),
    );
    let survivors = select_by_class(types, scratch, &shapes);
    let builtin = |index: &usize| {
        admitted[*index]
            .callee
            .as_callable()
            .and_then(Knotted::builtin)
    };
    let chosen = match survivors[..] {
        [only] => admitted[only],
        _ => match survivors.iter().find_map(builtin) {
            Some(builtin) => return Selection::Builtin(builtin),
            None => return Selection::Ambiguous(survivors.len()),
        },
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
        registered: registration(member)
            .expect("a candidate with a shape is a builtin or a registration's function"),
        solution: chosen.solution,
    }
}

/// What a keyworded call of `member` binds its arguments by: a registration's function's own
/// registration, or — for a function behind a view's barriers — the shape the barrier shows its
/// caller, its slots bound to the names the function behind it registers. A barrier's call is that
/// function's by name, which solves its own group, so it binds no type parameter.
fn registration<'here>(member: Knotted<'_, 'here>) -> Option<Registered<'here, KType>> {
    let Some(barrier) = member.coerced() else {
        return member.function()?.registered();
    };
    Some(Registered {
        shape: barrier.ktype(),
        quantifier_map: ShapeGroupMap::default(),
        parameters: member
            .behind_barriers()
            .function()?
            .registered()?
            .parameters,
    })
}

/// The candidate the load selected, read at `coordinate` through `at`'s view, for operands of the
/// types `arguments`: run without admitting, save that a quantified one solves its group from them
/// and `contributed` — and where that solve fails, no overload, as full selection finds.
pub(super) fn chosen<'x, 'graph, 'here>(
    at: &Evaluation<'graph, 'here>,
    coordinate: Coordinate,
    arguments: &[KType],
    contributed: &[Option<KType>],
    scratch: &'x Bump,
) -> Selection<'x, 'graph, 'here> {
    let types = at.program.types();
    let callee = at.view.read(coordinate);
    let member = callee
        .as_callable()
        .expect("a selected candidate is callable");
    if let Some(builtin) = member.builtin() {
        return Selection::Builtin(builtin);
    }
    let registered = registration(member)
        .expect("a selected candidate is a builtin or a registration's function");
    let solution = if matches!(registered.shape, DeclaredType::Type(_)) {
        &[][..]
    } else {
        let solved = solved_from(types, scratch, registered.shape, arguments, contributed);
        match admit_by_class(types, scratch, registered.shape, solved) {
            Some(solution) => solution,
            None => return Selection::NoOverload,
        }
    };
    Selection::Function {
        callee,
        registered,
        solution,
    }
}

/// The types `shape`'s group is solved from: at each slot the solve reads, the argument's
/// contribution where the load recorded one, and its carried type everywhere else.
fn solved_from<'x>(
    types: &TypeRegistry<'_>,
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
    types: &TypeRegistry<'_>,
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

/// Whether two selections run the same thing: one builtin, one function with one solution, or the
/// same miss.
#[cfg(debug_assertions)]
pub(super) fn agree<'graph, 'here>(
    a: &Selection<'_, 'graph, 'here>,
    b: &Selection<'_, 'graph, 'here>,
) -> bool {
    match (a, b) {
        (Selection::Builtin(a), Selection::Builtin(b)) => std::ptr::eq(*a, *b),
        (
            Selection::Function {
                callee: a,
                solution: x,
                ..
            },
            Selection::Function {
                callee: b,
                solution: y,
                ..
            },
        ) => a.as_callable() == b.as_callable() && x == y,
        (Selection::NoOverload, Selection::NoOverload) => true,
        (Selection::Ambiguous(a), Selection::Ambiguous(b)) => a == b,
        _ => false,
    }
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
/// type parameter the shape's group solved to `solution`, built in `writer`'s region.
pub(super) fn arguments<'graph, 'here>(
    types: &TypeRegistry<'_>,
    writer: Writer<'here>,
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
    Value::Record(Record::new(writer, &fields, types, scratch))
}

/// Whether a call of the function registered at `shape`, its group solved to `solution`, returns a
/// type satisfying `contract` — so the evaluation owing the contract can hop to its frame. A call
/// through a barrier never hops, which its caller checks.
pub(super) fn keeps(
    types: &TypeRegistry<'_>,
    shape: DeclaredType<KType>,
    solution: &[KType],
    contract: Contract,
) -> bool {
    let scratch = Bump::new();
    let returns = match shape {
        DeclaredType::Type(shape) => shape_return(shape, types),
        // A run-time solution is concrete, and a scheme holds only its own group's variables.
        DeclaredType::Scheme(scheme) => scheme_return(scheme, types).map(|returns| {
            let returns = substitute_quantified(types, &scratch, returns, solution);
            types
                .concrete(returns)
                .expect("a run-time scheme holds only its own group's variables")
        }),
    };
    returns.is_some_and(|returns| satisfied_by(types, &scratch, contract.returns, returns))
}
