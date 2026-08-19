//! The [`WriteGate`] capability: the compile-time proof that a binding-table write is happening at
//! one of the two doors allowed to perform one.
//!
//! Every write verb on [`Bindings`](super::Bindings) — and every `*_direct` door on
//! [`Scope`](crate::machine::core::Scope) that reaches one — takes a `&mut WriteGate`. The gate is
//! a zero-sized token with no public constructor and no `Clone`, minted only inside
//! `crate::machine`, so a builtin body (`crate::builtins` is a sibling of `crate::machine`, not a
//! descendant) cannot produce one and therefore cannot name a write verb at all — a resolution
//! failure rather than a convention.
//!
//! `&mut` rather than `&`: exclusivity. A gate cannot be reborrowed into two concurrent write
//! paths, so "one write in flight" is the borrow checker's invariant too.

/// Zero-sized capability every binding-table write verb requires.
///
/// Two mints, one per production door — [`WriteGate::for_run_loop`] and
/// [`WriteGate::for_unpublished_scope`] — both `pub(in crate::machine)`, plus a `#[cfg(test)]`
/// mint for fixtures. Neither is reachable from outside the machine:
///
/// ```compile_fail
/// // `for_unpublished_scope` is `pub(in crate::machine)`, so nothing outside the machine can mint
/// // a gate — and every binding-table write verb needs one.
/// let _gate = koan::machine::WriteGate::for_unpublished_scope();
/// ```
pub struct WriteGate {
    _private: (),
}

impl WriteGate {
    /// The run loop's door: a write into a published scope's table, performed between steps with no
    /// koan frame on the stack — which is what lets the write verbs take firm `borrow_mut`s.
    pub(in crate::machine) fn for_run_loop() -> Self {
        WriteGate { _private: () }
    }

    /// The construction door: a write into a scope no other node can reach, because the scope is
    /// still being built. A site mints this gate only while it owns that construction, or receives
    /// it as a parameter from the `machine`-side caller that does.
    pub(in crate::machine) fn for_unpublished_scope() -> Self {
        WriteGate { _private: () }
    }

    /// Fixture mint. `#[cfg(test)]` so it cannot widen the production doors.
    #[cfg(test)]
    pub(crate) fn for_test() -> Self {
        WriteGate { _private: () }
    }
}
