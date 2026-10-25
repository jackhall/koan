//! How a run cell finds the arena node it runs under: what its asker hands the drain, and what the
//! drain does with it before the cell is born.
//!
//! An asker that calls a body hands a [`Seek`]: the arena node the callee was born under, and, for
//! a body keyed on its bindings whose content the asker read and the load could have declared, the
//! key, the image of the types its binding carries, and the [`Ask`] a miss settles. The drain looks
//! the key up in the [arena](super::arena) under that parent: a live hit is stamped, and a miss
//! settles the body through the program's [`Settles`] door — the image's nodes relayed into the
//! agent tier under the root, into the walk's own chain under a composition — and evicts to the
//! budget once the frame borrows it. A miss under a composition binding a module of other composed
//! code pins that code's composition; where it is gone, the frame is born under the parent
//! wanting it, and its first step copies the module's syntax and asks again
//! ([`Lending::Pinning`]). So a frame is born under its
//! settlement and parks for nothing. A frame with no key runs under the parent itself. A tail hop
//! of the same callee under a live settlement stamps it too. A parent that is no longer live
//! births the frame cold, under the root. An `EVAL` of code composed at run time runs under its
//! composition, found by the code's syntax digest. The books hold digests, not syntax, so a miss
//! births the cell under the root, and its first step — holding the code value, or the callee whose
//! body record carries its root — copies the syntax and bounces into itself with the copy
//! ([`Compose::Owned`]), which the drain builds the composition over. Any other cell runs under
//! what its asker runs under. See
//! [build/README.md § Steps and the drain](../build/README.md#steps-and-the-drain).

use smallvec::SmallVec;

use crate::memory::Bump;
use crate::scope::{ComposedSyntax, Composition, Facts, Segment, Settlement, Slot};
use crate::type_lattice::KType;
use crate::values::{ArenaRef, BindingImage, Chain, ContentDigest, SettlementKey};

/// What an asker hands the drain to find a frame's arena node: the one the callee was born under,
/// and the settlement under it the frame seeks, if any.
pub struct Seek<'graph> {
    pub parent: ArenaRef,
    /// A keyed body's key under `parent`, its binding's image, and what a miss settles: `None` for
    /// a body that keys no settlement or a call whose content at a keyed name the asker could not
    /// read or the load does not hold, whose frame runs under `parent`.
    pub settlement: Option<(SettlementKey, BindingImage<'graph>, Ask)>,
}

/// What a miss settles: the callee's body, and what each name its key lists binds there.
pub struct Ask {
    pub body: Segment,
    pub bound: SmallVec<[Asked; 1]>,
}

/// What the keyed name at `slot` binds: a type, or a module's carried type over the closed tree
/// its bare member's content digest names, `retyped` where that type is not the member's own. A
/// module born of code composed at run time names its composition by syntax digest, where a miss
/// under a composition finds its tree if no layer below does, and pins it. The slot rides in each
/// arm, in the padding the digests leave, so an ask stays small enough to hand on by value.
#[derive(Clone, Copy, Debug)]
pub enum Asked {
    Type {
        slot: Slot,
        named: KType,
    },
    Module {
        slot: Slot,
        carried: KType,
        content: ContentDigest,
        retyped: bool,
        composition: Option<ContentDigest>,
    },
}

impl Asked {
    /// The slot of the keyed name this binds.
    pub fn slot(&self) -> Slot {
        match *self {
            Asked::Type { slot, .. } | Asked::Module { slot, .. } => slot,
        }
    }
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
}

/// What a cell running composed code asks of the drain: its composition by syntax digest — the
/// root, where none is live, so the cell's first step bounces — or built over the syntax that
/// bounce copied, where a sibling has not built it meanwhile.
pub(super) enum Compose {
    Find(ContentDigest),
    Owned(ComposedSyntax),
}
