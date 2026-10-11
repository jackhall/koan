//! Where a value's types live: the type lattice's tiers, reached from a habitat half.
//!
//! Each habitat half — a cell's region, or its scratch habitat — keeps one [`Branch`] of the type
//! lattice as its header ([`TypeHeader`]), laid by the first door that interns into the half and
//! released with it. A [`Types`] chain is built per step over the frozen
//! [`Root`](crate::type_lattice::Root) the load built and a [`Half`]: the writer that lays a value
//! beside the header door of the same half, so a value door that takes both at one brand interns
//! its type into the branch of the habitat it lays the value in. During the load the chain writes
//! the root builder instead ([`Half::Load`]).
//!
//! `memory` names no type tier, so the habitat a chain writes is assembled here, where values pair
//! writers with types. See [README.md § Crossing](README.md#crossing) for how a crossing makes a
//! value's types resolve in its destination.

use crate::memory::{Header, LaysRuns, Writer, reattachable};
use crate::type_lattice::{Branch, Habitat, Own, RootBuilder, TypeChain};

/// The header family a koan graph keeps per habitat half: the half's branch of the type lattice.
pub struct TypeHeader;

reattachable!(TypeHeader => &'cell Branch<'cell>);

/// The tier a chain interns into, beside the runs it lays content through: a habitat half's
/// writer and header door at one brand, or the root builder during the load.
#[derive(Clone, Copy)]
pub enum Half<'graph, 'cell>
where
    'graph: 'cell,
{
    /// A cell region or a scratch habitat: its writer beside its header door.
    Habitat {
        writer: Writer<'cell>,
        header: Header<'graph, 'cell, TypeHeader>,
    },
    /// The load: the root builder the load's chain writes, at `'cell = 'graph`.
    Load(&'cell RootBuilder<'cell>),
}

impl<'graph, 'cell> Half<'graph, 'cell> {
    /// The half `writer` lays into, whose header door is `header`.
    pub fn of(writer: Writer<'cell>, header: Header<'graph, 'cell, TypeHeader>) -> Self {
        Half::Habitat { writer, header }
    }
}

impl<'cell> LaysRuns<'cell> for Half<'_, 'cell> {
    fn lay<T>(self, len: usize, fill: impl FnMut(usize) -> T) -> &'cell [T] {
        match self {
            Half::Habitat { writer, .. } => writer.fill(len, fill),
            Half::Load(builder) => builder.lay(len, fill),
        }
    }

    fn text(self, text: &str) -> &'cell str {
        match self {
            Half::Habitat { writer, .. } => writer.text(text),
            Half::Load(builder) => builder.text(text),
        }
    }
}

impl<'cell> Habitat<'cell> for Half<'_, 'cell> {
    fn own(self) -> Option<Own<'cell>> {
        match self {
            Half::Habitat { header, .. } => header.get().map(Own::Branch),
            Half::Load(builder) => Some(Own::Builder(builder)),
        }
    }

    fn own_or_lay(self) -> Own<'cell> {
        match self {
            Half::Habitat { writer, header } => Own::Branch(header.get_or_lay(|| {
                let mut branch = Some(Branch::new(self));
                &writer.fill(1, |_| branch.take().expect("a run of one fills once"))[0]
            })),
            Half::Load(builder) => Own::Builder(builder),
        }
    }
}

/// The chain a step, a build or the load reads and interns types through, writing the half its
/// writer lays into.
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
///     step: Step<'_, 'graph, '_, '_, '_, KBundle>,
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
///     step: Step<'_, 'graph, '_, '_, '_, KBundle>,
/// ) -> Action<'graph, KBundle> {
///     let types = step.types();
///     let items = [Value::Number(1.0)].into_iter();
///     let _: &List<'_> = List::new(step.scratch_writer(), items, &types, &Bump::new());
///     step.done()
/// }
/// ```
pub type Types<'graph, 'cell> = TypeChain<'cell, Half<'graph, 'cell>>;
