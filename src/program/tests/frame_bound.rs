//! What a frame binds a top-level binding to, read back after the run: the binding a base record's
//! types close under where a frame's slot holds the value ([`bound_of`]).

use crate::memory::Bump;
use crate::program::{CellSubstrate, KBirth, KBundle, KState, bound_of};
use crate::scheduler::{Action, Step, StepError};
use crate::scope::Bound;
use crate::type_lattice::display_name;

use super::{WANTED, wanted};

/// What a frame binds the top-level binding `name` to, read off the resting top level once the
/// program has run: `type <name>`, or `module <carried type> <digest>`.
pub(crate) fn frame_bound_back(substrate: &mut CellSubstrate, name: &str) -> String {
    super::evaluator::reset();
    WANTED.with(|wanted| *wanted.borrow_mut() = vec![name.to_string()]);
    substrate.with(|running| {
        running
            .inspect(framing)
            .expect("the inspection runs to completion")
    });
    super::evaluator::recorded().concat()
}

/// [`frame_bound_back`]'s root work: it records what a frame binds the wanted binding to.
fn framing<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, KBundle>) -> Action<'graph, KBundle> {
    let (step, state) = step.state();
    let KState::Born(birth @ KBirth::Inspect { program, view }) = state else {
        return step.failed(StepError::Refused);
    };
    let name = WANTED.with(|wanted| wanted.borrow()[0].clone());
    let scratch = Bump::new();
    let types = &step.types();
    let named = |ktype| display_name(ktype, types, program.symbols()).to_string();
    let value = wanted(program, &view, &name);
    let rendered = match bound_of(&scratch, value, types, &scratch, None) {
        Some(Bound::Type(handle)) => format!("type {}", named(handle)),
        Some(Bound::Module { carried, tree }) => {
            let digest = tree
                .digest()
                .expect("a frame binds a module to a closed tree");
            format!("module {} {digest:?}", named(carried))
        }
        Some(Bound::Carried(carried)) => format!("carried {}", named(carried)),
        None => panic!("`{name}` holds a value"),
    };
    super::evaluator::record(rendered);
    step.leave(birth)
}
