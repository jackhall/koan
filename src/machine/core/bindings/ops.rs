//! The [`WriteOp`] currency: a published scope's binding-table write expressed as **data** on a
//! step's outcome rather than a mutation a builtin body performs.
//!
//! A builtin body (or a wake-time finish) constructs its value under the step brand — mint, copy,
//! seal — and returns the resulting table write as a `WriteOp` on its [`Action`](crate::machine::Action).
//! The run loop drains a step's ops after the continuation has returned and applies them in program
//! order against the step scope, before finalize. So a published binding table is mutated from the
//! run loop rather than from a builtin body: no builtin holds a `Bindings` borrow across user code,
//! which is what lets every write verb here take a firm `borrow_mut`. [`WriteOp::apply`] takes the
//! [`WriteGate`](super::WriteGate) the run loop mints, so a builtin cannot short-circuit its own op
//! back through the interpreter either.
//!
//! An apply error is the node's error terminal — the run loop drops the step's remaining ops and
//! turns the step into an error, so the ordinary finalize arms retire the producer's claims
//! and attribute the error. A body that errors before deciding its write installs nothing at all:
//! the writes are outcome data, and an error terminal carries none.
//!
//! A write into a scope no other node can reach — one still under construction — needs no such
//! discipline and stays direct (`*_direct` on [`Scope`]), under the construction-door mint of the
//! same gate.

use super::{BindingIndex, DeclarationSite, SealedValue, WriteGate};
use crate::machine::core::carrier_witness::{GroupSeal, OverloadSeal};
use crate::machine::core::{KError, KErrorKind, Scope};
use crate::machine::model::{KType, probe_key};

/// How a [`WriteOp::Type`] meets an existing `types[name]`: `Insert` is strict insert-if-absent (a
/// present name is a `Rebind`), `UpsertEqual` admits a re-entry of the *same* declaration — the
/// nominal finalizes, which overwrite an announced group's pre-installed identity and
/// tolerate a parallel finalize of their own slot. Folding the two type writers into one
/// description site keeps the shared skeleton — cross-kind probe, partition guard, and the retire
/// of the write's own claim — in one place.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) enum TypeWritePolicy {
    Insert,
    UpsertEqual,
}

/// One binding-table write, as data. Ops apply in `Vec` order — program order within the step.
pub(crate) enum WriteOp<'a> {
    /// LET binding a value. A value binding is callable by name alone — a function value binds
    /// here like any other value and publishes no keyworded expression.
    Value {
        name: String,
        index: BindingIndex,
        sealed: SealedValue<'a>,
    },
    /// `FN` / `OP` overload registration: dispatch bucket only, no `data` entry.
    /// `builtin_shadow_guard` is false only for the operator door — a user module may declare an
    /// operator the root already declares, because dispatch consults the immutable root bucket
    /// first and shadowing is type-gated there.
    Overload {
        name: String,
        index: BindingIndex,
        seal: OverloadSeal<'a>,
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
        seal: GroupSeal<'a>,
        index: BindingIndex,
    },
    /// A `VAL` slot into the nearest enclosing SIG decl scope's slot collector. A slot is a schema
    /// entry, not a binding — it takes no [`BindingIndex`] and touches no binding map.
    SigSlot { name: String, kt: KType },
}

impl<'a> WriteOp<'a> {
    /// Apply this write against `scope` — the step scope the op was returned from, which is always
    /// the scope the entry lands in. Runs the door's guards before the table verb.
    pub(crate) fn apply(self, scope: &Scope<'a>, gate: &mut WriteGate) -> Result<(), KError> {
        scope.assert_owns_bindings();
        match self {
            WriteOp::Value {
                name,
                index,
                sealed,
            } => {
                scope.assert_open(&name);
                scope.bindings().write_value(&name, index, sealed, gate)
            }
            WriteOp::Overload {
                name,
                index,
                seal,
                builtin_shadow_guard,
            } => {
                scope.assert_open(&name);
                // A user overload may not join a builtin's bucket — builtins are immutable and
                // unshadowable. The root registers its own at `BUILTIN`, so only a non-`BUILTIN`
                // index is gated.
                if builtin_shadow_guard
                    && index != BindingIndex::BUILTIN
                    && scope.shadows_builtin_function(&seal.key)
                {
                    return Err(KError::new(KErrorKind::Rebind { name }));
                }
                scope.bindings().write_overload(&name, index, seal, gate)
            }
            WriteOp::Type {
                name,
                kt,
                site,
                policy,
                builtin_shadow_guard,
            } => {
                if builtin_shadow_guard && scope.shadows_builtin_type(&name) {
                    return Err(KError::new(KErrorKind::Rebind { name }));
                }
                scope.assert_open(&name);
                scope.bindings().write_type(&name, kt, site, policy, gate)
            }
            WriteOp::Group {
                probes,
                seal,
                index,
            } => {
                let bindings = scope.bindings();
                for probe in probes {
                    bindings.write_operator_group(probe, &seal, index, gate)?;
                }
                Ok(())
            }
            WriteOp::SigSlot { name, kt } => scope.write_sig_slot(name, kt),
        }
    }
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
