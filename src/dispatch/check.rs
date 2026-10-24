//! The overlap check: a user overload may not take operands a builtin overload at its key already
//! takes.
//!
//! It runs where a registration is typed, in the load's walk: as the unit holding it ends, and over
//! a quote's code as the code's closing record is written, which keeps the first refusal for the
//! `EVAL` running the code to report. It reads the registration's expression shape as the load
//! typed it: every closed, unquantified one is checked, whatever declared types its signature
//! names, so a registration whose types wait on a `$` name or a `\` mark is open at the load and
//! checked nowhere until an `EVAL`'s activation types it with the names bound.
//! A `USING … SCOPE` block's surfaced head births no callable and is not checked here: the
//! definition answering it is the module's own, checked where the module is built.
//! A builtin overload whose operands are all `Any` is shadowable and overlaps nothing; any other
//! overlaps a registration when every slot pair meets above `Never`.

use crate::knot::KBuiltins;
use crate::memory::BumpAllocator;
use crate::scope::{
    BodyShape, Registration, ShapeError, ShapeKind, Slot, Static, StaticRegistered,
};
use crate::type_lattice::{DeclaredType, KType, meet, shape_slots};
use crate::values::Chain;

#[cfg(test)]
thread_local! {
    /// How many registrations this thread's overlap checks have read: what a test reads to see the
    /// run check nothing.
    pub(super) static CHECKS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Refuse the first registration in `shape`, or in a body nested in it short of a quote's code,
/// that overlaps a builtin overload at its key, each typed as `typed` reads it.
pub(super) fn overlaps_through<'graph, 'o>(
    shape: &'graph BodyShape<'graph>,
    typed: &impl Fn(&BodyShape<'graph>, Slot) -> StaticRegistered<'o>,
    builtins: &KBuiltins<'_, '_>,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
) -> Result<(), ShapeError<'o>>
where
    'graph: 'o,
{
    for registration in shape.registrations() {
        let registered = typed(shape, registration.slot);
        overlap(shape, registration, registered, builtins, types, scratch)?;
    }
    for (_, nested) in shape.nested_shapes() {
        if nested.kind() != ShapeKind::Code {
            overlaps_through(nested, typed, builtins, types, scratch)?;
        }
    }
    Ok(())
}

/// Refuse `registration`, one of `shape`'s typed `typed`, where it overlaps a builtin overload at
/// its key: checked where it is closed and unquantified and births a callable.
pub(super) fn overlap<'graph, 'o>(
    shape: &'graph BodyShape<'graph>,
    registration: &Registration<'graph>,
    typed: StaticRegistered<'_>,
    builtins: &KBuiltins<'_, '_>,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
) -> Result<(), ShapeError<'o>>
where
    'graph: 'o,
{
    #[cfg(test)]
    CHECKS.with(|checks| checks.set(checks.get() + 1));
    if builtins.overloads(registration.key).next().is_none() {
        return Ok(());
    }
    let Some(form) = shape.births(registration.slot).and_then(BodyShape::form) else {
        return Ok(());
    };
    let Static::Closed(registered) = typed else {
        return Ok(());
    };
    // A quantified registration's slots read its own group, which the meet does not relate.
    let DeclaredType::Type(registered) = registered.shape else {
        return Ok(());
    };
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
        let met = shape_slots(registered, types)
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
    Ok(())
}
