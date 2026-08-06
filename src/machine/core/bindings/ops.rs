//! The [`WriteOp`] currency: a published scope's binding-table write expressed as **data** on a
//! step's outcome rather than a mutation a builtin body performs.
//!
//! A builtin body (or a wake-time finish) constructs its value under the step brand — mint, copy,
//! seal — and returns the resulting table write as a `WriteOp` on its [`Action`](crate::machine::Action).
//! The run loop drains a step's ops after the continuation has returned and applies them in program
//! order against the step scope, before finalize. So exactly one code path mutates a published
//! binding table, and it is run-loop-owned: no builtin holds a `Bindings` borrow across user code,
//! which is what lets every write verb here take a firm `borrow_mut`. [`WriteOp::apply`] takes the
//! [`WriteGate`](super::WriteGate) the run loop mints, so a builtin cannot short-circuit its own op
//! back through the interpreter either.
//!
//! An apply error is the node's error terminal — the run loop drops the step's remaining ops and
//! turns the step into an error, so the ordinary finalize arms drop the producer's pending arms
//! and attribute the error. A body that errors before deciding its write installs nothing at all:
//! the writes are outcome data, and an error terminal carries none.
//!
//! Writes into a scope no other node can reach — startup builtin registration into the run-global
//! root, parameter binds into a not-yet-published per-call scope, an ascription view's bulk install
//! — need no such discipline and stay direct (`*_direct` on [`Scope`]), under the construction-door
//! mint of the same gate.

use super::{BindingIndex, DeclarationSite, SealedValue, WriteGate};
use crate::machine::core::carrier_witness::{GroupSeal, OverloadSeal};
use crate::machine::core::{KError, KErrorKind, Scope};
use crate::machine::model::{probe_key, KType};

/// How a [`WriteOp::Type`] meets an existing `types[name]`: `Insert` is strict insert-if-absent (a
/// present name is a `Rebind`), `UpsertEqual` admits a re-entry of the *same* declaration — the
/// nominal finalizes, which overwrite a `RECURSIVE TYPES` block's pre-installed identity and
/// tolerate a parallel finalize of their own slot. Folding the two type writers into one
/// description site keeps the shared skeleton — cross-kind probe, partition guard, in-place
/// finalize of the pending arm — in one place.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) enum TypeWritePolicy {
    Insert,
    UpsertEqual,
}

/// One binding-table write, as data. Ops apply in `Vec` order — program order within the step.
pub(crate) enum WriteOp {
    /// LET binding a value. A value binding is callable by name alone — a function value binds
    /// here like any other value and publishes no keyworded expression.
    Value {
        name: String,
        index: BindingIndex,
        sealed: SealedValue,
    },
    /// `FN` / `OP` overload registration: dispatch bucket only, no `data` entry — the only door a
    /// keyworded expression becomes dispatchable through. `builtin_shadow_guard` is false
    /// only for the operator door — a user module may declare an operator the root already
    /// declares, because dispatch consults the immutable root bucket first and shadowing is
    /// type-gated there.
    Overload {
        name: String,
        index: BindingIndex,
        seal: OverloadSeal,
        builtin_shadow_guard: bool,
    },
    /// Type registration. `builtin_shadow_guard` is set by the user-facing doors (`LET <Type> =`,
    /// `NEWTYPE`, `TYPE`, the nominal finalizes): builtins are immutable and unshadowable at any
    /// depth.
    Type {
        name: String,
        kt: KType,
        site: DeclarationSite,
        policy: TypeWritePolicy,
        builtin_shadow_guard: bool,
    },
    /// One operator-group registration, carrying every probe key it installs under — the per-group
    /// powerset expansion happens where the op is built ([`powerset_probes`]) and apply stays a
    /// loop. One op rather than one per key so the declaration's identity data is built once for
    /// the whole install, not cloned per subset. The group rides as its seal-time bundle, which is
    /// lifetime-free, so a `WriteOp` still names no region borrow.
    Group {
        probes: Vec<String>,
        seal: GroupSeal,
        index: BindingIndex,
    },
    /// A `VAL` slot into the nearest enclosing SIG decl scope's slot collector. A slot is a schema
    /// entry, not a binding — it takes no [`BindingIndex`] and touches no binding map.
    SigSlot { name: String, kt: KType },
}

impl WriteOp {
    /// Apply this write against `scope` — the step scope the op was returned from. The single
    /// interpreter: resolve the write target (forwarding through a transparent `USING` window),
    /// run the door's guards, then mutate the table.
    pub(crate) fn apply(self, scope: &Scope<'_>, gate: &mut WriteGate) -> Result<(), KError> {
        match self {
            WriteOp::Value {
                name,
                index,
                sealed,
            } => {
                let target = value_write_target(scope, &name)?;
                target.assert_open(&name);
                target.bindings().write_value(&name, index, sealed, gate)
            }
            WriteOp::Overload {
                name,
                index,
                seal,
                builtin_shadow_guard,
            } => {
                let target = scope.write_scope();
                target.assert_open(&name);
                // A user overload may not join a builtin's bucket — builtins are immutable and
                // unshadowable. The root registers its own at `BUILTIN`, so only a non-`BUILTIN`
                // index is gated.
                if builtin_shadow_guard
                    && index != BindingIndex::BUILTIN
                    && target.shadows_builtin_function(&seal.key)
                {
                    return Err(KError::new(KErrorKind::Rebind { name }));
                }
                target.bindings().write_overload(&name, index, seal, gate)
            }
            WriteOp::Type {
                name,
                kt,
                site,
                policy,
                builtin_shadow_guard,
            } => {
                let target = scope.write_scope();
                if builtin_shadow_guard && target.shadows_builtin_type(&name) {
                    return Err(KError::new(KErrorKind::Rebind { name }));
                }
                target.assert_open(&name);
                target.bindings().write_type(&name, kt, site, policy, gate)
            }
            WriteOp::Group {
                probes,
                seal,
                index,
            } => {
                let bindings = scope.write_scope().bindings();
                for probe in probes {
                    bindings.write_operator_group(probe, &seal, index, gate)?;
                }
                Ok(())
            }
            WriteOp::SigSlot { name, kt } => scope.write_sig_slot(name, kt),
        }
    }
}

/// The scope a value-side write lands in, with the transparent-`USING` collision check. Reads
/// through such a window consult the window before the call site, so a local bind whose name is
/// already a surfaced module member would be silently shadowed — reject it; otherwise the write
/// forwards to the call site, where the binding belongs (the caller's own block and statement
/// position).
fn value_write_target<'s, 'a>(scope: &'s Scope<'a>, name: &str) -> Result<&'s Scope<'a>, KError> {
    if scope.is_using_window() && scope.bindings().has_value(name, None) {
        return Err(KError::new(KErrorKind::ShapeError(format!(
            "USING: local bind `{name}` collides with a surfaced module member; \
             rename it to avoid silently shadowing the module's `{name}`",
        ))));
    }
    Ok(scope.write_scope())
}

/// The probe key of every nonempty subset of `members` — the powerset-key story
/// [`crate::machine::model::operators`] describes, shared by the builtin seeds, the `GROUP` binder
/// and the `OP` declaration. `members.len()` stays small, so the `2^n - 1` bitmask walk is cheap;
/// each subset's key is derived through [`probe_key`] rather than hand-enumerated, so a
/// registration key always agrees with a real chain's probe.
///
/// One region-hosted record backs every key, so past these strings the whole install allocates
/// nothing.
pub(crate) fn powerset_probes(members: &[&str]) -> Vec<String> {
    let subset_count = 1usize << members.len();
    (1..subset_count)
        .map(|mask| {
            let subset: Vec<&str> = members
                .iter()
                .enumerate()
                .filter(|(bit, _)| mask & (1 << bit) != 0)
                .map(|(_, op)| *op)
                .collect();
            probe_key(&subset)
        })
        .collect()
}
