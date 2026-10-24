//! Where a value's types live: the type lattice's tiers, reached from a habitat half.
//!
//! Each habitat half — a cell's region, or its scratch habitat — keeps one [`Branch`] of the type
//! lattice in its header, laid by the first door that interns into the half and released with it.
//! A [`Half`] reaches the header only through a door projected to the branch's slot
//! ([`BranchSlot`], whose family is [`BranchCell`]): any header family that projects to it serves,
//! and a half that never interns lays no header at all. A [`Types`] chain is built per step over the
//! [`TypeTiers`] the load built and a [`Half`]: the writer that lays a
//! value beside the projected header door of the same half, so a value door that takes both at one
//! brand interns its type into the branch of the habitat it lays the value in. During the load the
//! chain writes the root builder instead ([`Half::Load`]), and the agent chain writes the agent
//! tier ([`Half::Agent`]): where a settlement's binding and an invariant site's types rest, so
//! every later frame of the program finds them.
//!
//! `memory` names no type tier, so the habitat a chain writes is assembled here, where values pair
//! writers with types. See [README.md § Crossing](README.md#crossing) for how a crossing makes a
//! value's types resolve in its destination.

use std::cell::Cell;
use std::marker::PhantomData;

use crate::memory::{Header, LaysRuns, Projected, Projects, Writer, reattachable};
use crate::type_lattice::{Agent, Branch, Frozen, Habitat, Halves, Own, RootBuilder, TypeChain};

/// The two tiers every chain of the program's one agent reads above its own halves: the frozen
/// load, and the agent tier, which outlives every cell and is released with the program.
#[derive(Clone, Copy)]
pub struct TypeTiers<'graph> {
    pub frozen: &'graph Frozen<'graph>,
    pub agent: &'graph Agent<'graph>,
}

impl<'graph> TypeTiers<'graph> {
    /// A chain over `half`, reading the tiers first.
    pub fn chain<'tier, H: Habitat<'tier>>(self, half: H) -> TypeChain<'tier, H>
    where
        'graph: 'tier,
    {
        TypeChain::new(self.frozen, self.agent, half)
    }

    /// A scratch chain over `half`, reading the tiers, then `region`'s branch, before its own.
    pub fn scratch<'reg: 'tier, 'tier, H: Habitat<'tier>, R: Habitat<'reg>>(
        self,
        region: R,
        half: H,
    ) -> TypeChain<'tier, Halves<'reg, 'tier, H, R>>
    where
        'graph: 'reg,
    {
        TypeChain::scratch(self.frozen, self.agent, region, half)
    }

    /// The agent chain: the frozen load, then the agent tier, which it writes. Paired to the writer
    /// at `'pair`, whose values take their types from it by digest alone.
    pub fn resting<'pair, 'door>(self) -> Chain<'graph, 'pair, 'graph, 'door> {
        TypeChain::resting(
            self.frozen,
            Halves::alone(Half::Agent(self.agent, PhantomData)),
        )
    }
}

/// A half's branch slot, as its header keeps it: empty until the first door that interns into
/// the half lays the branch.
pub type BranchSlot<'cell> = &'cell Cell<Option<&'cell Branch<'cell>>>;

/// The family of a [`BranchSlot`]: what a header family projects to for `values`, which reads a
/// half's header through that part alone.
pub struct BranchCell;

reattachable!(BranchCell => BranchSlot<'cell>);

/// The tier a chain interns into, beside the runs it lays content through: a habitat half's
/// writer and header door at one brand, the root builder during the load, or the agent tier.
///
/// `'pair` is the pairing brand: the brand of the writer the chain serves, which a value door
/// pairs its writer with. A half built by [`of`](Half::of) or [`load`](Half::load) is paired to
/// its own brand; a step pairs a caching body's scratch halves to its region writer
/// ([`paired`](Half::paired)), since a value holds its type by digest and never references a node.
///
/// `'door` is the brand of the step that minted the half's doors, so a half cannot outlive it.
#[derive(Clone, Copy)]
pub enum Half<'cell, 'pair, 'door> {
    /// A cell region or a scratch habitat: its writer beside its header door, projected to the
    /// branch slot. `own_brand` is set where the half is paired to its own brand
    /// ([`of`](Half::of)), so a door beside it lays its value in the half it interns into.
    Habitat {
        writer: Writer<'cell, 'door>,
        branch: Projected<'cell, 'door, BranchSlot<'cell>>,
        pair: Pairing<'pair>,
        own_brand: bool,
    },
    /// The load: the root builder the load's chain writes, at `'cell = 'graph`.
    Load(&'cell RootBuilder<'cell>, Pairing<'pair>),
    /// The agent tier the agent chain writes, at `'cell = 'graph`, which outlives every half.
    Agent(&'cell Agent<'cell>, Pairing<'pair>),
}

/// The pairing brand a [`Half`] carries, invariant so a chain paired to one brand is never read
/// as paired to another.
pub type Pairing<'pair> = PhantomData<fn(&'pair ()) -> &'pair ()>;

impl<'cell, 'door> Half<'cell, 'cell, 'door> {
    /// The half `writer` lays into, whose header door is `header`, paired to its own brand. It
    /// keeps the door projected to the branch slot, and lays nothing.
    pub fn of<'graph: 'cell, H: Projects<'graph, Projection = BranchCell>>(
        writer: Writer<'cell, 'door>,
        header: Header<'graph, 'cell, 'door, H>,
    ) -> Self {
        Half::Habitat {
            writer,
            branch: header.project(),
            pair: PhantomData,
            own_brand: true,
        }
    }

    /// The load's half over `builder`, paired to its own brand.
    pub fn load(builder: &'cell RootBuilder<'cell>) -> Self {
        Half::Load(builder, PhantomData)
    }
}

impl<'cell, 'pair, 'door> Half<'cell, 'pair, 'door> {
    /// The half `writer` lays into, paired to the writer at `'pair` that a caching body's value
    /// doors take beside a chain over it.
    pub(crate) fn paired<'graph: 'cell, H: Projects<'graph, Projection = BranchCell>>(
        writer: Writer<'cell, 'door>,
        header: Header<'graph, 'cell, 'door, H>,
    ) -> Self {
        Half::Habitat {
            writer,
            branch: header.project(),
            pair: PhantomData,
            own_brand: false,
        }
    }

    /// Whether a value door beside this half lays its value where the half interns: a half paired
    /// to its own brand, or the load's, whose types all land in the load.
    pub(crate) fn interns_where_it_lays(&self) -> bool {
        match self {
            Half::Habitat { own_brand, .. } => *own_brand,
            Half::Load(..) => true,
            Half::Agent(..) => false,
        }
    }
}

impl<'cell> LaysRuns<'cell> for Half<'cell, '_, '_> {
    fn lay<T>(self, len: usize, fill: impl FnMut(usize) -> T) -> &'cell [T] {
        match self {
            Half::Habitat { writer, .. } => writer.fill(len, fill),
            Half::Load(builder, _) => builder.lay(len, fill),
            Half::Agent(agent, _) => agent.lay(len, fill),
        }
    }

    fn text(self, text: &str) -> &'cell str {
        match self {
            Half::Habitat { writer, .. } => writer.text(text),
            Half::Load(builder, _) => builder.text(text),
            Half::Agent(agent, _) => agent.text(text),
        }
    }
}

impl<'cell> Habitat<'cell> for Half<'cell, '_, '_> {
    fn own(self) -> Option<Own<'cell>> {
        match self {
            Half::Habitat { branch, .. } => branch.get()?.get().map(Own::Branch),
            Half::Load(builder, _) => Some(Own::Builder(builder)),
            Half::Agent(agent, _) => Some(Own::Agent(agent)),
        }
    }

    fn own_or_lay(self) -> Own<'cell> {
        match self {
            Half::Habitat { writer, branch, .. } => {
                let slot = branch.get_or_lay(writer);
                if let Some(laid) = slot.get() {
                    return Own::Branch(laid);
                }
                let mut branch = Some(Branch::new(self));
                let laid = &writer.fill(1, |_| branch.take().expect("a run of one fills once"))[0];
                slot.set(Some(laid));
                Own::Branch(laid)
            }
            Half::Load(builder, _) => Own::Builder(builder),
            Half::Agent(agent, _) => Own::Agent(agent),
        }
    }
}

/// The chain a step, a build or the load reads and interns types through, writing the half its
/// writer lays into. It holds that half's doors, so it is branded `'door` to the step that built it.
///
/// A value door takes its writer and its chain at one brand, so the types it interns land in the
/// habitat it lays the value in:
///
/// ```
/// use koan::memory::Bump;
/// use koan::program::KBundle;
/// use koan::scheduler::{Action, Step};
/// use koan::values::{List, Value};
///
/// fn scratch_list<'graph>(
///     step: Step<'_, 'graph, '_, '_, '_, '_, KBundle>,
/// ) -> Action<'graph, KBundle> {
///     let types = step.scratch_types();
///     let items = [Value::Number(1.0)].into_iter();
///     let _: &List<'_> = List::new(step.scratch_writer(), items, &types, &Bump::new());
///     step.done()
/// }
/// ```
///
/// A door handed one half's writer and another half's chain does not compile:
///
/// ```compile_fail
/// use koan::memory::Bump;
/// use koan::program::KBundle;
/// use koan::scheduler::{Action, Step};
/// use koan::values::{List, Value};
///
/// fn scratch_list<'graph>(
///     step: Step<'_, 'graph, '_, '_, '_, '_, KBundle>,
/// ) -> Action<'graph, KBundle> {
///     let types = step.types();
///     let items = [Value::Number(1.0)].into_iter();
///     let _: &List<'_> = List::new(step.scratch_writer(), items, &types, &Bump::new());
///     step.done()
/// }
/// ```
///
/// A caching body's chain lays in a scratch tier but is paired to the region writer, so a door
/// takes the two together:
///
/// ```
/// use koan::memory::Bump;
/// use koan::program::KBundle;
/// use koan::scheduler::{Action, Step};
/// use koan::values::{List, Value};
///
/// fn cached_list<'graph>(
///     step: Step<'_, 'graph, '_, '_, '_, '_, KBundle>,
/// ) -> Action<'graph, KBundle> {
///     let types = step.caching_types();
///     let items = [Value::Number(1.0)].into_iter();
///     let _: &List<'_> = List::new(step.writer(), items, &types, &Bump::new());
///     step.done()
/// }
/// ```
///
/// and the same chain beside the scratch writer, whose brand it is not paired to, does not
/// compile:
///
/// ```compile_fail
/// use koan::memory::Bump;
/// use koan::program::KBundle;
/// use koan::scheduler::{Action, Step};
/// use koan::values::{List, Value};
///
/// fn cached_list<'graph>(
///     step: Step<'_, 'graph, '_, '_, '_, '_, KBundle>,
/// ) -> Action<'graph, KBundle> {
///     let types = step.caching_types();
///     let items = [Value::Number(1.0)].into_iter();
///     let _: &List<'_> = List::new(step.scratch_writer(), items, &types, &Bump::new());
///     step.done()
/// }
/// ```
pub type Types<'cell, 'door> = Chain<'cell, 'cell, 'cell, 'door>;

/// A chain whose tiers sit at `'tier`, paired to the writer at `'pair`: what a value door takes
/// beside a writer at `'pair`. A caching body's chains lay into a scratch tier, so their tier brand
/// differs from the region writer they are paired to. A scratch chain reads a region at `'reg`
/// between the load's tiers and its own, holding the region's half by value ([`Halves`]), so building
/// one lays nothing; the region's half takes the chain's pairing brand, which it lays no value
/// under. A chain over a half alone holds none, and its `'reg` is its `'tier`. The chain holds its
/// halves' doors, so it is branded `'door` to the step that built it.
pub type Chain<'tier, 'pair, 'reg, 'door> =
    TypeChain<'tier, Halves<'reg, 'tier, Half<'tier, 'pair, 'door>, Half<'reg, 'pair, 'door>>>;
