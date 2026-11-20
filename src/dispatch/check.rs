//! The overlap check: a user overload may not take operands a builtin overload at its key already
//! takes.
//!
//! It runs where a registration is typed, in the load's walk: as the unit holding it ends, and over
//! a quote's code as the code's closing record is written, which keeps the first refusal for the
//! `EVAL` running the code to report. It reads the registration's expression shape as the load
//! typed it: every closed, unquantified one is checked, whatever declared types its signature
//! names, so a registration whose types wait on a `$` name or a `\` mark is open at the load and
//! checked nowhere until an `EVAL`'s activation types it with the names bound. One written over
//! run-bound names asks which overload it overlaps ([`judge::overlapping`]): none where its bounds
//! already miss each, else the binding's answer, so a binding that makes it overlap refuses it as
//! a fresh load would.
//! A `USING … SCOPE` block's surfaced head births no callable and is not checked here: the
//! definition answering it is the module's own, checked where the module is built.
//! A builtin overload whose operands are all `Any` is shadowable and overlaps nothing; any other
//! overlaps a registration when every slot pair meets above `Never`.

use crate::elaborate::judge::{self, Marks};
use crate::knot::KBuiltins;
use crate::memory::{BumpAllocator, BumpVec};
use crate::scope::{
    BodyShape, Registration, ShapeError, ShapeKind, Slot, Static, StaticRegistered,
};
use crate::type_lattice::{DeclaredType, KType, shape_slots};
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
    marks: &dyn Marks,
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
        overlap(
            marks,
            shape,
            registration,
            registered,
            builtins,
            types,
            scratch,
        )?;
    }
    for (_, nested) in shape.nested_shapes() {
        if nested.kind() != ShapeKind::Code {
            overlaps_through(marks, nested, typed, builtins, types, scratch)?;
        }
    }
    Ok(())
}

/// Refuse `registration`, one of `shape`'s typed `typed`, where it overlaps a builtin overload at
/// its key: checked where it is closed or written over names, unquantified, and births a callable.
pub(super) fn overlap<'graph, 'o>(
    marks: &dyn Marks,
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
    let registered = match typed {
        Static::Closed(registered) => registered.shape.into(),
        Static::Rigid { value, .. } => value.shape,
        _ => return Ok(()),
    };
    // A quantified registration's slots read its own group, which the meet does not relate.
    let DeclaredType::Type(registered) = registered else {
        return Ok(());
    };
    let mut against = BumpVec::new_in(scratch);
    for index in builtins.overloads(registration.key) {
        let Some(builtin) = builtins
            .get(index)
            .as_callable()
            .and_then(|member| member.builtin())
        else {
            continue;
        };
        let builtin = builtin.ktype();
        if !shape_slots(builtin, types).all(|slot| slot == KType::ANY) {
            against.push(builtin);
        }
    }
    if let Some(at) = judge::overlapping(marks, types, scratch, registered, &against) {
        return Err(ShapeError::Overlaps {
            key: registration.elements,
            builtin: against[at],
            at: form.source,
        });
    }
    Ok(())
}
