//! Shared scaffolding for `module`'s suites: programs shaped, brought into being and ascribed
//! through `function`'s fixture, which is the only thing that can build a module to look at.

mod boundary;
mod coerce;
mod surface;
mod view;

use crate::function::tests::{Fixture, bound};
use crate::function::{KValue, Knotted};
use crate::memory::BumpAllocator;
use crate::scope::Activation;
use crate::symbols::BinderSymbol;
use crate::type_lattice::{KType, SigSchema, TypeRegistry};

use super::layout;

/// The module bound under `name`.
pub(super) fn module<'graph, 'cell>(
    fixture: &Fixture<'_, 'graph>,
    activation: &Activation<'graph, 'cell, Knotted<'graph, 'cell>>,
    name: &str,
) -> Knotted<'graph, 'cell> {
    bound(fixture, activation, name)
        .as_module()
        .unwrap_or_else(|| panic!("`{name}` is bound to a module"))
}

/// The schema of the signature `handle` is.
pub(super) fn schema<'run>(handle: KType, types: &TypeRegistry<'run>) -> SigSchema<'run> {
    layout::schema_of(handle, types).expect("a signature handle")
}

/// The member of `view` named `name`, which must be there.
pub(super) fn member<'graph, 'cell>(
    fixture: &Fixture<'_, 'graph>,
    view: Knotted<'graph, 'cell>,
    name: &str,
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
) -> KValue<'graph, 'cell> {
    let name: BinderSymbol = fixture.name(name);
    layout::member(view, name, types, scratch)
        .unwrap_or_else(|| panic!("the module holds `{name:?}`"))
}
