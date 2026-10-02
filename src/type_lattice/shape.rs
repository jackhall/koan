//! The vocabulary an expression shape is spelled in: the element run a call must match, the
//! priority classes its slots rank in, the surface a deferred return is shadowed as, and the
//! four-verdict specificity a dispatch ranks candidates by.
//!
//! A shape node itself lives in [`node`](super::node); this file owns the pieces that appear
//! *inside* one, plus the specificity verdict [`shape_specificity`](super::sig_relations)
//! produces.

use crate::memory::{BumpAllocator, BumpVec};
use crate::symbols::{KeywordSymbol, SymbolInterner, TypeSymbol};

use super::handle::{Handle, TypeHandle, wrap};

/// One position of an expression shape: a fixed token as its [`KeywordSymbol`], or an argument
/// slot carrying its declared type as `H`. Both arms are `Copy` handles — a symbol is `u128` bits,
/// a type handle is interned in the run registry — so an element compares meaningfully wherever it
/// is stored and a re-homed run copies nothing but the elements themselves. A node stores its run
/// raw; a typed read hands each slot back as the reading handle's child.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum DispatchTokenElement<H = Handle> {
    Keyword(KeywordSymbol),
    Slot(H),
}

impl<H: TypeHandle> DispatchTokenElement<H> {
    /// The element with its slot read as `C`.
    pub(super) fn view<C: TypeHandle>(self) -> DispatchTokenElement<C> {
        match self {
            DispatchTokenElement::Keyword(keyword) => DispatchTokenElement::Keyword(keyword),
            DispatchTokenElement::Slot(slot) => DispatchTokenElement::Slot(wrap(slot.raw())),
        }
    }

    /// The element with its slot raw.
    pub(super) fn raw(self) -> DispatchTokenElement {
        self.view()
    }
}

/// A confined FN `ret` slot whose source return is deferred to per-call elaboration — `-> er` or
/// `-> er.Carrier`. It holds only the hashable surface shadow: a bare name's lifetime-free
/// [`TypeSymbol`], or an expression's canonical render, which the registry bumps into the run
/// region when the node is interned. Identity is syntactic, so a
/// [`TypeNode::DeferredReturn`](super::node::TypeNode::DeferredReturn) compares, hashes and
/// digests by surface form.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum DeferredReturnSurface<'run> {
    Type(TypeSymbol),
    Expression(&'run str),
}

impl DeferredReturnSurface<'_> {
    /// Surface form for diagnostics, written straight into `f`; the `Type` carrier resolves its
    /// spelling through the run's interner and the `Expression` carrier writes the text it stores.
    pub fn write_surface(
        &self,
        f: &mut std::fmt::Formatter<'_>,
        symbols: &SymbolInterner,
    ) -> std::fmt::Result {
        match self {
            Self::Type(name) => write!(f, "{}", symbols.display(name.symbol())),
            Self::Expression(text) => f.write_str(text),
        }
    }

    /// Whether this surface opens with the type sigil — the
    /// [`surface_opens_sigil`](super::render::surface_opens_sigil) arm for a deferred return. A
    /// `Type` carrier names a bare token; an `Expression` carrier answers for the text it stores.
    pub fn opens_sigil(&self) -> bool {
        match self {
            Self::Type(_) => false,
            Self::Expression(text) => text.starts_with(':'),
        }
    }
}

/// One slot's place in a written ranking, in the order the slots are written: an integer a bucket
/// declaration or a `SIG` member writes in the slot's place (`EXPR #(MOVE 2 TO 1)`), or `_`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RawRank {
    Numbered(u32),
    Unnumbered,
}

/// The one normalizer from a written ranking to the dense classes an
/// [`ExpressionShape`](super::node::TypeNode::ExpressionShape) stores: numbered slots come first,
/// in integer order, slots sharing an integer share a class, and each `_` is a class of its own
/// after them, in written order. So `2 1` and `20 10` are both `[1, 0]`, and `_ _` is `[0, 1]`.
pub fn dense_classes<'s>(scratch: BumpAllocator<'s>, raw: &[RawRank]) -> &'s [u8] {
    let key = |index: usize| match raw[index] {
        RawRank::Numbered(n) => (0u8, n as usize),
        RawRank::Unnumbered => (1u8, index),
    };
    let mut keys = BumpVec::with_capacity_in(raw.len(), scratch);
    keys.extend((0..raw.len()).map(key));
    keys.sort_unstable();
    keys.dedup();
    scratch.alloc_slice_fill_iter((0..raw.len()).map(|index| {
        let class = keys
            .binary_search(&key(index))
            .expect("every key was collected");
        u8::try_from(class).expect("a shape has fewer than 256 slots")
    }))
}

/// Whether `classes` ranks the slots in written order: empty, or `0..n`.
pub(super) fn written_order(classes: &[u8]) -> bool {
    classes
        .iter()
        .enumerate()
        .all(|(index, class)| usize::from(*class) == index)
}

/// The class of slot `index` under `classes` — its own index where the shape is written-order.
pub(super) fn class_of(classes: &[u8], index: usize) -> u8 {
    classes
        .get(index)
        .copied()
        .unwrap_or_else(|| u8::try_from(index).expect("a shape has fewer than 256 slots"))
}

/// How two candidates under one dispatch bucket rank against each other.
/// `Incomparable` means neither dominates: each admits an argument tuple the other rejects.
#[derive(Debug, Eq, PartialEq, Clone, Copy)]
pub enum Specificity {
    StrictlyMore,
    StrictlyLess,
    Equal,
    Incomparable,
}
