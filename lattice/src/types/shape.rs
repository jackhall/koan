//! The vocabulary an expression shape is spelled in: the element run a call must match, the
//! priority classes its slots rank in, the surface a deferred return is shadowed as, and the
//! four-verdict specificity a dispatch ranks candidates by.
//!
//! A shape node itself lives in [`node`](super::node); this file owns the pieces that appear
//! *inside* one — among them [`StoredElement`], an element as the node stores it, with a slot's
//! priority class beside its type's node, and [`Classes`], the ranking read back off that run — the
//! one reader of a shape's parts ([`Shape`], and the free readers such as [`shape_slots`] and
//! [`keys_equal`]), [`map_slots`], the one way an element run is rebuilt, plus the specificity
//! verdict [`shape_specificity`](super::sig_relations) produces.

use crate::bump::{BumpAllocator, BumpVec};
use crate::symbols::{KeywordSymbol, SymbolInterner, TypeSymbol};

use super::chain::TypeChain;
use super::handle::{Handle, KType, Naming, Parametric, Scheme, TypeHandle};
use super::node::{Group, TypeNode};
use super::run::Elements;
use super::stored::Linked;
use super::tiers::Habitat;

/// One position of an expression shape: a fixed token as its [`KeywordSymbol`], or an argument
/// slot carrying its declared type as `H`. Both arms are `Copy` — a symbol is `u128` bits, a type
/// is named by its handle or its node — so an element compares meaningfully wherever it is stored
/// and a re-homed run copies nothing but the elements themselves. A node stores each slot by its
/// node; a typed read hands each slot back as the reading handle's child.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum DispatchTokenElement<H = Handle> {
    Keyword(KeywordSymbol),
    Slot(H),
}

/// One element as an interned shape stores it: a keyword, or a reference to a slot's type beside
/// the priority class it ranks in. The class costs nothing: it sits in the padding beside the
/// reference.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(super) enum StoredElement<'run> {
    Keyword(KeywordSymbol),
    Slot(Linked<'run>, u16),
}

const _: () = assert!(std::mem::size_of::<StoredElement<'static>>() == 32);

impl<'run> StoredElement<'run> {
    /// The element read as `C`, its class dropped.
    pub(super) fn element<C: Naming<'run>>(self) -> DispatchTokenElement<C> {
        match self {
            StoredElement::Keyword(keyword) => DispatchTokenElement::Keyword(keyword),
            StoredElement::Slot(slot, _) => DispatchTokenElement::Slot(C::read(slot)),
        }
    }
}

/// A shape's ranking: each slot's priority class, in slot order, read off the stored elements.
/// Written order is spelled out, `0..n`.
#[derive(Clone, Copy)]
pub struct Classes<'run> {
    elements: &'run [StoredElement<'run>],
}

impl<'run> Classes<'run> {
    /// The ranking of a shape with no slots.
    pub(super) const EMPTY: Self = Classes { elements: &[] };

    /// The ranking over a stored element run.
    pub(super) fn over(elements: &'run [StoredElement<'run>]) -> Self {
        Classes { elements }
    }

    /// The class of each slot, in slot order.
    pub fn iter(self) -> impl Iterator<Item = u16> + 'run {
        self.elements.iter().filter_map(|element| match element {
            StoredElement::Slot(_, class) => Some(*class),
            StoredElement::Keyword(_) => None,
        })
    }

    /// The class of slot `slot`.
    pub fn get(self, slot: usize) -> u16 {
        self.iter().nth(slot).expect("a slot of the shape")
    }

    /// Whether every slot ranks in its own written position.
    pub(super) fn is_written_order(self) -> bool {
        self.iter()
            .enumerate()
            .all(|(index, class)| usize::from(class) == index)
    }

    /// How many classes the ranking has: one past the highest, or none for no slots.
    pub(super) fn count(self) -> usize {
        self.iter()
            .max()
            .map_or(0, |highest| usize::from(highest) + 1)
    }

    /// The ranking as a slice in `scratch`, as a shape door takes one.
    pub fn copied_into<'s>(self, scratch: BumpAllocator<'s>) -> &'s [u16] {
        let mut ranking = BumpVec::new_in(scratch);
        ranking.extend(self.iter());
        ranking.leak()
    }
}

/// Two rankings are equal when their slots rank alike, slot by slot.
impl PartialEq for Classes<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.iter().eq(other.iter())
    }
}

impl Eq for Classes<'_> {}

impl std::fmt::Debug for Classes<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

/// `elements` with each slot type mapped through `map` and every keyword kept — the one way an
/// element run is rebuilt. Staged in `scratch`.
pub(super) fn map_slots<'s, 'run, H: Copy>(
    scratch: BumpAllocator<'s>,
    elements: impl IntoIterator<Item = DispatchTokenElement<H>>,
    mut map: impl FnMut(H) -> Linked<'run>,
) -> BumpVec<'s, DispatchTokenElement<Linked<'run>>> {
    let elements = elements.into_iter();
    let mut out = BumpVec::with_capacity_in(elements.size_hint().0, scratch);
    out.extend(elements.map(|element| match element {
        DispatchTokenElement::Slot(slot) => DispatchTokenElement::Slot(map(slot)),
        DispatchTokenElement::Keyword(keyword) => DispatchTokenElement::Keyword(keyword),
    }));
    out
}

/// An expression shape's parts, read once off its node — the one reader every class-by-class
/// walk, relation and render shares. A group's bounds are handles, which a relation enters.
#[derive(Clone, Copy)]
pub(super) struct Shape<'run> {
    pub(super) quantifiers: &'run [TypeSymbol],
    pub(super) bounds: &'run [KType],
    pub(super) elements: Elements<'run, Linked<'run>>,
    pub(super) classes: Classes<'run>,
    pub(super) ret: Linked<'run>,
}

impl<'run> Shape<'run> {
    /// `kt`'s shape parts, read through a scheme to its body, or `None` for anything that is not a
    /// shape or a shape scheme. A plain shape's group is empty.
    pub(super) fn of(kt: Linked<'run>) -> Option<Self> {
        let (group, body) = match kt.view() {
            TypeNode::Scheme { group, body } => (*group, body.view()),
            node => (Group::EMPTY, node),
        };
        let TypeNode::ExpressionShape {
            elements,
            classes,
            ret,
        } = body
        else {
            return None;
        };
        Some(Shape {
            quantifiers: group.names,
            bounds: group.bounds,
            elements,
            classes,
            ret,
        })
    }

    /// The slot types, in slot order.
    pub(super) fn slots(self) -> impl Iterator<Item = Linked<'run>> + use<'run> {
        self.elements.iter().filter_map(|element| match element {
            DispatchTokenElement::Slot(kt) => Some(kt),
            DispatchTokenElement::Keyword(_) => None,
        })
    }

    /// How many classes the ranking has: one past the highest.
    pub(super) fn class_count(self) -> usize {
        self.classes.count()
    }

    /// Every class index of the ranking, as the `u16` a class is named by.
    pub(super) fn class_indices(self) -> impl Iterator<Item = u16> {
        (0..self.class_count())
            .map(|class| u16::try_from(class).expect("a shape has fewer than 65536 slots"))
    }

    /// The bound of the `index`-th variable of the shape's scheme group.
    pub(super) fn bound(self, index: usize) -> KType {
        self.bounds.get(index).copied().unwrap_or(KType::ANY)
    }
}

/// Whether two shapes key the same dispatch bucket — each one's element sequence with the slot
/// types erased, compared position by position.
///
/// The bucket key is a *reading* of the member's type, never a second copy of it, and a reading
/// only ever has to be compared, so no consumer materializes one and the pairwise readers
/// allocate nothing at all.
pub(super) fn keys_equal<'run>(left: Linked<'run>, right: Linked<'run>) -> bool {
    elements_key_equal(shape_elements(left), shape_elements(right))
}

/// [`keys_equal`] over two element runs already in hand.
pub(super) fn elements_key_equal<'run>(
    left: Elements<'run, Linked<'run>>,
    right: Elements<'run, Linked<'run>>,
) -> bool {
    left.len() == right.len()
        && left.iter().zip(right.iter()).all(|(a, b)| match (a, b) {
            (DispatchTokenElement::Keyword(x), DispatchTokenElement::Keyword(y)) => x == y,
            (DispatchTokenElement::Slot(_), DispatchTokenElement::Slot(_)) => true,
            _ => false,
        })
}

/// A shape's element sequence, read through a scheme; empty for anything that is not a shape,
/// which no schema member ever is. The one place the "not a shape reads as the empty key"
/// convention lives.
pub(super) fn shape_elements(kt: Linked<'_>) -> Elements<'_, Linked<'_>> {
    Shape::of(kt).map_or(Elements::over(&[]), |shape| shape.elements)
}

/// A shape's ranking — each slot's dense priority class — read through a scheme, or empty for
/// anything that is not a shape.
pub(super) fn shape_classes(kt: Linked<'_>) -> Classes<'_> {
    Shape::of(kt).map_or(Classes::EMPTY, |shape| shape.classes)
}

/// A shape's argument-position types, in order — the bucket key's typed half, for the readers that
/// compare or render one position at a time. Read straight off the node's element run, so the
/// read builds nothing.
pub fn shape_slots<'run, H: TypeHandle, X: Habitat<'run>>(
    kt: H,
    types: &TypeChain<'run, X>,
) -> impl Iterator<Item = H::Child> + use<'run, H, X> {
    slots_of(shape_elements(types.enter(kt.raw())))
}

/// A scheme's argument-position types, in order, each of which may read the scheme's own group.
pub fn scheme_slots<'run, X: Habitat<'run>>(
    scheme: Scheme,
    types: &TypeChain<'run, X>,
) -> impl Iterator<Item = Parametric> + use<'run, X> {
    slots_of(shape_elements(types.enter(scheme.raw())))
}

/// The slot types of an element run, read as `C`.
pub(super) fn slots_of<'run, C: Naming<'run>>(
    elements: Elements<'run, Linked<'run>>,
) -> impl Iterator<Item = C> + use<'run, C> {
    elements.raw().iter().filter_map(|element| match *element {
        StoredElement::Slot(kt, _) => Some(C::read(kt)),
        StoredElement::Keyword(_) => None,
    })
}

/// A shape's return type, or `None` for anything that is not a shape.
pub fn shape_return<'ty, H: TypeHandle>(
    kt: H,
    types: &TypeChain<'ty, impl Habitat<'ty>>,
) -> Option<H::Child> {
    Shape::of(types.enter(kt.raw())).map(|shape| <H::Child as Naming>::read(shape.ret))
}

/// A scheme's return type, which may read the scheme's own group; `None` for a function scheme.
pub fn scheme_return<'ty>(
    scheme: Scheme,
    types: &TypeChain<'ty, impl Habitat<'ty>>,
) -> Option<Parametric> {
    Shape::of(types.enter(scheme.raw())).map(|shape| <Parametric as Naming>::read(shape.ret))
}

/// A confined FN `ret` slot whose source return is deferred to per-call elaboration — `-> er` or
/// `-> er.Carrier`. It holds only the hashable surface shadow: a bare name's lifetime-free
/// [`TypeSymbol`], or an expression's canonical render, which the chain lays into its half
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
pub fn dense_classes<'s>(scratch: BumpAllocator<'s>, raw: &[RawRank]) -> &'s [u16] {
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
        u16::try_from(class).expect("a shape has fewer than 65536 slots")
    }))
}

/// Whether `classes` ranks the slots in written order: empty, or `0..n`.
pub(super) fn written_order(classes: &[u16]) -> bool {
    classes
        .iter()
        .enumerate()
        .all(|(index, class)| usize::from(*class) == index)
}

/// The class of slot `index` under `classes` — its own index where the shape is written-order.
pub(super) fn class_of(classes: &[u16], index: usize) -> u16 {
    classes
        .get(index)
        .copied()
        .unwrap_or_else(|| u16::try_from(index).expect("a shape has fewer than 65536 slots"))
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
