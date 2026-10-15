//! The **habitat header**: one embedder value per habitat half — each cell region, each scratch
//! habitat — that the substrate stores and never reads. Its family is the graph's fourth embedder
//! type, [`HabitatHeader`], and defaults to [`NoHeader`], which keeps nothing. See
//! [../README.md](../README.md) § The habitat header.
//!
//! The slots rest in [`region`](crate::region): a [`Region`](crate::region::Region) carries its
//! region half's, so a seal keeps it with the region and a splice or absorb keeps the absorber's,
//! and the table's habitat carries the scratch half's beside the scratch bump, cleared with every
//! reset. Each holds an erased `Copy` value in a `Cell`, so a slot adds no drop glue and no door
//! ever lends a reference into it.
//!
//! Two doors reach a slot, each at a brand it may only be read back at:
//!
//! - [`Header`], onto one of a step's own halves, at that half's brand: the step's `'here` and
//!   `'scratch`, or a build closure's own brand for the half it writes, handed out beside the
//!   [`Writer`](crate::Writer) into the same half. Invariant, like the writer, so a writer and a
//!   header of different halves never meet in one call. Like the writer, it also carries the
//!   brand of the step that minted it, so it cannot outlive that step.
//! - [`HomeHeader`], an operand's home header at the operand's brand, which a build reads through
//!   a closure and never writes. The graph resolves it from a region the operand's reach names;
//!   [`StepContext::alloc_into`](crate::StepContext::alloc_into) documents the rule.

use std::cell::Cell;
use std::marker::PhantomData;

use crate::carrier::Active;
use crate::reattach::{Erased, Reattachable};
use crate::region::DoorBrands;

/// The habitat header family: a reattachable family whose erased form is `Copy`, kept one per
/// habitat half and handed back at that half's brand. Implemented for every such family, so an
/// embedder declares a header family with [`reattachable!`](crate::reattachable) and nothing more.
///
/// Not [`DropFree`](crate::DropFree)-bound: `Copy` already rules out drop glue, and the slot is a
/// field beside a region's bump rather than bytes in it.
pub trait HabitatHeader<'graph>: Reattachable<'graph, At<'graph>: Copy> {}

impl<'graph, T: Reattachable<'graph, At<'graph>: Copy>> HabitatHeader<'graph> for T {}

/// The header family of a graph whose embedder keeps nothing per habitat. Its form is `()`.
pub struct NoHeader;

crate::reattachable!(NoHeader => ());

/// One habitat half's header slot: empty until a door lays a header in it.
pub(crate) type HeaderSlot<'graph, H> = Cell<Option<Erased<'graph, H>>>;

/// The door onto one habitat half's header slot, at that half's brand.
///
/// Invariant in `'cell`: a header is laid and read back at the brand of the half it describes, so
/// no door at a shorter brand can lay a value that a longer one reads. A step's own doors are at
/// `'here` and `'scratch`; a build's is at the brand its closure is quantified over:
///
/// ```compile_fail
/// # use std::cell::Cell;
/// # use cellgraph::{CellGraph, NoDelivery, NoScratch, Verdict, reattachable};
/// # struct Work;
/// # reattachable!(Work => String);
/// # struct Table;
/// # reattachable!(Table => &'cell Cell<&'cell u32>);
/// # let mut graph: CellGraph<'static, Work, NoScratch, NoDelivery, 1, Table> =
/// #     CellGraph::new(1, |_| Verdict::Pin);
/// # let cell = graph.create(None).unwrap();
/// graph
///     .enter(cell, |context| {
///         let mut header = context.scratch_header();
///         header = context.header();
///         header.get();
///     })
///     .unwrap();
/// ```
///
/// The same rebinding at one brand compiles:
///
/// ```
/// # use std::cell::Cell;
/// # use cellgraph::{CellGraph, NoDelivery, NoScratch, Verdict, reattachable};
/// # struct Work;
/// # reattachable!(Work => String);
/// # struct Table;
/// # reattachable!(Table => &'cell Cell<&'cell u32>);
/// # let mut graph: CellGraph<'static, Work, NoScratch, NoDelivery, 1, Table> =
/// #     CellGraph::new(1, |_| Verdict::Pin);
/// # let cell = graph.create(None).unwrap();
/// graph
///     .enter(cell, |context| {
///         let mut header = context.scratch_header();
///         header = context.scratch_header();
///         header.get();
///     })
///     .unwrap();
/// ```
///
/// A header door is a door too, and parks nowhere either:
///
/// ```compile_fail
/// # use cellgraph::*;
/// # struct Work;
/// # reattachable!(Work => ());
/// struct Parked;
/// reattachable!(Parked => Option<Header<'static, 'cell, 'cell, NoHeader>>);
/// let mut graph: CellGraph<'static, Parked> = CellGraph::new(2, |_| Verdict::Pin);
/// let cell = graph.create(None).unwrap();
/// graph.enter(cell, |context| context.store_successor(Some(context.header()))).unwrap();
/// ```
pub struct Header<'graph, 'cell, 'door, H: HabitatHeader<'graph>>
where
    'graph: 'cell,
{
    slot: &'cell HeaderSlot<'graph, H>,
    _brand: DoorBrands<'cell, 'door>,
}

impl<'graph, 'cell, 'door, H: HabitatHeader<'graph>> Clone for Header<'graph, 'cell, 'door, H>
where
    'graph: 'cell,
{
    fn clone(&self) -> Self {
        *self
    }
}

impl<'graph, 'cell, 'door, H: HabitatHeader<'graph>> Copy for Header<'graph, 'cell, 'door, H> where
    'graph: 'cell
{
}

impl<'graph, 'cell, 'door, H: HabitatHeader<'graph>> Header<'graph, 'cell, 'door, H>
where
    'graph: 'cell,
{
    /// The door onto `slot`, at the brand of the half it belongs to. Crate-private: the caller
    /// asserts that `'cell` is that half's brand, which [`get`](Self::get) relies on, and picks
    /// `'door`: every step door passes its `'step`.
    pub(crate) fn new(slot: &'cell HeaderSlot<'graph, H>) -> Self {
        Header {
            slot,
            _brand: PhantomData,
        }
    }

    /// The half's header at its brand, or `None` while nothing has laid one.
    pub fn get(self) -> Option<H::At<'cell>> {
        let header = self.slot.get()?;
        // SAFETY: the slot belongs to one habitat half and this door is at that half's brand: its
        // own cell's, or a borrower's `'lent` while the half is lent and its owner undisposed.
        // Every value in it was put there by `get_or_lay` through a door onto the same half, at
        // one of those brands of some step or build. Its referents are of four kinds: the half's
        // own bytes; `'graph`, which outlives the graph — a door onto `Storage` among them;
        // storage the half's cell holds (a region's hold set, monotone for its life, including
        // what a placement into it minted before its build ran); and, for the scratch half, the
        // write home's region, which `'here: 'scratch` lets a scratch value name. A door onto a
        // habitat is none of these: it names the half's table entry, which moves when the table
        // grows, and it carries the brand of the step that minted it, which outlives no habitat
        // brand, so no value laid at one holds such a door. A region's chunks do not move while it
        // lives, its slot leaves with it, and no recycled region keeps one (`Region::new`,
        // `Region::is_untouched`); the scratch slot is cleared at every reset. So every referent
        // outlives `'cell`. `Header` is invariant, and every brand
        // it is handed out at is quantified by `enter` or by a build and nameable nowhere outside
        // it, which discharges the invariant-family condition.
        Some(unsafe { header.reattach::<'cell>() })
    }

    /// The half's header, laying the one `lay` builds where none is. Nothing clears or replaces a
    /// laid header; a `lay` that itself lays through a copy of this door is overwritten here.
    pub fn get_or_lay(self, lay: impl FnOnce() -> H::At<'cell>) -> H::At<'cell> {
        // No local holds an `H::At<'cell>`: moving one would have rustc prove it `Copy` from the
        // family's `At<'graph>: Copy` bound, which unifies `'cell` with `'graph`.
        if self.slot.get().is_none() {
            self.slot.set(Some(Erased::erase(lay())));
        }
        self.get().expect("a header was just laid")
    }
}

/// An operand's home header, at the operand's brand `'b`: `'cell` for a pinned operand, `'severed`
/// for a copied one. Empty where the home has none or is the destination's own write home.
///
/// Read-only: its one door, [`read`](Self::read), hands the header to a closure. There is no door
/// that lays or sets one:
///
/// ```compile_fail
/// # use std::cell::Cell;
/// # use cellgraph::{HomeHeader, reattachable};
/// # struct Table;
/// # reattachable!(Table => &'cell Cell<&'cell u32>);
/// fn lay<'b>(home: HomeHeader<'static, 'b, Table>, table: &'b Cell<&'b u32>) {
///     home.get_or_lay(|| table);
/// }
/// ```
///
/// The read compiles:
///
/// ```
/// # use std::cell::Cell;
/// # use cellgraph::{HomeHeader, reattachable};
/// # struct Table;
/// # reattachable!(Table => &'cell Cell<&'cell u32>);
/// fn read<'b>(home: HomeHeader<'static, 'b, Table>) -> Option<&'b u32> {
///     home.read(|header, _| header.into_value().get())
/// }
/// ```
pub struct HomeHeader<'graph, 'b, H: HabitatHeader<'graph>>
where
    'graph: 'b,
{
    header: Option<Erased<'graph, H>>,
    _brand: PhantomData<fn(&'b ()) -> &'b ()>,
}

impl<'graph, 'b, H: HabitatHeader<'graph>> Clone for HomeHeader<'graph, 'b, H>
where
    'graph: 'b,
{
    fn clone(&self) -> Self {
        *self
    }
}

impl<'graph, 'b, H: HabitatHeader<'graph>> Copy for HomeHeader<'graph, 'b, H> where 'graph: 'b {}

impl<'graph, 'b, H: HabitatHeader<'graph>> HomeHeader<'graph, 'b, H>
where
    'graph: 'b,
{
    /// The home header an operand crosses with. Crate-private: the caller asserts the header's
    /// referents outlive `'b`, which [`read`](Self::read) relies on.
    pub(crate) fn new(header: Option<Erased<'graph, H>>) -> Self {
        HomeHeader {
            header,
            _brand: PhantomData,
        }
    }

    /// Read the home's header, or `None` where the home has none or is the destination's own write
    /// home.
    ///
    /// `read` receives the header at a brand `'h` it is generic over, outliving the operand's
    /// (`'h: 'b`, stated by the witness). So it can take anything out at `'b`, and write nothing
    /// in but the header's own contents and `'graph` borrows: a destination borrow, or another
    /// home's contents, is valid at no brand the closure can count on.
    ///
    /// ```compile_fail
    /// # use std::cell::Cell;
    /// # use cellgraph::{HomeHeader, reattachable};
    /// # struct Table;
    /// # reattachable!(Table => &'cell Cell<&'cell u32>);
    /// fn write<'b>(home: HomeHeader<'static, 'b, Table>, destination: &'b u32) {
    ///     home.read(|header, _| header.into_value().set(destination));
    /// }
    /// ```
    ///
    /// Nor can one home's contents be written into another's header:
    ///
    /// ```compile_fail
    /// # use std::cell::Cell;
    /// # use cellgraph::{HomeHeader, reattachable};
    /// # struct Table;
    /// # reattachable!(Table => &'cell Cell<&'cell u32>);
    /// fn cross<'b>(first: HomeHeader<'static, 'b, Table>, second: HomeHeader<'static, 'b, Table>) {
    ///     first.read(|one, _| {
    ///         second.read(|other, _| other.into_value().set(one.value().get()));
    ///     });
    /// }
    /// ```
    ///
    /// Nested reads that only read compile:
    ///
    /// ```
    /// # use std::cell::Cell;
    /// # use cellgraph::{HomeHeader, reattachable};
    /// # struct Table;
    /// # reattachable!(Table => &'cell Cell<&'cell u32>);
    /// fn both<'b>(
    ///     first: HomeHeader<'static, 'b, Table>,
    ///     second: HomeHeader<'static, 'b, Table>,
    /// ) -> Option<(&'b u32, &'b u32)> {
    ///     first
    ///         .read(|one, _| second.read(|other, _| (one.value().get(), other.value().get())))
    ///         .flatten()
    /// }
    /// ```
    pub fn read<R>(
        self,
        read: impl for<'h> FnOnce(Active<'graph, 'h, H>, &'b &'h &'graph ()) -> R,
    ) -> Option<R> {
        let header = self.header?;
        Some(read(Active::anchored(header), &&&()))
    }
}
