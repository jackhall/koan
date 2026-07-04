//! The value-relocation hook [`relocate_carried`] now lives in
//! [`machine::core`](crate::machine::core::relocate_carried) — alongside [`RegionBrand`] — so the
//! [`DepTerminal`](crate::machine::core::kfunction::action::DepTerminal) relocation named in the
//! builtin-`Action` currency reaches it without depending on the execute layer. Re-exported here so
//! the execute-side callers (dep delivery, single-poll) keep their `super::lift::relocate_carried`
//! path, and the relocation-behavior tests stay co-located with the workload that runs it.

pub(in crate::machine::execute) use crate::machine::core::relocate_carried;

#[allow(unused_imports)]
use crate::machine::core::RegionBrand;

#[cfg(test)]
mod tests;
