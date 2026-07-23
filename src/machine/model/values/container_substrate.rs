//! [`ContainerSubstrate<C>`] — a region-resident container payload `C` plus its [`SubstrateMemos`]:
//! the three construction-time memos (contains-borrows, copy-cost, borrows-home) every container
//! carries. [`RecordSubstrate`] (`C = Record<Held>`) is the field substrate behind a record value;
//! [`ListSubstrate`] (`C = Vec<Held>`) is the element substrate behind a list value;
//! [`DictSubstrate`] (`C = hashbrown::HashMap<KKey, Held>`) is the entry substrate behind a dict
//! value. The wrapper is the pattern every later container conversion in this project copies; see
//! [design/value-substrates.md](../../../../design/value-substrates.md).

use hashbrown::HashMap;

use crate::machine::core::KoanRegion;
use crate::machine::model::types::Record;

use super::{Held, KKey, KObject};

/// A container payload's three construction-time memos — computed once, in the same pass, and never
/// recomputed by a walk. Rides with the payload it summarizes, so the memos can never go stale
/// relative to their own cells.
pub struct SubstrateMemos {
    /// Set iff some transitive cell is a region-borrow leaf (closure, module, non-splice-free
    /// expression) or a still-`Rc` composite (dict/tagged/wrapped — carrying no memo of its own to
    /// consult, so the bit is conservative there until each container converts). A nested `Record`
    /// or `List` contributes its own memoized bit. Memoized in the same pass that computes the
    /// element/field-type join; never recomputed by a walk.
    contains_borrows: bool,
    /// Exact cost in bytes of totally rebuilding this container's reachable structure at a
    /// destination brand. `u64::MAX` (saturated): unpriceable — some transitive cell is a
    /// still-`Rc` composite (dict/tagged/wrapped) or a `KExpression`, which carry no memo of their
    /// own until their conversions ship. A nested `Record` or `List` contributes its own cost.
    copy_cost: u64,
    /// Whether some transitive borrow leaf points into this container's own home region. Exact when
    /// `copy_cost` is priced (leaf regions are O(1) reads at construction; nested records compose
    /// their own bits, co-resident by construction); conservatively `true` alongside an unpriceable
    /// cost.
    borrows_home: bool,
}

impl SubstrateMemos {
    /// Build from the three precomputed memos.
    pub(crate) fn new(contains_borrows: bool, copy_cost: u64, borrows_home: bool) -> Self {
        SubstrateMemos {
            contains_borrows,
            copy_cost,
            borrows_home,
        }
    }

    /// Derive all three memos in a single pass over the container's [`Held`] cells, reading `home`
    /// as the substrate's own region. Collapses the per-cell any/saturating-add/any rules
    /// (`held_contains_borrows` / `held_copy_cost` / `held_borrows_home`) into one fold over a
    /// running triple.
    pub(crate) fn compute<'a, 'b>(
        cells: impl Iterator<Item = &'b Held<'a>>,
        home: &KoanRegion,
    ) -> Self
    where
        'a: 'b,
    {
        let (contains_borrows, copy_cost, borrows_home) = cells.fold(
            (false, 0u64, false),
            |(contains_borrows, copy_cost, borrows_home), cell| {
                (
                    contains_borrows || held_contains_borrows(cell),
                    copy_cost.saturating_add(held_copy_cost(cell, home)),
                    borrows_home || held_borrows_home(cell, home),
                )
            },
        );
        SubstrateMemos::new(contains_borrows, copy_cost, borrows_home)
    }

    /// Whether some transitive cell is a region-borrow leaf or a still-`Rc` composite — see the
    /// field's own doc.
    pub fn contains_borrows(&self) -> bool {
        self.contains_borrows
    }

    /// Exact cost in bytes of totally rebuilding this container at a destination brand, or
    /// `u64::MAX` when unpriceable — see the field's own doc.
    pub fn copy_cost(&self) -> u64 {
        self.copy_cost
    }

    /// Whether some transitive borrow leaf points into this container's own home region — see the
    /// field's own doc.
    pub fn borrows_home(&self) -> bool {
        self.borrows_home
    }
}

/// A region-resident container: its payload `C` (the cells) plus the [`SubstrateMemos`] computed
/// over them at construction. Immutable after construction — no interior cell writes exist anywhere
/// in the runtime, so a region-resident substrate needs no mutation story. Born only through the
/// branded door
/// ([`FoldingBrand::alloc_substrate_folded`](crate::machine::core::FoldingBrand::alloc_substrate_folded)),
/// which stores the substrate and hands back a co-located borrow — the cells and the memoized bits
/// ride together, so the memos can never go stale relative to their own cells.
pub struct ContainerSubstrate<C> {
    cells: C,
    memos: SubstrateMemos,
}

impl<C> ContainerSubstrate<C> {
    /// Build from a payload and its precomputed memos. The pass that derives the memos from the
    /// cells (see [`SubstrateMemos::compute`]) lives at the construction site, not here — this is
    /// the door's own plain constructor.
    pub(crate) fn new(cells: C, memos: SubstrateMemos) -> Self {
        ContainerSubstrate { cells, memos }
    }

    /// The container payload — the cells this substrate resides.
    pub fn cells(&self) -> &C {
        &self.cells
    }

    /// The three construction-time memos over the cells.
    pub fn memos(&self) -> &SubstrateMemos {
        &self.memos
    }

    /// Whether some transitive cell is a region-borrow leaf or a still-`Rc` composite — see
    /// [`SubstrateMemos::contains_borrows`].
    pub fn contains_borrows(&self) -> bool {
        self.memos.contains_borrows()
    }

    /// Exact cost in bytes of totally rebuilding this container at a destination brand, or
    /// `u64::MAX` when unpriceable — see [`SubstrateMemos::copy_cost`].
    pub fn copy_cost(&self) -> u64 {
        self.memos.copy_cost()
    }

    /// Whether some transitive borrow leaf points into this container's own home region — see
    /// [`SubstrateMemos::borrows_home`].
    pub fn borrows_home(&self) -> bool {
        self.memos.borrows_home()
    }
}

/// The field substrate a record value borrows — [`ContainerSubstrate<C>`] at `C = Record<Held>`.
pub(crate) type RecordSubstrate<'a> = ContainerSubstrate<Record<Held<'a>>>;

impl<'a> ContainerSubstrate<Record<Held<'a>>> {
    /// The field record — declaration-ordered, order-blind equality (see [`Record`]).
    pub fn fields(&self) -> &Record<Held<'a>> {
        self.cells()
    }
}

/// The element substrate a list value borrows — [`ContainerSubstrate<C>`] at `C = Vec<Held>`.
/// A list is positional, so the payload is a bare `Vec` (unlike [`RecordSubstrate`]'s order-blind
/// [`Record`]).
pub(crate) type ListSubstrate<'a> = ContainerSubstrate<Vec<Held<'a>>>;

impl<'a> ContainerSubstrate<Vec<Held<'a>>> {
    /// The element slice — positional, index-ordered.
    pub fn elements(&self) -> &Vec<Held<'a>> {
        self.cells()
    }
}

/// The entry substrate a dict value borrows — [`ContainerSubstrate<C>`] at
/// `C = hashbrown::HashMap<KKey, Held>`. Keys are the concrete scalar [`KKey`]; values are [`Held`]
/// cells (an object or a first-class type). The table is frozen at construction (last-wins dedup
/// happens in the transient construction map) and never written again; iteration order is arbitrary
/// (unspecified, as the prior `Rc<HashMap>` layout was). The block is a default-`Global` heap
/// allocation the wrapper owns and drops at region death — a `hashbrown` table so a future
/// region-`Allocator` swap is a zero-payload-churn change.
pub(crate) type DictSubstrate<'a> = ContainerSubstrate<HashMap<KKey, Held<'a>>>;

impl<'a> ContainerSubstrate<HashMap<KKey, Held<'a>>> {
    /// The entry table — arbitrary iteration order; look up by key with `entries().get(key)`.
    pub fn entries(&self) -> &HashMap<KKey, Held<'a>> {
        self.cells()
    }
}

/// The per-cell contains-borrows rule shared by every substrate constructor (see
/// [`RecordSubstrate`] / [`ListSubstrate`]): a type-channel cell never borrows a region; a scalar
/// owns its data outright; `KFunction` / `Module` are borrow leaves; a `KExpression` borrows iff it
/// carries a splice; a nested `Record` or `List` contributes its own memoized bit (never re-walked);
/// every other still-`Rc` composite is conservative `true` until it, too, converts to a substrate.
fn held_contains_borrows(h: &Held<'_>) -> bool {
    match h {
        Held::Type(_) | Held::UnresolvedType(_) => false,
        Held::Object(o) => match o {
            KObject::Number(_) | KObject::KString(_) | KObject::Bool(_) | KObject::Null => false,
            KObject::KFunction(_) | KObject::Module(_) => true,
            KObject::KExpression(e) => !e.is_splice_free(),
            KObject::Record(substrate, _) => substrate.contains_borrows(),
            KObject::List(substrate, _) => substrate.contains_borrows(),
            KObject::Dict(..) | KObject::Tagged { .. } | KObject::Wrapped { .. } => true,
        },
    }
}

/// One [`Held`] cell's flat size in bytes — the [`Held`] discriminant plus its owned payload,
/// counted for a cost memo. `Held` is invariant in its lifetime, so its size is lifetime-independent.
fn held_flat_size() -> u64 {
    std::mem::size_of::<Held<'static>>() as u64
}

/// The per-cell copy-cost rule shared by every substrate constructor (see [`RecordSubstrate`] /
/// [`ListSubstrate`]): a cell contributes the bytes of totally rebuilding it at a destination brand.
/// A type cell or a scalar costs one flat [`Held`]; a `KString` adds its byte length; a `KFunction` /
/// `Module` is a borrow leaf that rides the transfer and rebuilds nothing (**0**); a nested `Record`
/// or `List` contributes its own memoized cost; a `KExpression` and every still-`Rc` composite are
/// unpriceable (`u64::MAX`), carrying no cost memo of their own until each converts to a substrate.
fn held_copy_cost(h: &Held<'_>, _home: &KoanRegion) -> u64 {
    match h {
        Held::Type(_) | Held::UnresolvedType(_) => held_flat_size(),
        Held::Object(o) => match o {
            KObject::Number(_) | KObject::Bool(_) | KObject::Null => held_flat_size(),
            KObject::KString(s) => held_flat_size().saturating_add(s.len() as u64),
            KObject::KFunction(_) | KObject::Module(_) => 0,
            KObject::Record(substrate, _) => substrate.copy_cost(),
            KObject::List(substrate, _) => substrate.copy_cost(),
            KObject::KExpression(_)
            | KObject::Dict(..)
            | KObject::Tagged { .. }
            | KObject::Wrapped { .. } => u64::MAX,
        },
    }
}

/// The per-cell borrows-home rule shared by every substrate constructor (see [`RecordSubstrate`] /
/// [`ListSubstrate`]): whether this cell's transitive borrow leaf points into `home`, the
/// substrate's own region. A type cell and a scalar borrow nothing (false); a `KFunction` / `Module`
/// leaf borrows home iff its captured scope's region is `home` (an O(1) region read); a nested
/// `Record` or `List` composes its own memoized bit (co-resident by construction); a `KExpression`
/// is conservative on a carried splice; every still-`Rc` composite is conservatively `true` until it,
/// too, converts to a substrate.
fn held_borrows_home(h: &Held<'_>, home: &KoanRegion) -> bool {
    match h {
        Held::Type(_) | Held::UnresolvedType(_) => false,
        Held::Object(o) => match o {
            KObject::Number(_) | KObject::KString(_) | KObject::Bool(_) | KObject::Null => false,
            KObject::KFunction(f) => std::ptr::eq(f.captured_scope().region(), home),
            KObject::Module(m) => std::ptr::eq(m.child_scope().region(), home),
            KObject::Record(substrate, _) => substrate.borrows_home(),
            KObject::List(substrate, _) => substrate.borrows_home(),
            KObject::KExpression(e) => !e.is_splice_free(),
            KObject::Dict(..) | KObject::Tagged { .. } | KObject::Wrapped { .. } => true,
        },
    }
}
