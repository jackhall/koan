//! Operator-group registry record. A set of chainable operators is declared
//! together and registered — one shared [`OperatorGroup`], pointer-shared — under
//! every nonempty subset of the group's operators (the per-group powerset,
//! singletons included, so a same-operator run like `a + b + c`, whose deduped probe
//! is just `+`, still resolves). A chain's operator probe (the sorted-joined unique
//! operators of a `Slot (Keyword Slot)+` expression) looks the group up in one
//! hashmap hit; a cross-group mix — which nothing registers — simply misses.
//!
//! A group's record is its member set plus one [`ReductionMode`] describing how a
//! recognized run of its operators reduces. The record is **lifetime-free**: a pairwise
//! group's combiner is a [`Combiner`] *name*, not a resolved function, so the chain reducer
//! resolves it at the use site through the ordinary scope walk and the record borrows no
//! region. That is what lets `RegionBrand::alloc_operator_group` stay a trivial no-op-gate
//! door.
//!
//! Registry lookup is innermost-wins
//! ([`Scope::resolve_operator_group_with_chain`](crate::machine::core::Scope::resolve_operator_group_with_chain)):
//! the builtin comparison / additive / multiplicative groups seeded into the run-global root
//! by `register_builtin_operator_groups` (`src/builtins/arithmetic.rs`) are found last, so they
//! are chaining defaults a declaring scope may override — a registry hit carries no operand
//! types and so cannot type-gate the way a function bucket does. The `OP` / `GROUP` binder that
//! lets user modules populate the registry is the declaration surface owned by the
//! user-defined-operator-modules roadmap item. This module is the record and lookup key only.

use std::collections::HashSet;

/// Which way a fold nests a run of more than two operands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FoldDirection {
    /// `a ⊙ b ⊙ c` ⇒ `(a ⊙ b) ⊙ c`.
    Left,
    /// `a ⊙ b ⊙ c` ⇒ `a ⊙ (b ⊙ c)`.
    Right,
}

/// The binary combiner a [`ReductionMode::Pairwise`] group folds its pair results through,
/// held as a **name** rather than a resolved function: the chain reducer synthesizes a call
/// and the ordinary scope walk resolves it at the use site, which is what keeps
/// [`OperatorGroup`] lifetime-free (no region borrow, no reaching-tier allocation door).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Combiner {
    /// A keyword token (the builtin comparison group's `AND`): the reducer synthesizes a
    /// keyworded call `left <kw> right`.
    Keyword(String),
    /// A value binding naming a two-argument function: the reducer synthesizes an
    /// Identifier-head call, resolved through the `FunctionValueCall` lane in the scope the
    /// chain is written in.
    Name(String),
}

/// How a recognized run of this group's operators reduces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReductionMode {
    /// The whole operand run is handed to one body as a single list operand.
    Unary,
    /// A binary body folds the run left-associated: `a - b - c` ⇒ `(a - b) - c`.
    FoldLeft,
    /// Right-associated: `a ^ b ^ c` ⇒ `a ^ (b ^ c)`.
    FoldRight,
    /// Each adjacent pair dispatches through its own operator's binary body; the pair
    /// results fold through the group's combiner in the declared direction.
    Pairwise {
        combiner: Combiner,
        direction: FoldDirection,
    },
}

/// A declared set of mutually chainable operators plus the mode a recognized run of
/// them reduces by. Pointer-shared: every powerset key the registering module
/// installs points at the same region-allocated record, so a subset used in one
/// expression resolves to the same group as any other subset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperatorGroup {
    /// The full declared member set (keywords), not the probed subset.
    members: HashSet<String>,
    mode: ReductionMode,
}

impl OperatorGroup {
    pub fn new(members: HashSet<String>, mode: ReductionMode) -> Self {
        OperatorGroup { members, mode }
    }

    /// The mode a recognized run of this group's operators reduces by.
    pub fn mode(&self) -> &ReductionMode {
        &self.mode
    }

    /// Every member operator keyword. Order is unspecified (hash-set iteration).
    pub fn member_operators(&self) -> impl Iterator<Item = &str> {
        self.members.iter().map(|s| s.as_str())
    }

    /// True iff every operator in `probe_operators` is a member of this group — the
    /// admission gate for a chain whose probe hit this group's registry slot. A probe
    /// subset that names a non-member is a cross-group mix that must miss.
    pub fn covers(&self, probe_operators: &[&str]) -> bool {
        probe_operators.iter().all(|op| self.members.contains(*op))
    }
}

/// Sorts (byte order), dedups, and joins `operators` with `" "`. This is the same key
/// `operator_probe_for` (`src/machine/model/ast.rs`) computes from a real parse, so a
/// group's registration keys and a chain's probe always agree on one key shape.
pub fn probe_key(operators: &[&str]) -> String {
    let mut sorted: Vec<&str> = operators.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    sorted.join(" ")
}
