//! The overlap check: a user overload may not take operands a builtin overload at its key already
//! takes.
//!
//! It runs where a shape is built and its types can be read — over a loaded program's shape and
//! every body nested in it, and over a quote's code where an `EVAL` runs it — and reads each
//! registration's expression shape off the cell [the load pass](crate::elaborate::type_channel)
//! filled: every closed, unquantified one is checked, whatever declared types its signature names.
//! A builtin overload whose operands are all `Any` is shadowable and overlaps nothing; any other
//! overlaps a registration when every slot pair meets above `Never`.

use crate::knot::KBuiltins;
use crate::memory::BumpAllocator;
use crate::scope::{BodyShape, ShapeError, ShapeKind, Static};
use crate::type_lattice::{KType, TypeRegistry, meet, shape_slots};

/// Refuse the first registration in `shape`, or in a body nested in it short of a quote's code,
/// that overlaps a builtin overload at its key.
pub(super) fn overlaps<'graph>(
    shape: &'graph BodyShape<'graph>,
    builtins: &KBuiltins<'_, '_>,
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
) -> Result<(), ShapeError<'graph>> {
    for registration in shape.registrations() {
        if builtins.overloads(registration.key).next().is_none() {
            continue;
        }
        let Some(form) = shape.births(registration.slot).and_then(BodyShape::form) else {
            continue;
        };
        let Static::Closed(registered) = shape.registered_type(registration.slot) else {
            continue;
        };
        if !registered.quantifier_map.is_empty() {
            continue;
        }
        for index in builtins.overloads(registration.key) {
            let Some(builtin) = builtins
                .get(index)
                .as_callable()
                .and_then(|member| member.builtin())
            else {
                continue;
            };
            let builtin = builtin.ktype();
            if shape_slots(builtin, types).all(|slot| slot == KType::ANY) {
                continue;
            }
            let met = shape_slots(registered.shape, types)
                .zip(shape_slots(builtin, types))
                .all(|(own, theirs)| meet(types, scratch, own, theirs) != KType::NEVER);
            if met {
                return Err(ShapeError::Overlaps {
                    key: registration.elements,
                    builtin,
                    at: form.source,
                });
            }
        }
    }
    for (_, nested) in shape.nested_shapes() {
        if nested.kind() != ShapeKind::Code {
            overlaps(nested, builtins, types, scratch)?;
        }
    }
    Ok(())
}
