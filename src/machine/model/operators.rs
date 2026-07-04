//! Operator-group registry record. A user module declares a set of chainable
//! operators together — fixing their relative precedence and associativity — and
//! registers one shared [`OperatorGroup`] under every size-≥2 subset of the group's
//! operators (the per-group powerset). A chain's operator probe (the sorted-joined
//! unique operators of a `Slot (Keyword Slot)+` expression) looks the group up in one
//! hashmap hit; a cross-group mix — which nothing registers — simply misses.
//!
//! This is the substrate the fold pre-pass reads: it climbs the flat operator key
//! using each operator's [`OperatorEntry::tier`] and [`OperatorEntry::associativity`]
//! to decide grouping. Populating the registry is the `OP` binder (declaration
//! surface, owned by the user-defined-operator-modules roadmap item); this module is
//! the record and lookup key only.

use std::collections::HashMap;

/// How a run of one operator at a given tier associates when the fold pre-pass climbs
/// the flat key. `Left` (`a - b - c` ⇒ `(a - b) - c`) is the arithmetic default;
/// `Right` (`a ^ b ^ c` ⇒ `a ^ (b ^ c)`) suits exponent-shaped operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Associativity {
    Left,
    Right,
}

/// One operator's fold metadata within its group. `tier` is the precedence rank —
/// higher binds tighter; operators in one group are declared together, fixing their
/// relative tiers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperatorEntry {
    pub tier: u32,
    pub associativity: Associativity,
}

/// A declared set of mutually chainable operators plus each member's fold metadata.
/// Pointer-shared: every powerset key the registering module installs points at the
/// same region-allocated record, so a subset used in one expression resolves to the
/// same group as any other subset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperatorGroup {
    /// Operator keyword → its fold metadata. The members are the full declared set,
    /// not the probed subset.
    members: HashMap<String, OperatorEntry>,
}

impl OperatorGroup {
    pub fn new(members: HashMap<String, OperatorEntry>) -> Self {
        OperatorGroup { members }
    }

    pub fn entry(&self, operator: &str) -> Option<&OperatorEntry> {
        self.members.get(operator)
    }

    /// Every member operator keyword. Order is unspecified (hashmap iteration).
    pub fn member_operators(&self) -> impl Iterator<Item = &str> {
        self.members.keys().map(|s| s.as_str())
    }

    /// True iff every operator in `probe_operators` is a member of this group — the
    /// admission gate for a chain whose probe hit this group's registry slot. A probe
    /// subset that names a non-member is a cross-group mix that must miss.
    pub fn covers(&self, probe_operators: &[&str]) -> bool {
        probe_operators
            .iter()
            .all(|op| self.members.contains_key(*op))
    }
}
