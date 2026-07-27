//! The [`WriteGate`] capability: the compile-time proof that a binding-table write is happening at
//! one of the two doors allowed to perform one.
//!
//! Every write verb on [`Bindings`](super::Bindings) — and every `*_direct` door on
//! [`Scope`](crate::machine::core::Scope) that reaches one — takes a `&mut WriteGate`. The gate is
//! a zero-sized token with no public constructor and no `Clone`, minted only inside
//! `crate::machine`, so a builtin body (`crate::builtins` is a sibling of `crate::machine`, not a
//! descendant) cannot produce one and therefore cannot name a write verb at all. A published
//! scope's table is mutated by the run loop and nothing else, and that is now a resolution failure
//! rather than a convention.
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
    /// The run loop's door: the apply loop that drains a step's [`WriteOp`](super::WriteOp)s after
    /// its continuation returns, the error-path placeholder clears in the same loop, and the
    /// submission-channel placeholder stamp dispatch performs when it submits a binder. All three
    /// run with no koan frame on the stack, which is what lets the write verbs take firm
    /// `borrow_mut`s.
    pub(in crate::machine) fn for_run_loop() -> Self {
        WriteGate { _private: () }
    }

    /// The construction door: a write into a scope no other node can reach — the run-global root at
    /// startup, a not-yet-published per-call scope (parameters, MATCH / TRY `it`), a freshly minted
    /// child scope before its body dispatches, an ascription view before the view module captures
    /// it. Every such site either owns the scope's construction outright or receives this gate as a
    /// parameter from the `machine`-side caller that does.
    pub(in crate::machine) fn for_unpublished_scope() -> Self {
        WriteGate { _private: () }
    }

    /// Fixture mint. `#[cfg(test)]` so it cannot widen the production doors.
    #[cfg(test)]
    pub(crate) fn for_test() -> Self {
        WriteGate { _private: () }
    }
}
