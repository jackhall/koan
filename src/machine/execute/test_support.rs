//! [`KoanRuntime::scheduler`] is scoped `pub(in crate::machine::execute)`, so a test that needs a
//! delivered edge terminal cannot read one from its own call site — it reaches through here.

use crate::machine::{DeliveredCarried, FrameCoverage, KError, Scope};
use crate::scheduler::EdgeId;

use super::KoanRuntime;

/// Duplicate `edge`'s delivered terminal — leaving the edge's own resident intact, releasing and
/// consuming nothing — and lift it into an envelope reachable at `scope`'s lifetime.
///
/// A test that goes on to copy the value out needs the envelope's own coverage as its holder-rule
/// proof, which [`read_edge_result_with`](super::KoanRuntime::read_edge_result_with)'s scoped-open
/// read cannot hand back.
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
