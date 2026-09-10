//! **Program storage**: the tier where program text and the raw AST live, outside the region
//! model's per-call tier and above even the run root. Its own module because the parser depends on
//! it and on nothing else here — a parsed AST is bumped into this storage, so `parse` names
//! [`ProgramBrand`] and never the frame lifecycle.

use std::marker::PhantomData;
use std::rc::Rc;

use super::region::{FrameStorage, FrameStorageExt, RegionBrand};
use super::substrate::RegionHost;

/// Stand up a fresh program storage. Created by
/// [`interpret_with_writer_path`](crate::machine::execute::interpret_with_writer_path) before the
/// run region and held for the whole run, so it is created first and released last.
///
/// Same species as [`run_root_storage`](super::region::run_root_storage) — a [`FrameStorage`] at
/// the eternal tier — which is what makes an expression whose parts live only here reach nothing:
/// [`RegionHost::is_eternal`] drives `needs_no_pin`, and the eternal rule filters such a member out
/// of every pin bundle and reach description with no special case anywhere. It never enters the
/// frame lifecycle or the scheduler: the wrapped storage is private and [`brand`](ProgramStorage::brand)
/// is the only capability the type exposes, so nothing outside this module can adopt it into a
/// [`Frame`](super::frame::Frame) or make it a resident's home region.
pub fn program_storage() -> ProgramStorage {
    ProgramStorage(RegionHost::fresh_eternal())
}

/// The host whose region an AST borrows. Its own type, not a [`FrameStorage`] alias, because
/// the property the AST's reach answers rest on is a property of *this host* rather than of the
/// eternal tier at large: the run root is eternal too, but a [`Frame`](super::frame::Frame) adopts
/// it and a resident names it, so it can be a `home` and a pin-bundle member. Program storage is neither, and the
/// wrapped `Rc` is private, so [`program_storage`] is the type's only constructor.
pub struct ProgramStorage(Rc<FrameStorage>);

impl ProgramStorage {
    /// Mint this storage's [`ProgramBrand`] — the allocation capability the parse entry points take.
    pub fn brand(&self) -> ProgramBrand<'_> {
        ProgramBrand(self.0.brand(), PhantomData)
    }
}

/// A [`RegionBrand`] carrying the proof that its region is [`ProgramStorage`]'s. The parse entry
/// points take this rather than a bare `RegionBrand`, so a parsed AST's storage tier is checked at
/// every call site rather than held by the discipline of one.
///
/// This is also the value channel's only key. Two answers in
/// [`KObject`](crate::machine::model::KObject) — `object_cell_reach` calling an expression's cell
/// `Owned`, `retains_home` answering `false` — hold because no expression reaching the value
/// channel borrows a region a holder can outlive, and so does the expression door's own claim that
/// the cell it bumps names no producer region ([`RegionBrand::alloc_expression`]). All three cite
/// one type rather than a flow: the channel admits only a
/// [`ProgramExpression`](crate::machine::model::ast::ProgramExpression), which this brand's doors
/// alone mint. A node built at an ordinary [`RegionBrand`] carries no such marker, so the channel
/// is closed to it by type — a runtime-synthesized node dispatches in place instead, and a site
/// that needs one as a value takes this brand (`op_def`'s bridge body) or threads the proof out of
/// the arm it matched.
///
/// The distinction needs a type because `KExpression` is covariant: a node borrowing a per-call
/// region coerces to any shorter lifetime, so the borrow checker sees nothing to object to.
/// Widening through [`ProgramBrand::region`] is free; the reverse does not exist.
///
/// The brand is **invariant** in `'a`, so a held brand never shortens either. A door call therefore
/// pins its `parts` at the storage's own lifetime rather than at whatever shorter lifetime the
/// caller happens to run at — which is what carries the storage-tier obligation in the parameter
/// types instead of in prose. The doors' *products* stay covariant, so program-hosted AST still
/// reaches step code by ordinary subtyping.
#[derive(Clone, Copy)]
pub struct ProgramBrand<'a>(RegionBrand<'a>, PhantomData<fn(&'a ()) -> &'a ()>);

impl<'a> ProgramBrand<'a> {
    /// The plain allocation capability underneath — for the parser's own `alloc_*` calls, which
    /// need no more than a region to bump into.
    pub fn region(self) -> RegionBrand<'a> {
        self.0
    }
}
