//! The type channel's two entries: the load's walk, whose write half writes each fact into its
//! shape's cell, and a resumption's, whose write half keeps what it re-runs and relays it into
//! the `Settled` it returns ([`store`](super::store)). Both run the one walk ([`Pass::walk`]).

use crate::memory::{BumpAllocator, BumpVec, Writer};
use crate::scope::{BodyShape, Builtins, ChainRelay, Relay, Resumption, Settled, ShapeError};
use crate::values::{Chain, Knotted};

use super::Pass;
use super::store::{Load, Resume, Write};

impl<'p, 'graph, 'cell, 'tier, 'pair, 'reg, X> Pass<'p, 'graph, 'cell, 'tier, 'pair, 'reg, X> {
    fn new(
        builtins: &'p Builtins<'cell, X>,
        types: &'p Chain<'tier, 'pair, 'reg, 'p>,
        scratch: BumpAllocator<'p>,
        over: Option<Resumption<'p, 'p>>,
        write: &'p dyn Write<'p, 'graph>,
    ) -> Self {
        Pass {
            builtins,
            types,
            scratch,
            chain: BumpVec::new_in(scratch),
            over,
            write,
        }
    }
}

/// Type `root`'s type channel and every shape nested in it, writing each result into its cell
/// through `writer`. A closed type that does not elaborate, or a `MATCH … WITH` arm set guarding
/// one type twice, refuses the load; inside a quote's code the refusal is kept on the code shape
/// instead.
pub fn type_channel<'graph, X: Knotted>(
    root: &'graph BodyShape<'graph>,
    builtins: &Builtins<'_, X>,
    types: &Chain<'graph, 'graph, 'graph, '_>,
    writer: Writer<'graph, 'graph>,
    scratch: BumpAllocator<'_>,
) -> Result<(), ShapeError<'graph>> {
    let relay = Relay::new(writer, types, types, scratch);
    let load = Load { relay: &relay };
    let mut pass = Pass::new(builtins, types, scratch, None, &load);
    pass.walk(root).map_err(|error| relay.shape_error(error))
}

/// Resume `root`'s type channel under `over`'s binding, once the load has typed it: the walk the
/// load runs, each bound name read as the closed type it binds, re-running only the cells whose
/// frontier waits on what it binds. The walk lays in `scratch` and interns through `output`'s
/// source chain; what it settles is relayed out through `output`, the types it names adopted into
/// `output`'s chain. A closed type that no longer elaborates refuses as the load would.
pub fn resume_channel<'graph, 'out, 'w, X: Knotted>(
    root: &'graph BodyShape<'graph>,
    over: Resumption<'_, 'out>,
    builtins: &Builtins<'_, X>,
    output: &ChainRelay<'out, 'w, '_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
) -> Result<Settled<'out>, ShapeError<'out>> {
    let work = output.source();
    // Laid in the walk's scratch, which outlives the walk that borrows it.
    let resume = scratch.alloc(Resume::new(over.everything, scratch));
    let mut pass = Pass::new(builtins, work, scratch, Some(over), resume);
    pass.walk(root).map_err(|error| output.shape_error(error))?;
    Ok(resume.settled(over.binding, output))
}
