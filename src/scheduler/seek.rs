//! How a run cell finds the arena node it runs under: what its asker hands the drain, and what the
//! drain does with it before the cell is born.
//!
//! An asker that calls a body hands a [`Seek`]: the arena node the callee was born under, and, for
//! a body keyed on its bindings whose content the asker read and the load could have declared, the
//! image of the types its binding carries beside its request ([`Sought`]): the key, the [`Ask`] a
//! miss settles, and those types. Where the frame lays its own level from that request — a frame
//! with a region of its own, whose callee's body can birth a callable — the drain hands the
//! request back to the frame's first step once the frame is born under the settlement
//! ([`Step::sought`](super::Step::sought)). The drain looks the key up in the
//! [arena](super::arena) under that parent: a live hit is stamped, and a miss
//! settles the body through the program's [`Settles`] door — the image's nodes relayed into the
//! agent tier under the root, into the walk's own chain under a composition — and evicts to the
//! budget once the frame borrows it. A miss under a composition binding a module of other composed
//! code pins that code's composition; where it is gone, the frame is born under the parent
//! wanting it, and its first step copies the module's syntax and asks again
//! ([`Lending::Pinning`]). So a frame is born under its
//! settlement and parks for nothing. A frame with no key runs under the parent itself. A tail hop
//! of the same callee under a live settlement stamps it too. A parent that is no longer live
//! births the frame cold, under the root, where its first step replays its callee's stamp
//! ([`Lending::Replay`]): each level found or settled under the one before, held until the frame
//! borrows its own, so the frame is born warm. An `EVAL` of code composed at run time runs under its
//! composition, found by the code's syntax digest. The books hold digests, not syntax, so a miss
//! births the cell under the root, and its first step — holding the code value, or the callee whose
//! body record carries its root — copies the syntax and bounces into itself with the copy
//! ([`Compose::Owned`]), which the drain builds the composition over. Any other cell runs under
//! what its asker runs under. See
//! [build/README.md § Steps and the drain](../build/README.md#steps-and-the-drain).

use crate::memory::Bump;
pub use crate::scope::{Ask, Asked, Sought};
use crate::scope::{ComposedSyntax, Composition, Facts, Settlement};
use crate::values::{ArenaRef, BindingImage, Chain, ContentDigest};
use smallvec::SmallVec;

/// What an asker hands the drain to find a frame's arena node: the one the callee was born under,
/// the settlement under it the frame seeks, if any, and whether the frame's first step is handed
/// that request back to lay its own level.
pub struct Seek<'graph> {
    pub parent: ArenaRef,
    /// A keyed body's binding image under `parent`, beside its request — its key, what a miss
    /// settles and the types its binding carries: `None` for a body that keys no settlement or a
    /// call whose content at a keyed name the asker could not read or the load does not hold, whose
    /// frame runs under `parent`.
    pub settlement: Option<(BindingImage<'graph>, Sought)>,
    /// Whether the drain hands the request back to the frame's first step, which lays its level:
    /// set for a frame whose callee's body can birth a callable and whose asker laid no level.
    pub hands_back: bool,
}

/// The door a miss settles through, which the program hands the graph at load: it walks the
/// asked body under the asked binding over `parent`, the layers the new settlement rests on, and
/// lays what it decides in `bump`. A module's tree is found in those layers, the load's, or a
/// `pinned` composition's. `None` where the binding names a module whose tree none holds, so
/// nothing settles; `rest` is handed the walk's chain, where it has one, once the binding
/// resolves, before the walk reads its types, so nothing rests for a settlement that is not
/// built. `witness` is that `'graph`
/// outlives the bump.
pub trait Settles<'graph> {
    fn settle<'a>(
        &self,
        bump: &'a Bump,
        parent: Facts<'a>,
        pinned: &[&'a Composition<'a>],
        ask: &Ask,
        rest: &mut dyn FnMut(Option<&Chain<'_, '_, '_, '_>>),
        witness: &'a &'graph (),
    ) -> Option<Settlement<'a>>;

    /// The composition of `syntax`, code composed at run time copied into storage of its own: its
    /// shapes built alone over the copy and walked with nothing bound, laid in that storage, its
    /// walk interning through a chain laid in `bump`, where a term of each type its facts name
    /// past the agent tier is laid too.
    fn compose<'a>(
        &self,
        syntax: &'a ComposedSyntax,
        bump: &'a Bump,
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
        _: &[&'a Composition<'a>],
        _: &Ask,
        rest: &mut dyn FnMut(Option<&Chain<'_, '_, '_, '_>>),
        _: &'a &'graph (),
    ) -> Option<Settlement<'a>> {
        rest(None);
        Some(Settlement::empty(parent.top()))
    }

    fn compose<'a>(
        &self,
        _: &'a ComposedSyntax,
        _: &'a Bump,
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
    /// The composition of code composed at run time, found by its syntax digest and stamped, or
    /// built over the syntax a bounce copied.
    Compose(Compose),
    /// A call's frame, as [`Call`](Self::Call), once the composition a module its settlement binds
    /// was born of is found or built over the syntax its first step copied.
    Pinning(Seek<'graph>, ComposedSyntax),
    /// A call's frame born warm: each level of its callee's stamp found or settled under the one
    /// before, from the asker's arena node, the last its own seek.
    Replay(Vec<Replayed<'graph>>),
}

/// One level of a replay: the seek of a settlement of a callee's chain, its parent the level
/// before, beside a copy of the syntax of each composition it pins, which the drain finds by
/// digest or builds before it seeks.
pub struct Replayed<'graph> {
    pub seek: Seek<'graph>,
    pub pins: SmallVec<[ComposedSyntax; 1]>,
}

/// What a cell running composed code asks of the drain: its composition by syntax digest — the
/// root, where none is live, so the cell's first step bounces — or built over the syntax that
/// bounce copied, where a sibling has not built it meanwhile.
pub(super) enum Compose {
    Find(ContentDigest),
    Owned(ComposedSyntax),
}
