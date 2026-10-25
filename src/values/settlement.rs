//! What the build arena's books are keyed and bounded by, and how code above the scheduler names
//! an arena node.
//!
//! A [`SettlementKey`] finds a settlement under its parent. A [`BindingImage`] is the types a
//! seek's keyed arguments carry that the frozen load and the agent tier lack, with a copy of their
//! nodes laid in the image's own bump from the asker's chain: what the drain relays into the agent tier
//! when it builds a settlement, so a frame under it finds them after the asker is gone. A
//! [`BuildBudget`] bounds the bytes of the arena nodes no run cell holds. An [`ArenaRef`] is how
//! code above the scheduler holds an arena node without naming a slot: only `values` and
//! `scheduler` open one. See [build/README.md](../build/README.md#references-between-frames-and-the-arena).

use std::cell::Cell;
use std::num::NonZeroUsize;

use self_cell::self_cell;
use smallvec::SmallVec;

use crate::memory::{Bump, LaysRuns};
use crate::type_lattice::{Branch, DeclaredType, Habitat, KType, Own, Parametric, TypeChain};

use super::{ContentDigest, Knotted, TypeTiers, Value};

/// What a parent finds a settlement by: the callee's segment and the content digests at the
/// names its key lists, hashed into one digest.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct SettlementKey(pub ContentDigest);

/// The types a settlement's binding carries that the frozen load and the agent tier lacked when
/// the asker read them, one entry per keyed name in the order its key digests them, beside a copy
/// of their nodes ([build/README.md](../build/README.md#references-between-frames-and-the-arena)). A handle
/// is a content digest, so the image outlives the values it was read from. A binding whose every
/// type already rests is imaged by nothing, so a seek that hits allocates nothing.
#[derive(Default)]
pub struct BindingImage<'graph>(Option<ImageNodes<'graph>>);

impl<'graph> BindingImage<'graph> {
    /// The image of `keyed`, its nodes read from `types`, the asker's chain, where `tiers` lacks
    /// them.
    pub fn of<'asker, H: Habitat<'asker>>(
        keyed: SmallVec<[KeyedTypes; 1]>,
        tiers: TypeTiers<'graph>,
        types: &TypeChain<'asker, H>,
    ) -> Self {
        let resting = tiers.resting();
        if (keyed.iter().flat_map(KeyedTypes::handles)).all(|kt| resting.resolves(kt)) {
            return BindingImage(None);
        }
        let owner = ImageOwner {
            bump: Bump::new(),
            tiers,
            keyed,
        };
        BindingImage(Some(ImageNodes::new(owner, |owner| {
            let bump = &owner.bump;
            let half = ImageHalf {
                bump,
                slot: bump.alloc(Cell::new(None)),
            };
            let image = TypeChain::new(owner.tiers.frozen, owner.tiers.agent, half);
            for kt in owner.handles() {
                image.adopt_from(types, kt);
            }
            half.slot.get()
        })))
    }

    /// Every handle the image holds the nodes of: each entry's own type, then the type it names,
    /// if any. None for a binding whose every type already rested.
    pub fn handles(&self) -> impl Iterator<Item = DeclaredType<Parametric>> + '_ {
        (self.0.iter()).flat_map(|nodes| nodes.borrow_owner().handles())
    }

    /// Make every handle the image holds resolve in `into`, laying from the image what it lacks.
    pub fn relay<'tier, H: Habitat<'tier>>(&self, into: &TypeChain<'tier, H>) {
        let Some(nodes) = &self.0 else {
            return;
        };
        nodes.with_dependent(|owner, branch| {
            for kt in owner.handles() {
                into.adopt(*branch, kt);
            }
        });
    }
}

/// What an image's nodes are laid in: their own bump, beside the tiers the chain that lays them
/// reads first and the keyed types they are the nodes of.
struct ImageOwner<'graph> {
    bump: Bump,
    tiers: TypeTiers<'graph>,
    keyed: SmallVec<[KeyedTypes; 1]>,
}

impl ImageOwner<'_> {
    fn handles(&self) -> impl Iterator<Item = DeclaredType<Parametric>> + '_ {
        (self.keyed.iter()).flat_map(KeyedTypes::handles)
    }
}

/// The branch an image laid its nodes in: `None` where every node was found in the tiers.
type ImageBranch<'a> = Option<&'a Branch<'a>>;

self_cell!(
    struct ImageNodes<'graph> {
        owner: ImageOwner<'graph>,
        #[not_covariant]
        dependent: ImageBranch,
    }
);

/// The habitat an image's chain lays into: its bump, and the slot its branch is laid in.
#[derive(Clone, Copy)]
struct ImageHalf<'a> {
    bump: &'a Bump,
    slot: &'a Cell<Option<&'a Branch<'a>>>,
}

impl<'a> LaysRuns<'a> for ImageHalf<'a> {
    fn lay<T>(self, len: usize, fill: impl FnMut(usize) -> T) -> &'a [T] {
        self.bump.lay(len, fill)
    }

    fn text(self, text: &str) -> &'a str {
        self.bump.text(text)
    }
}

impl<'a> Habitat<'a> for ImageHalf<'a> {
    fn own(self) -> Option<Own<'a>> {
        self.slot.get().map(Own::Branch)
    }

    fn own_or_lay(self) -> Own<'a> {
        Own::Branch(match self.slot.get() {
            Some(branch) => branch,
            None => {
                let branch = &*self.bump.alloc(Branch::new(self.bump));
                self.slot.set(Some(branch));
                branch
            }
        })
    }
}

/// What one keyed argument carries: its own type and, for a type value, the type it names.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct KeyedTypes {
    pub carried: DeclaredType<KType>,
    pub named: Option<KType>,
}

impl KeyedTypes {
    pub fn of<X: Knotted>(argument: &Value<'_, X>) -> Self {
        KeyedTypes {
            carried: argument.ktype(),
            named: match argument {
                Value::Type(type_value) => Some(type_value.handle()),
                _ => None,
            },
        }
    }

    /// Its own type, then the type it names, if any.
    fn handles(&self) -> impl Iterator<Item = DeclaredType<Parametric>> {
        std::iter::once(self.carried.into()).chain(self.named.map(Into::into))
    }
}

/// How many bytes the build arena's unheld arena nodes may hold: past it, a miss evicts the least
/// recently used unheld leaves.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BuildBudget {
    pub bytes: NonZeroUsize,
}

impl Default for BuildBudget {
    fn default() -> Self {
        BuildBudget {
            bytes: NonZeroUsize::new(4 << 20).expect("4 MiB is not zero"),
        }
    }
}

/// The arena node a function was born under: a settlement, by its slot and the slot's generation
/// then, or the root, the frozen load. Opaque above the scheduler, which alone opens one, so a step
/// can hand the drain only an arena node the scheduler named to it. A handle whose slot has since
/// been evicted names nothing.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ArenaRef(Option<(u32, u32)>);

impl ArenaRef {
    /// The root: the frozen load.
    pub const ROOT: ArenaRef = ArenaRef(None);

    /// The settlement at `slot` while the slot is at `generation`.
    pub(crate) fn of(slot: u32, generation: u32) -> Self {
        ArenaRef(Some((slot, generation)))
    }

    /// The slot and generation this names, or `None` for the root.
    pub(crate) fn opened(self) -> Option<(u32, u32)> {
        self.0
    }
}
