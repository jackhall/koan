//! [`Delivered<T, W, F>`] — the **delivery envelope**: a sealed carrier paired with the frame-owner
//! `Rc` that retains its value's backing *in transit*, from a scheduler pull to the point a consumer
//! adopts or re-homes it. See
//! [design/witness-hosting.md § Retention model](../../../design/witness-hosting.md#retention-model).
//!
//! Liveness *at rest* is the scheduler's retention table (a producer frame stays held while any
//! consumer edge is undischarged). Liveness *in transit* — from a pull to its adoption — is this
//! envelope: it bundles the dormant [`Sealed`] carrier with the retained `Rc<F>` that owns the
//! region the carrier's value and its hosted reach set live in, so a consumer reads the value under a
//! pin it does not have to thread separately. The fields are private and every constructor is a
//! surface that has the true owner in hand, so an envelope whose pin disagrees with its payload is
//! not constructible — the co-location the carrier's owned host arm keeps by convention becomes
//! enforcement by construction.
//!
//! `host = None` means a frameless / run-region producer whose backing already outlives every
//! consumer, so no held pin is required; the value opens under the carrier's own bundled witness.

use std::rc::Rc;

use super::{Erased, Reattachable, Sealed, Witness, Witnessed};

/// A sealed carrier paired with the retained frame owner that pins its value's backing in transit.
/// `T` is the carrier's value family, `W` its reach witness, `F` the workload's frame-owner type
/// (`Rc<F>` is the residence pin).
pub struct Delivered<T: Reattachable, W, F> {
    /// The dormant carrier — value and reach as one unit.
    cell: Sealed<T, W>,
    /// The retained frame owner whose region the value lives in, or `None` for a frameless /
    /// run-region producer whose backing outlives every consumer.
    host: Option<Rc<F>>,
}

impl<T: Reattachable, W: Witness, F> Delivered<T, W, F> {
    /// Pair a sealed carrier with the retained frame owner that pins its value's backing (`None` for
    /// a frameless / run-region producer). The caller supplies the true owner — the scheduler's
    /// retention hold for a delivered dep, or the region owner for a resident seal — so the pairing
    /// is co-located by construction.
    pub fn hosted(cell: Sealed<T, W>, host: Option<Rc<F>>) -> Self {
        Delivered { cell, host }
    }

    /// Read the delivered value at a **rank-2** (`for<'b>`) brand, pinned by the retained frame owner
    /// when present ([`Sealed::open_with`]), else by the bundled witness ([`Sealed::open`]) — the
    /// single read verb that collapses the open-with / open fork every consumer site carried while
    /// the pin was threaded separately. The `for<'b>` quantifier confines the re-anchored value
    /// exactly as [`Sealed::open`] does.
    pub fn open<R>(&self, f: impl for<'b> FnOnce(T::At<'b>) -> R) -> R
    where
        T::At<'static>: Copy,
    {
        match &self.host {
            Some(host) => self.cell.open_with(host, f),
            None => self.cell.open(f),
        }
    }

    /// The reference-only reach witness — the set of regions the value names, for a reach query or a
    /// mint. Freely passable (a bit-copy / reference-copy of the carrier's witness); it keeps nothing
    /// alive on its own.
    pub fn witness(&self) -> &W {
        self.cell.witness()
    }

    /// The retained frame owner pinning this delivery in transit, or `None` for a frameless /
    /// run-region producer whose backing outlives every consumer.
    pub fn host(&self) -> Option<&Rc<F>> {
        self.host.as_ref()
    }

    /// The dormant carrier cell — for a consumer that folds it into a construction transfer directly
    /// (its own bundled witness carries the fold's liveness) rather than reading the value under the
    /// retained pin. The transport accessor for the redundant phase, before the carrier collapses to
    /// reference-only.
    pub fn cell(&self) -> &Sealed<T, W> {
        &self.cell
    }

    /// Recover the dormant carrier, consuming the envelope and dropping the retained pin — for a
    /// consumer that re-homes the value under its own liveness and no longer needs the transit host
    /// (the single-part pass-through's `unseal`).
    pub fn into_cell(self) -> Sealed<T, W> {
        self.cell
    }

    /// Duplicate the envelope: [`duplicate`](Sealed::duplicate) the sealed carrier (bit-copy value +
    /// witness clone) and clone the host `Rc`, leaving the source intact — the producer keeps its
    /// terminal for other consumers, and the retained hold gains one `Rc` clone (dropped at
    /// adoption).
    pub fn duplicate(&self) -> Self
    where
        Erased<T>: Copy,
        W: Clone,
    {
        Delivered {
            cell: self.cell.duplicate(),
            host: self.host.clone(),
        }
    }
}

impl<T: Reattachable, W: Witness, F> Delivered<T, W, F> {
    /// Seal a live [`Witnessed`] carrier into a delivery envelope pinned by `host` — the resident
    /// seal veneer's library half. Bundles the born-witnessed carrier with the region owner the
    /// caller already holds, so a resident value travels as an envelope self-covering by its own
    /// witness *and* pinned by its home frame, identical in shape to a delivered dep.
    pub fn seal(witnessed: Witnessed<T, W>, host: Option<Rc<F>>) -> Self {
        Delivered {
            cell: Sealed::seal(witnessed),
            host,
        }
    }
}
