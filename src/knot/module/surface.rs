//! Entering a `USING … SCOPE` block: each surfaced name bound to the member it names.
//!
//! The shape builder read the surfaced names off the module's declaration where the `USING` was
//! shaped, and laid them down as the block's **parameters** — so a mention of one resolves through
//! the ordinary local read and a callable nested in the block captures it the ordinary way. What
//! is left at run time is the binding: each parameter is bound by name to the member the module's
//! carried signature names ([`layout::member`]), at the type it declares there.
//!
//! A block declares locals of its own beside its parameters, and a local sorts in among them
//! rather than after, so the parameters are picked out by declared position — `Position::PARAMETER`
//! — and not by taking the first `n` slots. A surfaced key is a registration parameter, bound to
//! the list of the functions the module offers at it ([`layout::functions_at`]), which a use at
//! the key spreads.

use crate::knot::{KActivation, KValue, Knotted};
use crate::memory::{BumpAllocator, BumpVec, Writer};
use crate::scope::{Position, ShapeKind, Slot};
use crate::symbols::BinderSymbol;
use crate::values::{Chain, List, Value};

use super::layout;

/// Why a module could not be surfaced into a block.
///
/// None of these is reachable from koan source: the block's parameters *are* the module's
/// declared names, read off that declaration where the `USING` was shaped, so a block and the
/// module it surfaces cannot disagree. They guard a caller that hands this the wrong module.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unsurfaceable {
    /// The operand is no module.
    NotAModule,
    /// The block's parameters and the module's members do not line up.
    Count { expected: usize, found: usize },
    /// The block surfaces a name the module does not hold.
    Unnamed { name: BinderSymbol },
}

/// Bind each surfaced parameter of `block` to the member of `module` it names, read at the type
/// `module`'s carried signature declares it ([`layout::member`]), and each surfaced key to the list
/// of the functions `module` offers there, laid down through `writer`. A refusal binds nothing.
pub fn surface<'graph, 'cell>(
    writer: Writer<'cell, '_>,
    module: KValue<'graph, 'cell>,
    block: &KActivation<'graph, 'cell>,
    types: &Chain<'_, '_, 'cell, '_, '_>,
    scratch: BumpAllocator<'_>,
) -> Result<(), Unsurfaceable> {
    debug_assert_eq!(block.shape().kind(), ShapeKind::Block);
    let carried = (module.knot_member())
        .and_then(Knotted::module)
        .and_then(|_| module.ktype().as_type())
        .ok_or(Unsurfaceable::NotAModule)?;
    let shape = block.shape();

    // Staged first: a refusal binds nothing, so nothing is written until the whole walk agrees.
    let mut bindings = BumpVec::new_in(scratch);
    let mut named = 0;
    for slot in 0..shape.slots() {
        let slot = Slot(slot as u32);
        let name = shape.slot_name(slot);
        // A block declares locals of its own, and a local sorts in among the parameters rather
        // than after them, so a parameter is picked out by its declared position.
        if shape.position_of(slot) != Position::PARAMETER {
            continue;
        }
        // A surfaced key holds every function the module offers at it.
        if let BinderSymbol::Registration(_) = name {
            let key = shape
                .registration(slot)
                .expect("a registration parameter is a surfaced key")
                .key;
            let functions = layout::functions_at(writer, module, key, types, scratch);
            let list = List::of_candidates(writer, functions.iter().copied());
            bindings.push((slot, Value::List(list)));
            continue;
        }
        let member = layout::member(writer, module, name, types, scratch)
            .ok_or(Unsurfaceable::Unnamed { name })?;
        named += 1;
        bindings.push((slot, member));
    }
    let held = layout::named_count(carried, types, scratch);
    if named != held {
        return Err(Unsurfaceable::Count {
            expected: held,
            found: named,
        });
    }
    for (slot, member) in bindings {
        block.bind(slot, member).expect("an empty slot binds");
    }
    Ok(())
}
