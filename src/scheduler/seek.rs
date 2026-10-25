//! How a run cell finds the arena node it runs under: what its asker hands the drain, and what the
//! drain does with it before the cell is born.
//!
//! An asker that calls a body hands a [`Seek`]: the arena node the callee was born under, and, for
//! a body keyed on its bindings whose content the asker read and the load could have declared, the
//! key, the image of the types its binding carries, and the [`Ask`] a miss settles. The drain looks
//! the key up in the [arena](super::arena) under that parent: a live hit is stamped, and a miss
//! relays the image's nodes into the agent tier, settles the body through the program's
//! [`Settles`] door, and evicts to the budget once the frame borrows it. So a frame is born under its
//! settlement and parks for nothing. A frame with no key runs under the parent itself. A tail hop
//! of the same callee under a live settlement stamps it too. A parent that is no longer live
//! births the frame cold, under the root. An `EVAL` of code composed at run time runs under its
//! composition, found by the code's syntax digest or built through the same door. Any other cell
//! runs under what its asker runs under. See
//! [build/README.md § Steps and the drain](../build/README.md#steps-and-the-drain).

use smallvec::SmallVec;

use crate::knot::ComposedRoot;
use crate::memory::{Bump, ProgramBrand};
use crate::scope::{Composition, Facts, Segment, Settlement, Slot};
use crate::type_lattice::KType;
use crate::values::{ArenaRef, BindingImage, ContentDigest, SettlementKey};

/// What an asker hands the drain to find a frame's arena node: the one the callee was born under,
/// and the settlement under it the frame seeks, if any.
pub struct Seek<'graph> {
    pub parent: ArenaRef,
    /// A keyed body's key under `parent`, its binding's image, and what a miss settles: `None` for
    /// a body that keys no settlement or a call whose content at a keyed name the asker could not
    /// read or the load does not hold, whose frame runs under `parent`.
    pub settlement: Option<(SettlementKey, BindingImage<'graph>, Ask)>,
}

/// What a miss settles: the callee's body, and what each name its key lists binds there, by slot.
pub struct Ask {
    pub body: Segment,
    pub bound: SmallVec<[(Slot, Asked); 1]>,
}

/// What a keyed name binds: a type, or a module's carried type over the closed tree its bare
/// member's content digest names, `retyped` where that type is not the member's own.
#[derive(Clone, Copy, Debug)]
pub enum Asked {
    Type(KType),
    Module {
        carried: KType,
        content: ContentDigest,
        retyped: bool,
    },
}

/// The door a miss settles through, which the program hands the graph at load: it walks the
/// asked body under the asked binding over `parent`, the layers the new settlement rests on, and
/// lays what it decides in `bump`. `None` where the binding names a module whose tree no layer
/// holds, so nothing settles. `witness` is that `'graph` outlives the bump.
pub trait Settles<'graph> {
    fn settle<'a>(
        &self,
        bump: &'a Bump,
        parent: Facts<'a>,
        ask: &Ask,
        witness: &'a &'graph (),
    ) -> Option<Settlement<'a>>;

    /// The composition of the code `root` holds, composed at run time: its shapes built alone and
    /// walked with nothing bound, laid through `brand`, its walk interning through a chain laid
    /// in `bump`, where a term of each type its facts name past the agent tier is laid too.
    fn compose<'a>(
        &self,
        brand: ProgramBrand<'a>,
        bump: &'a Bump,
        root: ComposedRoot<'graph>,
        witness: &'a &'graph (),
    ) -> Composition<'a>;
}

/// The door of a graph that runs no program: every settlement holds no fact.
pub(crate) struct Unsettled;

impl<'graph> Settles<'graph> for Unsettled {
    fn settle<'a>(
        &self,
        _: &'a Bump,
        parent: Facts<'a>,
        _: &Ask,
        _: &'a &'graph (),
    ) -> Option<Settlement<'a>> {
        Some(Settlement::empty(parent.top()))
    }

    fn compose<'a>(
        &self,
        _: ProgramBrand<'a>,
        _: &'a Bump,
        _: ComposedRoot<'graph>,
        _: &'a &'graph (),
    ) -> Composition<'a> {
        unreachable!("a graph that runs no program runs no composed code")
    }
}

/// What a request's cell runs under, resolved by the drain at its creation.
pub(super) enum Lending<'graph> {
    /// What its asker runs under, as an evaluation, a block or a hand-off does.
    Parent,
    /// A call's frame: the arena node its [`Seek`] finds or builds.
    Call(Seek<'graph>),
    /// The settlement an `ArenaRef` names, as a tail hop of the same callee runs under its
    /// frame's, stamped; the root, cold, where that one is no longer live.
    Under(ArenaRef),
    /// The composition of the code `root` holds, found by its syntax digest and stamped, or built.
    Compose(ComposedRoot<'graph>),
}
