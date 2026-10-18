//! What a call seeks, and what a specialization runs.
//!
//! A call whose callee's body keys a specialization — its caching record lists the parameters its
//! invariant sites read — hands the drain a [`Seek`], whatever its frame's placement: the build cell
//! the callee was born under, the key of its segment under the arguments at those parameters, the
//! types those arguments carry, and [`specialize`], the work a missed specialization runs. That step
//! only lays the cell's own header and table; the drain enters it in its parent's table, and what it
//! holds is the nodes the drain laid from its binding and the warm types its borrowers intern into
//! it. See
//! [build/README.md](../build/README.md).

use crate::build::key_of;
use crate::knot::{Function, KValue, Knotted};
use crate::memory::{Bump, BumpVec};
use crate::scheduler::{Action, Seek, Step, StepError, Work};
use crate::values::{Chain, KeyedTypes, SpecBinding};

use super::bundle::{KBirth, KBundle, KState};

/// The function a call of `callee` runs, behind every barrier: `None` for a callee that is no
/// function.
pub fn called_function<'graph, 'cell>(
    callee: KValue<'graph, 'cell>,
) -> Option<&'cell Function<'graph, 'cell, Knotted<'graph, 'cell>>> {
    callee.as_callable()?.behind_barriers().function()
}

/// What a call of `callee` over `arguments`, a record of its parameters by name, seeks: `None` for
/// a callee that is no function, a body that keys no specialization, and arguments missing a keyed
/// parameter, whose frame is born cold.
pub fn seek<'graph>(
    callee: KValue<'graph, '_>,
    arguments: KValue<'graph, '_>,
    types: &Chain<'_, '_, '_, '_>,
) -> Option<Seek<'graph, KBundle>> {
    let function = called_function(callee)?;
    let shape = function.shape();
    let keyed = shape.caching()?.invariant?;
    let scratch = Bump::new();
    let mut digests = BumpVec::with_capacity_in(keyed.len(), &scratch);
    let mut binding = SpecBinding::default();
    for slot in keyed {
        let name = shape.slot_name(*slot).symbol();
        let argument = arguments.field(name, types, &scratch)?.value();
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
