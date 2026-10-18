//! What a call seeks, and what a specialization runs.
//!
//! A call whose callee's body keys a specialization — the load laid it a key list
//! (`BodyShape::specialization_keys`) — hands the drain a [`Seek`], whatever its frame's placement:
//! the build cell the callee was born under, the key of its segment under the content at each
//! keyed name, the types that content carries, and [`specialize`], the work a missed specialization
//! runs. A parameter's content is its argument; a `FOR ALL` name's is the call's solution at it,
//! digested as the type value the frame binds: carried by a keyworded call, held by an instance,
//! and solved here, on the asker's region, for a call by name. A call past a header or a barrier
//! of a body keyed on a `FOR ALL` name is born cold, since its frame solves only once the arguments
//! are retyped or have crossed. That step only lays the cell's own header and table; the drain
//! enters it in its parent's table, and what it holds is the nodes the drain laid from its binding
//! and the warm types its borrowers intern into it. See
//! [build/README.md](../build/README.md).

use crate::build::key_of;
use crate::knot::{Function, KValue, Knotted};
use crate::memory::{Bump, BumpVec};
use crate::scheduler::{Action, Seek, Step, StepError, Work};
use crate::symbols::BinderSymbol;
use crate::type_lattice::KType;
use crate::values::digest::type_digest;
use crate::values::{Chain, KeyedTypes, SpecBinding};

use super::body::admitted;
use super::bundle::{KBirth, KBundle, KState};
use super::record::CallKind;

/// The function a call of `callee` runs, behind every barrier: `None` for a callee that is no
/// function.
pub fn called_function<'graph, 'cell>(
    callee: KValue<'graph, 'cell>,
) -> Option<&'cell Function<'graph, 'cell, Knotted<'graph, 'cell>>> {
    callee.as_callable()?.behind_barriers().function()
}

/// What the call `birth` asks for seeks: `None` for a birth that is no call, a callee that is no
/// function, a body that keys no specialization, and a call whose content at a keyed name the asker
/// cannot read, whose frame is born cold. `types` is the asker's evaluation chain, which reads the
/// arguments; `region` its region's, where a call by name's solution is solved.
pub fn seek<'graph>(
    birth: &KBirth<'graph, '_>,
    types: &Chain<'_, '_, '_, '_>,
    region: &Chain<'_, '_, '_, '_>,
) -> Option<Seek<'graph, KBundle>> {
    let KBirth::Call {
        callee,
        arguments,
        kind,
        contributed,
        ..
    } = birth
    else {
        return None;
    };
    let (callee, arguments) = (*callee, *arguments);
    let function = called_function(callee)?;
    let shape = function.shape();
    let keyed = shape.specialization_keys()?;
    let scratch = Bump::new();
    // The call's solution of its group, solved at most once and only for a keyed `FOR ALL` name:
    // `None` past a header or a barrier, whose frame solves after retyping or crossing.
    let solve = || -> Option<BumpVec<'_, KType>> {
        let mut solved = BumpVec::new_in(&scratch);
        if let Some(instance) = function.instance() {
            solved.extend_from_slice(instance);
            return Some(solved);
        }
        let node = callee.as_callable()?;
        if node.coerced().is_some() || callee.ktype() != node.declared() {
            return None;
        }
        match kind {
            CallKind::Keyworded => {
                solved.resize(function.quantifier_map().iter().count(), KType::NEVER);
                for (name, index) in function.quantifier_map().iter() {
                    let field = arguments.field(name.symbol(), types, &scratch)?;
                    solved[index] = field.value().as_type()?.handle();
                }
                Some(solved)
            }
            CallKind::ByName => {
                let view = node.declared().view(region);
                admitted(region, &scratch, view, arguments, contributed).ok()?
            }
        }
    };
    let mut solution = None;
    let mut digests = BumpVec::with_capacity_in(keyed.len(), &scratch);
    let mut binding = SpecBinding::default();
    for slot in keyed {
        let name = shape.slot_name(*slot);
        if let BinderSymbol::Type(name) = name
            && let Some(index) = function.quantifier_index(name)
        {
            if solution.is_none() {
                solution = Some(solve()?);
            }
            let solved = *solution.as_ref()?.get(index)?;
            let kind = KType::of_kind(solved.kind_of(region));
            digests.push(type_digest(solved, kind));
            binding.push(KeyedTypes {
                carried: kind.into(),
                named: Some(solved),
            });
            continue;
        }
        let argument = arguments.field(name.symbol(), types, &scratch)?.value();
        digests.push(argument.digest(types, &scratch));
        binding.push(KeyedTypes::of(&argument));
    }
    let key = key_of(shape.segment(), digests.into_iter());
    Some(Seek {
        parent: function.closure().build(),
        key,
        binding,
        work: Work {
            step: specialize,
            state: KBirth::Specialize,
        },
    })
}

/// A specialization's step: lay the cell's own header and table, and quiesce. The drain enters it in
/// its parent's table.
pub fn specialize<'graph>(
    step: Step<'_, 'graph, '_, '_, '_, '_, KBundle>,
) -> Action<'graph, KBundle> {
    let (step, state) = step.state();
    match state {
        KState::Born(KBirth::Specialize) => step.enlist(None),
        _ => step.failed(StepError::Refused),
    }
}
