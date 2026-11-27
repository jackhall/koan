//! The crate's tests: a toy domain of its own, hand-written cases, and the laws.
//!
//! [`toy`] supplies the domain (terms of nested lists over atoms and names, and unions of them,
//! probed for their depth, leaf, or which name they are), its answer function, the plain-data
//! texts the generators draw, the bases and site order computed from a text alone, and the growth
//! oracle that composes terms by brute force. [`harness`] reads a `Bound` as a load would.
//! [`cases`] pins the design's examples and each refusal, [`edges`] the calls on sites, the
//! `AnswersName` edges on arms, through calls and on sites, their knots, the demands each puts on
//! Tarjan's stack and the evaluations kept, [`onward`] the onward calls into a statement's node,
//! and [`rebuilt`] handles, budgets, view tables and
//! replays. [`reference`] evaluates from scratch, sharing no code with
//! the crate's evaluation, and [`laws`] states each law of [the crate](../README.md#laws) as a
//! property over generated texts and bindings, with the same graph answered from scratch under
//! the reference's bindings as the oracle.

mod cases;
mod edges;
mod harness;
mod laws;
mod onward;
#[cfg(feature = "perf")]
mod perf;
mod rebuilt;
mod reference;
mod toy;

/// The case count for one property-test module, as a share of the proptest default.
///
/// `ProptestConfig::default().cases` reads `PROPTEST_CASES`, so a verification tier sets that one
/// variable and every module moves with it. No module drops below 64 cases.
pub(crate) fn case_share(numerator: u32, denominator: u32) -> u32 {
    (proptest::prelude::ProptestConfig::default().cases * numerator / denominator).max(64)
}
