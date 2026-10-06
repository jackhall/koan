//! Entering a `USING … SCOPE` block: each surfaced name bound to the member it names.
//!
//! The shape builder read the surfaced names off the module's declaration where the `USING` was
//! shaped, and laid them down as the block's **parameters** — so a mention of one resolves through
//! the ordinary local read and a callable nested in the block captures it the ordinary way. What
//! is left at run time is the binding, and layout order is what makes it an index: the `k`-th
//! parameter of a channel is member `k` of that channel.
//!
//! A block declares locals of its own beside its parameters, and a local sorts in among them
//! rather than after, so the parameters are picked out by declared position — `Position::PARAMETER`
//! — and not by taking the first `n` slots. A surfaced key is a registration parameter, bound to
//! the list of the functions the module offers at it ([`layout::functions_at`]), which a use at
//! the key spreads.

use crate::knot::{KActivation, Knotted};
use crate::memory::{BumpAllocator, BumpVec, Writer};
use crate::scope::{Position, ShapeKind, Slot};
use crate::symbols::BinderSymbol;
use crate::type_lattice::TypeRegistry;
use crate::values::{List, Value};

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
    /// The block surfaces a name the module does not hold, or holds elsewhere.
    Unnamed { name: BinderSymbol },
}

/// Bind each surfaced parameter of `block` to the member of `module` it names, and each surfaced
/// key to the list of the functions `module` offers there, laid down through `writer`. A refusal
/// binds nothing.
pub fn surface<'graph, 'cell>(
    writer: Writer<'cell>,
    module: Knotted<'graph, 'cell>,
    block: &KActivation<'graph, 'cell>,
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
) -> Result<(), Unsurfaceable> {
    debug_assert_eq!(block.shape().kind(), ShapeKind::Block);
    let node = module.module().ok_or(Unsurfaceable::NotAModule)?;
    let schema = layout::schema_of(node.ktype(), types).expect("a module's type is its signature");
    let shape = block.shape();

    // Staged first: a refusal binds nothing, so nothing is written until the whole walk agrees.
    let mut bindings = BumpVec::new_in(scratch);
    let (mut values, mut kinds) = (0, 0);
    for slot in 0..shape.slots() {
        let slot = Slot(slot as u32);
        let name = shape.slot_name(slot);
        let (_, position) = shape.slot(name).expect("a slot's own name resolves to it");
        // A block declares locals of its own, and a local sorts in among the parameters rather
        // than after them, so a parameter is picked out by its declared position.
        if position != Position::PARAMETER {
            continue;
        }
        // A surfaced key holds every function the module offers at it.
        if let BinderSymbol::Registration(_) = name {
            let key = shape
                .registration(slot)
                .expect("a registration parameter is a surfaced key")
                .key;
            let functions = layout::functions_at(module, key, types, scratch);
            let list = List::of_candidates(writer, functions.iter().copied());
            bindings.push((slot, Value::List(list)));
            continue;
        }
        // The `k`-th parameter of a channel is member `k` of that channel — the layout law, which
        // is what makes surfacing an index rather than a lookup.
        let index = match name {
            BinderSymbol::Value(_) => {
                values += 1;
                values - 1
            }
            BinderSymbol::Type(_) => {
                kinds += 1;
                layout::value_count(&schema) + kinds - 1
            }
            BinderSymbol::Registration(_) | BinderSymbol::Key(_) => {
                unreachable!("a parameter is a written name")
            }
        };
        if layout::member_index(&schema, scratch, name) != Some(index) {
            return Err(Unsurfaceable::Unnamed { name });
        }
        bindings.push((slot, node.members()[index]));
    }
    let held = layout::member_count(&schema, scratch);
    if values + kinds != held {
        return Err(Unsurfaceable::Count {
            expected: held,
            found: values + kinds,
        });
    }
    for (slot, member) in bindings {
        block.bind(slot, member).expect("an empty slot binds");
    }
    Ok(())
}
