//! The static pass's two entries: the load's walk, whose write half writes each fact into its
//! shape's cell, and a resumption's, whose write half keeps what it re-runs and relays it into the
//! `Settled` it lays ([`store`](super::store), [`resume`](super::resume)). Both run the one walk
//! ([`Pass::walk`]).

use crate::knot::KBuiltins;
use crate::memory::{BumpAllocator, BumpVec, Writer, bump_set, bump_table};
use crate::scope::{BodyShape, Relay, Resumption, Settled, ShapeError};
use crate::type_lattice::Habitat;
use crate::values::Chain;

use super::Pass;
use super::resume::Resume;
use super::store::{Load, Write};

impl<'p, 'x, 'graph: 'p, 'tier: 'p, 'pair, 'reg> Pass<'p, 'x, 'graph, 'tier, 'pair, 'reg> {
    fn new(
        builtins: &'x KBuiltins<'graph, 'graph>,
        types: &'x Chain<'tier, 'pair, 'reg, 'x>,
        scratch: BumpAllocator<'p>,
        over: Option<Resumption<'p, 'p>>,
        channel: Option<Settled<'p>>,
        write: &'p dyn Write<'p, 'graph>,
    ) -> Self {
        Pass {
            builtins,
            types,
            scratch,
            chain: BumpVec::new_in(scratch),
            unfilled: false,
            trees: bump_table(scratch),
            reading: BumpVec::new_in(scratch),
            typing: BumpVec::new_in(scratch),
            recursive: bump_set(scratch),
            views: bump_table(scratch),
            steps: bump_table(scratch),
            rebuilds: bump_table(scratch),
            modules: bump_table(scratch),
            paths: BumpVec::new_in(scratch),
            sites: BumpVec::new_in(scratch),
            composites: bump_table(scratch),
            edges: BumpVec::new_in(scratch),
            escaping: bump_set(scratch),
            bound_at_binder: bump_set(scratch),
            holders: BumpVec::new_in(scratch),
            module_mints: bump_table(scratch),
            heading: false,
            components: BumpVec::new_in(scratch),
            scopes: std::cell::Cell::new(BumpVec::new_in(scratch)),
            over,
            channel,
            write,
        }
    }

    /// Type `root` and every shape nested in it, from the walk's outermost gathering, which no
    /// rule closes.
    fn walk(&mut self, root: &'graph BodyShape<'graph>) -> Result<(), ShapeError<'p>> {
        self.open();
        self.push(root, true);
        self.open_paths(root);
        let visited = self.visit(0, None);
        self.close_paths(root);
        self.chain.pop();
        visited
    }
}

/// Give every value expression and value binder of `root`, and of every shape nested in it, a
/// static type, and narrow every keyworded use's candidates; refuse a use no candidate can admit.
/// Each fact is written into its shape's cell through `writer`.
pub(in crate::dispatch) fn statics<'graph>(
    root: &'graph BodyShape<'graph>,
    builtins: &KBuiltins<'graph, 'graph>,
    types: &Chain<'graph, 'graph, 'graph, '_>,
    writer: Writer<'graph, 'graph>,
    scratch: BumpAllocator<'_>,
) -> Result<(), ShapeError<'graph>> {
    let relay = Relay::new(writer, types, types, scratch);
    let load = Load { relay: &relay };
    let mut pass = Pass::new(builtins, types, scratch, None, None, &load);
    let visited = pass.walk(root);
    // A site's class reads tail edges from every body, so the classes are fixed last.
    if visited.is_ok() {
        pass.cache();
    }
    visited.map_err(|error| relay.shape_error(error))
}

/// Resume the static pass over `root` under `over`'s binding, beside `channel`, what the type
/// channel's resumption under it settled ([`resume`](super::resume)). The walk lays in `scratch`
/// and interns through `work`; what it settles is relayed out through `output`, the types it
/// names adopted into `output`'s chain, and laid there with `channel` as one `Settled`. A use the
/// binding leaves no candidate refuses as the load would.
pub(in crate::dispatch) fn resume_statics<'graph, 'out, 'w, 'from, H: Habitat<'from>>(
    root: &'graph BodyShape<'graph>,
    builtins: &KBuiltins<'graph, 'graph>,
    work: &Chain<'_, '_, '_, '_>,
    output: &Relay<'out, 'w, 'from, H>,
    scratch: BumpAllocator<'_>,
    over: Resumption<'_, 'out>,
    channel: Settled<'out>,
) -> Result<&'out Settled<'out>, ShapeError<'out>> {
    // Laid in the walk's scratch, which outlives the walk that borrows it.
    let resume = scratch.alloc(Resume::new(over.everything, scratch));
    let mut pass = Pass::new(builtins, work, scratch, Some(over), Some(channel), resume);
    if !pass.typed_shape(root) {
        pass.walk(root).map_err(|error| output.shape_error(error))?;
    }
    Ok(resume.settled(channel, output))
}
