//! Operator-group registry record. A user module declares a set of chainable
//! operators together and registers one shared [`OperatorGroup`] under every
//! size-≥2 subset of the group's operators (the per-group powerset). A chain's
//! operator probe (the sorted-joined unique operators of a `Slot (Keyword Slot)+`
//! expression) looks the group up in one hashmap hit; a cross-group mix — which
//! nothing registers — simply misses.
//!
//! A group's record is its member set plus one [`ReductionMode`] describing how a
//! recognized run of its operators reduces. Populating the registry is the `OP`
//! binder (declaration surface, owned by the user-defined-operator-modules roadmap
//! item); this module is the record and lookup key only.

use std::collections::HashSet;

/// How a recognized run of this group's operators reduces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReductionMode {
    /// The whole operand run is handed to one body as a single list operand.
    Unary,
    /// A binary body folds the run left-associated: `a - b - c` ⇒ `(a - b) - c`.
    FoldLeft,
    /// Right-associated: `a ^ b ^ c` ⇒ `a ^ (b ^ c)`.
    FoldRight,
    /// Each adjacent pair dispatches through its own operator's binary body;
    /// the pair results fold left through the named combiner keyword.
    Pairwise { combiner: String },
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
