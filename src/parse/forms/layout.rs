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
//! The run is bumped into the region of whatever names it (the body node's own at parse, the
//! callable's captured region at birth) and is plain `Copy` data with no drop glue, so a layout
//! costs the holder a thin pointer and its region nothing at teardown.

use crate::memory::{BumpAllocator, BumpVec, reattachable};
use crate::parse::ast::{ExpressionPart, KExpression};
use crate::parse::labels::{BinderSymbol, ValueSymbol};

/// One layout entry: a value binder's name and the lexical position its binder writes at.
type Entry = (ValueSymbol, u32);

/// The value binders one frame can hold, sorted by symbol. A slot **is** an index into `entries`.
///
/// Immutable and `Copy`-cheap to read through: nothing here is per-activation state, so one layout
/// is shared by every frame the body opens.
#[derive(Clone, Copy)]
pub struct SlotLayout<'a> {
    entries: &'a [Entry],
}

// Lifetimes do not affect layout, so the retype is a no-op: `SlotLayout<'r>` is one thin slice
// reference whatever `'r` is. The macro's `!needs_drop` backstop is the drop-freeness proof a
// bump-hosted layout rests on.
reattachable! { SlotLayout<'static> => SlotLayout<'r> }

impl<'a> SlotLayout<'a> {
    /// The layout of a body that binds no value — the shared empty run, so a bodyless or
    /// binder-free frame bumps nothing at all.
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
    /// Bumped into `brand`'s region — the node's own, since this runs from the construction door.
    /// A body that binds no value takes [`EMPTY`](Self::EMPTY) and bumps nothing.
    pub(crate) fn of_body(brand: BumpAllocator<'a>, body: &KExpression<'a>) -> &'a SlotLayout<'a> {
        // Counted before anything is staged: a body binding no value — every node that is not a
        // block of binders, which is nearly all of them — leaves this door having touched no
        // allocator at all.
        let binders = statements_of(body)
            .filter(|(statement, _)| binder_of(statement).is_some())
            .count();
        if binders == 0 {
            return SlotLayout::EMPTY;
        }
        let mut entries = BumpVec::with_capacity_in(binders, brand);
        entries.extend(statements_of(body).filter_map(|(statement, position)| {
            binder_of(statement).map(|name| (name, position as u32))
        }));
        Self::seal(brand, &mut entries)
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
        brand: BumpAllocator<'a>,
        params: &[(BinderSymbol, T)],
        body: &SlotLayout<'_>,
    ) -> &'a SlotLayout<'a> {
        let values = params
            .iter()
            .filter(|(binder, _)| matches!(binder, BinderSymbol::Value(_)));
        let mut entries =
            BumpVec::with_capacity_in(values.clone().count() + body.entries.len(), brand);
        entries.extend(values.filter_map(|(binder, _)| match binder {
            BinderSymbol::Value(name) => Some((*name, 0)),
            BinderSymbol::Type(_) => None,
        }));
        entries.extend(body.entries.iter().copied());
        Self::seal(brand, &mut entries)
    }

    /// One binder at one position — the layout of a frame whose whole body is a single statement
    /// submitted at a position the call site fixes rather than the body's own shape (`EVAL`).
    #[cfg_attr(not(feature = "pending_rewrite"), allow(dead_code))]
    pub(crate) fn single(
        brand: BumpAllocator<'a>,
        name: ValueSymbol,
        position: usize,
    ) -> &'a SlotLayout<'a> {
        brand.alloc(SlotLayout {
            entries: brand.alloc_slice_copy(&[(name, position as u32)]),
        })
    }

    /// Re-home this layout into `brand`'s region — what a copied environment's scope takes, minted
    /// at the destination the way the copied callable's signature is.
    #[cfg_attr(not(feature = "pending_rewrite"), allow(dead_code))]
    pub(crate) fn rehomed<'b>(&self, brand: BumpAllocator<'b>) -> &'b SlotLayout<'b> {
        if self.entries.is_empty() {
            return SlotLayout::EMPTY;
        }
        brand.alloc(SlotLayout {
            entries: brand.alloc_slice_copy(self.entries),
        })
    }

    /// Sort, dedupe first-wins, and freeze — the one place a layout is written, so every door above
    /// ships the same sorted, position-carrying invariant. `entries` is staged in `brand`'s own
    /// bump, so the run is sorted where it sits and the frozen copy costs one more bump rather than
    /// a heap round trip.
    fn seal(brand: BumpAllocator<'a>, entries: &mut BumpVec<'a, Entry>) -> &'a SlotLayout<'a> {
        if entries.is_empty() {
            return SlotLayout::EMPTY;
        }
        // Sort by symbol, and by position within a symbol so the retained duplicate is the
        // lexically earliest binder of the name — the one the bind-once table would keep.
        entries.sort_unstable();
        entries.dedup_by_key(|(name, _)| *name);
        brand.alloc(SlotLayout {
            entries: brand.alloc_slice_copy(entries),
        })
    }
}

/// A body's statements beside the lexical position each submits at: statement `i` of a statement
/// block writes at `i + 1`, and a single-statement body is its own statement at `1`.
fn statements_of<'b, 'a>(
    body: &'b KExpression<'a>,
) -> impl Iterator<Item = (&'b KExpression<'a>, usize)> + Clone {
    let block = body.is_statement_block().then_some(body.parts);
    let statements = block.unwrap_or_default().iter().map(|part| {
        let ExpressionPart::Expression(statement) = part.value else {
            unreachable!("a statement block's parts are all expressions");
        };
        statement.reference()
    });
    let single = (!body.is_statement_block()).then_some(body);
    statements
        .chain(single)
        .enumerate()
        .map(|(i, statement)| (statement, i + 1))
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
