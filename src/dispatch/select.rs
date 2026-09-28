//! Admission and selection: which of a keyworded call's candidates runs.
//!
//! Each candidate's registered expression shape admits the operands' carried types class by class
//! ([`admit_by_class`]), solving a quantified candidate's group as it goes. The admitting
//! candidates are ranked by the lattice's per-class verdicts ([`select_by_class`]), which the
//! registry records the first time a pair meets, so dispatch compares no slot types of its own. A
//! lone survivor runs; where several survive, a builtin among them wins, and otherwise the call is
//! ambiguous, whichever scopes the survivors were declared in.
//!
//! A selected function is called by keyword: its argument record binds each slot to the parameter
//! its registration names for it — or packs every slot into `operands` — and carries each of its
//! type parameters, by name, as the type the call solved it to.

use crate::elaborate::{Canonical, ParameterBinding, Registered};
use crate::knot::{BuiltinFunction, KValue, Knotted};
use crate::memory::{Bump, BumpVec, Writer};
use crate::program::Contract;
use crate::scope::Candidate;
use crate::scope::{CandidateList, IMPLICIT};
use crate::symbols::BinderSymbol;
use crate::type_lattice::{
    KType, TypeRegistry, admit_by_class, satisfied_by, select_by_class, shape_return,
    substitute_quantified,
};
use crate::values::{List, Record, TypeValue, Value};

use super::{Evaluation, Operand};

/// What a call selected.
pub(super) enum Selection<'x, 'graph, 'here> {
    Builtin(&'graph BuiltinFunction),
    /// A function a registration binds, beside the solution its shape's group took in the shape's
    /// canonical order — empty for an unquantified one.
    Function {
        callee: KValue<'graph, 'here>,
        registered: Registered<'here>,
        solution: &'x [KType],
    },
    NoOverload,
    Ambiguous(usize),
}

/// One candidate that admitted the call's operands.
#[derive(Clone, Copy)]
struct Admitted<'x, 'graph, 'here> {
    callee: KValue<'graph, 'here>,
    shape: KType,
    solution: &'x [KType],
}

/// Select among `list`'x candidates, read through `at`'x view, for operands of the types
/// `arguments`.
pub(super) fn selected<'x, 'graph, 'here>(
    at: &Evaluation<'graph, 'here>,
    list: &CandidateList<'graph>,
    arguments: &[KType],
    scratch: &'x Bump,
) -> Selection<'x, 'graph, 'here> {
    let types = at.program.types();
    let mut admitted: BumpVec<'_, Admitted<'x, 'graph, 'here>> = BumpVec::new_in(scratch);
    let mut consider = |callee: KValue<'graph, 'here>| {
        let Some(shape) = registered_shape(callee) else {
            return;
        };
        if let Some(solution) = admit_by_class(types, scratch, shape, arguments) {
            admitted.push(Admitted {
                callee,
                shape,
                solution,
            });
        }
    };
    for candidate in list.candidates {
        match candidate {
            Candidate::One(coordinate) => consider(at.view.read(*coordinate)),
            Candidate::Spread(coordinate) => {
                if let Some(functions) = at.view.read(*coordinate).as_list() {
                    functions
                        .cells()
                        .iter()
                        .for_each(|function| consider(*function));
                }
            }
        }
    }
    if admitted.is_empty() {
        return Selection::NoOverload;
    }
    let mut shapes = BumpVec::with_capacity_in(admitted.len(), scratch);
    shapes.extend(admitted.iter().map(|admitted| admitted.shape));
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
    let registered = member
        .function()
        .and_then(|function| function.registered())
        .expect("a candidate with a shape is a builtin or a registration's function");
    Selection::Function {
        callee: chosen.callee,
        registered,
        solution: chosen.solution,
    }
}

/// The expression shape `candidate` is registered at: a builtin's, or a registration's function's.
/// `None` for anything else a spread list holds.
fn registered_shape(candidate: KValue<'_, '_>) -> Option<KType> {
    let member = candidate.as_callable()?;
    match member.builtin() {
        Some(builtin) => Some(builtin.ktype()),
        None => member.function()?.registered_shape(),
    }
}

/// The argument record of a keyworded call of `registered`'x function over `operands`, with each
/// type parameter the shape's group solved to `solution`, built in `writer`'x region.
pub(super) fn arguments<'graph, 'here>(
    types: &TypeRegistry<'_>,
    writer: Writer<'here>,
    registered: Registered<'_>,
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
    for (name, canonical) in registered.quantifier_map {
        if let Canonical::At(index) = canonical {
            let solved = TypeValue::new(writer, solution[*index], types);
            fields.push((BinderSymbol::Type(*name), Value::Type(solved)));
        }
    }
    Value::Record(Record::new(writer, &fields, types, scratch))
}

/// Whether a call of the function registered at `shape`, its group solved to `solution`, returns a
/// type satisfying `contract` — so the evaluation owing the contract can hop to its frame.
pub(super) fn keeps(
    types: &TypeRegistry<'_>,
    shape: KType,
    solution: &[KType],
    contract: Contract,
) -> bool {
    let scratch = Bump::new();
    let Some(returns) = shape_return(shape, types) else {
        return false;
    };
    let returns = if solution.is_empty() {
        returns
    } else {
        substitute_quantified(types, &scratch, returns, solution)
    };
    satisfied_by(types, &scratch, contract.returns, returns)
}
