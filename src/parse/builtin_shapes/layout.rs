//! The **slot layout**: a body's value binders as a symbol-sorted run, computed once where the
//! shape is lexically fixed and read by every activation of that body.
//!
//! A per-call frame's value bindings are addressed by *slot* — a position in this run — rather than
//! by hash probe, so an activation allocates one sized array instead of building a table from
//! nothing ([`SlotArray`](crate::memory::SlotArray)). Two halves meet here and neither is a second
//! enumeration of the other: the body half is read off the same
//! [`statement_binder_plan`](crate::parse::ast::KExpression::statement_binder_plan) the
//! `CLOSE` capture walk and the dispatch-time claim stamp read
//! ([`of_body`](SlotLayout::of_body)), and the parameter half is the signature's own `params()` run,
//! merged in where a callable is born ([`for_function`](SlotLayout::for_function)).
//!
//! **Slot order is symbol order**, never signature or source order: a `FN` and its body agree on a
//! name's slot because both resolve it through the same sorted search, with nothing to keep in step.
//! The *lexical position* a binder writes at rides beside each entry — `0` for a parameter, `i + 1`
//! for the body's statement `i` — and it is what the `idx < cutoff` visibility rule reads, so
//! slotting changes the addressing and not the positional rule.
//!
//! The run is written into the store of whatever names it (the body node's own at parse, the
//! callable's captured region at birth) and is plain `Copy` data with no drop glue, so a layout
//! costs the holder a thin pointer and its region nothing at teardown.
//!
//! Entries are staged on the stack rather than in the store they end up in: the run must be sorted
//! and deduped before it is frozen, and a written run is a shared borrow. The same reason
//! [`fn_def_binder_bucket`](super::binder) stages its key in a `SmallVec` — a body binding more
//! than the inline capacity spills, which is the one case that allocates.

use smallvec::SmallVec;

use crate::memory::{Writer, collect, resident};
use crate::parse::ast::KExpression;
use crate::symbols::{BinderSymbol, ValueSymbol};

/// One layout entry: a value binder's name and the lexical position its binder writes at.
type Entry = (ValueSymbol, u32);

/// A layout's entries while they are still being ordered — on the stack, since sorting needs a
/// mutable run and a written one is shared. Eight entries is the body most bodies are.
type Staged = SmallVec<[Entry; 8]>;

/// The value binders one frame can hold, sorted by symbol. A slot **is** an index into `entries`.
///
/// Immutable and `Copy`-cheap to read through: nothing here is per-activation state, so one layout
/// is shared by every frame the body opens.
#[derive(Clone, Copy)]
pub struct SlotLayout<'a> {
    entries: &'a [Entry],
}

impl<'a> SlotLayout<'a> {
    /// The layout of a body that binds no value — the shared empty run, so a bodyless or
    /// binder-free frame writes nothing at all.
    pub const EMPTY: &'static SlotLayout<'static> = &SlotLayout { entries: &[] };

    /// How many slots a frame over this layout allocates.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The slot `name` addresses — one binary search over the sorted run, the read that replaces a
    /// hash probe on the value channel.
    pub fn slot_of(&self, name: ValueSymbol) -> Option<usize> {
        self.entries
            .binary_search_by_key(&name, |(symbol, _)| *symbol)
            .ok()
    }

    /// The lexical position the binder at `slot` writes at — what the `idx < cutoff` visibility
    /// rule reads.
    pub fn position(&self, slot: usize) -> usize {
        self.entries[slot].1 as usize
    }

    /// The name bound at `slot`.
    pub fn name(&self, slot: usize) -> ValueSymbol {
        self.entries[slot].0
    }

    /// Every `(slot, name, position)` in slot order.
    pub fn iter(&self) -> impl Iterator<Item = (usize, ValueSymbol, usize)> + '_ {
        self.entries
            .iter()
            .enumerate()
            .map(|(slot, (name, position))| (slot, *name, *position as usize))
    }

    /// The value binders `body`'s own statements declare, each at the position its statement
    /// submits at: statement `i` of a statement block writes at `i + 1`, and a single-statement
    /// body is its own statement at `1`. Read off [`KExpression::statement_binder_plan`], the same
    /// cached plan the claim stamp and the `CLOSE` capture walk read, so a layout and the bind it
    /// sizes cannot disagree about what a body binds.
    ///
    /// Type binders and bucket registrations contribute nothing: they land in the keyed channels,
    /// which slotting does not touch. A repeated name keeps its first (lowest) position, matching
    /// the bind-once table a second binder of the name would `Rebind` against.
    ///
    /// Written through `writer` — the node's own store, since this runs from the construction
    /// door. A body that binds no value takes [`EMPTY`](Self::EMPTY) and writes nothing.
    pub(crate) fn of_body(writer: Writer<'a>, body: &KExpression<'a>) -> &'a SlotLayout<'a> {
        let mut entries: Staged = body
            .body_statements()
            .filter_map(|(statement, position)| {
                binder_of(statement).map(|name| (name, position as u32))
            })
            .collect();
        Self::seal(writer, &mut entries)
    }

    /// A callable's whole layout: its parameters at position `0` merged with the body layout above.
    /// Built once where the callable is born, beside the signature the parameter half is read from,
    /// so no activation rebuilds it and the bind loop and the layout read one schema.
    ///
    /// A parameter beats a body binder of the same name on position — `0 < i + 1` — which is the
    /// same first-wins rule `of_body` applies inside the body, and leaves the body's own `LET` of a
    /// parameter name a rebind of the parameter's slot exactly as it is against the keyed map.
    /// Type-denoting parameters register types, so they take no slot.
    ///
    /// Generic in what a parameter is paired with: only the [`BinderSymbol`] is read, so the caller
    /// hands its own pairs through without restating their type half.
    #[cfg_attr(not(feature = "pending_rewrite"), allow(dead_code))]
    pub(crate) fn for_function<T>(
        writer: Writer<'a>,
        params: &[(BinderSymbol, T)],
        body: &SlotLayout<'_>,
    ) -> &'a SlotLayout<'a> {
        let mut entries: Staged = params
            .iter()
            .filter_map(|(binder, _)| match binder {
                BinderSymbol::Value(name) => Some((*name, 0)),
                BinderSymbol::Type(_) => None,
            })
            .collect();
        entries.extend(body.entries.iter().copied());
        Self::seal(writer, &mut entries)
    }

    /// One binder at one position — the layout of a frame whose whole body is a single statement
    /// submitted at a position the call site fixes rather than the body's own shape (`EVAL`).
    #[cfg_attr(not(feature = "pending_rewrite"), allow(dead_code))]
    pub(crate) fn single(
        writer: Writer<'a>,
        name: ValueSymbol,
        position: usize,
    ) -> &'a SlotLayout<'a> {
        resident(
            writer,
            SlotLayout {
                entries: collect(writer, std::iter::once((name, position as u32))),
            },
        )
    }

    /// Re-home this layout into `writer`'s region — what a copied environment's scope takes, minted
    /// at the destination the way the copied callable's signature is.
    #[cfg_attr(not(feature = "pending_rewrite"), allow(dead_code))]
    pub(crate) fn rehomed<'b>(&self, writer: Writer<'b>) -> &'b SlotLayout<'b> {
        if self.entries.is_empty() {
            return SlotLayout::EMPTY;
        }
        resident(
            writer,
            SlotLayout {
                entries: collect(writer, self.entries.iter().copied()),
            },
        )
    }

    /// Sort, dedupe first-wins, and freeze — the one place a layout is written, so every door above
    /// ships the same sorted, position-carrying invariant. `entries` is the caller's stack staging,
    /// so the run is ordered where it sits and reaches the store once, at the length the dedupe
    /// settled.
    fn seal(writer: Writer<'a>, entries: &mut Staged) -> &'a SlotLayout<'a> {
        if entries.is_empty() {
            return SlotLayout::EMPTY;
        }
        // Sort by symbol, and by position within a symbol so the retained duplicate is the
        // lexically earliest binder of the name — the one the bind-once table would keep.
        entries.sort_unstable();
        entries.dedup_by_key(|(name, _)| *name);
        resident(
            writer,
            SlotLayout {
                entries: collect(writer, entries.iter().copied()),
            },
        )
    }
}

/// `statement`'s own value binder, if it declares one.
fn binder_of(statement: &KExpression<'_>) -> Option<ValueSymbol> {
    match statement.statement_binder_plan()?.name {
        Some(BinderSymbol::Value(name)) => Some(name),
        Some(BinderSymbol::Type(_)) | None => None,
    }
}

#[cfg(test)]
mod tests;
