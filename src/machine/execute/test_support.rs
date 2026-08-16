//! The one door `builtins::test_support::extract_terminal` reaches through to read a delivered
//! edge terminal out of the scheduler: [`KoanRuntime::scheduler`] is scoped to this module tree
//! (`pub(in crate::machine::execute)`), so the read has to happen from inside it rather than at
//! `builtins::test_support`'s own call site.

use crate::machine::{DeliveredCarried, FrameCoverage, KError, Scope};
use crate::scheduler::EdgeId;

use super::KoanRuntime;

/// Duplicate `edge`'s delivered terminal — leaving the edge's own resident intact, releasing and
/// consuming nothing — and lift it into an envelope reachable at `scope`'s lifetime.
///
/// The resident is re-branded under `scope`'s own owner (the same brand the run loop mints for a
/// step's deps) via [`Scheduler::edge_resident_duplicate`](crate::scheduler::Scheduler::edge_resident_duplicate),
/// then lifted back into an envelope: a test that goes on to copy the value out (as
/// `extract_terminal` does) needs the envelope's own coverage as its holder-rule proof, which
/// `read_edge_result_with`'s scoped-open read cannot hand back.
pub(crate) fn edge_delivered<'a>(
    runtime: &KoanRuntime<'a>,
    edge: EdgeId,
    scope: &'a Scope<'a>,
) -> Result<DeliveredCarried, KError> {
    let resident = runtime.scheduler().edge_resident_duplicate(edge)?;
    let coverage = FrameCoverage::of(
        scope
            .region_owner()
            .upgrade()
            .expect("a live scope reference implies a live region owner"),
    );
    Ok(scope.lift_spliced(&resident.brand_with(&coverage)))
}
