//! A **layer**: a resumption's store over the facts it extends. Its binding extends its base's,
//! a unit re-runs where its frontier reaches a mark the layer **checks** that the binding does not
//! keep ([`Wakes`]), and every other read falls to the base. It writes only what re-runs, so its
//! writes are keyed, sized by what it wrote ([the store](README.md#the-store)).
//!
//! A **witnessing** layer binds only its base's names and the event's module binders, and leaves
//! the event's type names rigid: its walk asks the doors' questions of them under the witness
//! ([`Store::answer`]), and re-runs a unit that reaches a question on one only where the unit
//! does not **hold**. Its witness is read nowhere else, so no fact is built from it.

use crate::memory::resident;
use crate::memory::{BumpAllocator, BumpVec, Lays};
use crate::scope::{
    Answered, Binding, Bound, ClosedTypes, Closing, Frontier, Held, Identity, KeyedWrites, Levels,
    Question, RecordSource, Segment, Slot, Store, UnitFacts, UnitId, Unknown, Wake, answer, keeps,
};
use crate::values::Chain;

use super::wake::Wakes;

/// A resumption's store over `base`, laying through `L`.
pub(crate) struct LayerStore<'o, 'p, L, B> {
    lay: L,
    scratch: BumpAllocator<'p>,
    base: B,
    binding: Binding<'o>,
    /// The names the walk leaves rigid and asks the doors' questions of, beside their witness.
    witness: Binding<'o>,
    /// The closed type the chain's binding and the witness give each lexical level: what a
    /// question is judged again under.
    levels: Levels<'p>,
    wakes: Wakes<'o, 'p>,
    writes: KeyedWrites<'o, 'p>,
}

impl<'o, 'p, L: Lays<'o>, B: RecordSource<'o>> LayerStore<'o, 'p, L, B> {
    /// A layer over `base` binding `binding`, which binds every unknown the base's binds alike: it
    /// checks what `binding` binds that the base's does not.
    pub(crate) fn new(lay: L, scratch: BumpAllocator<'p>, base: B, binding: Binding<'o>) -> Self {
        let fresh = fresh(lay, scratch, &base, binding);
        Self::checking(lay, scratch, base, binding, fresh)
    }

    /// A layer over `base` binding `binding`, as [`new`](Self::new), checking `checked`: names
    /// `binding` binds whose marks a record laid over a witnessed layer may hold answered.
    pub(crate) fn checking(
        lay: L,
        scratch: BumpAllocator<'p>,
        base: B,
        binding: Binding<'o>,
        checked: Binding<'o>,
    ) -> Self {
        if cfg!(debug_assertions) {
            base.each_bound(&mut |unknown, below| {
                let bound = binding.bound(unknown);
                assert!(
                    bound.is_some_and(|bound| bound.same_as(&below)),
                    "a layer binds what its base binds"
                );
            });
        }
        LayerStore {
            lay,
            scratch,
            levels: Levels::of(scratch, base.shapes(), binding),
            base,
            binding,
            witness: Binding::EMPTY,
            wakes: Wakes::new(checked, Binding::EMPTY, binding, scratch),
            writes: KeyedWrites::new(scratch),
        }
    }

    /// A layer over `base` binding `binding`, as [`new`](Self::new), waking every record whose
    /// frontier meets anything `binding` binds: one a base layered over another walked without
    /// its names still names them.
    pub(crate) fn waking_all(
        lay: L,
        scratch: BumpAllocator<'p>,
        base: B,
        binding: Binding<'o>,
    ) -> Self {
        Self::checking(lay, scratch, base, binding, binding)
    }

    /// A witnessing layer over `base`: it binds the base's names and the module binders of `own`,
    /// and witnesses `witness`, the type names of `own` and any the base leaves unbound that an
    /// earlier event of the cell bound. A module binder is bound, never witnessed: every read of
    /// one reads its content.
    pub(crate) fn witnessing(
        lay: L,
        scratch: BumpAllocator<'p>,
        base: B,
        own: Binding<'o>,
        witness: Binding<'o>,
    ) -> Self {
        let mut staged = BumpVec::new_in(scratch);
        base.each_bound(&mut |unknown, bound| staged.push((unknown, bound)));
        staged.extend(
            (own.entries().iter()).filter(|(_, bound)| matches!(bound, Bound::Module { .. })),
        );
        let binding = Binding::laid(lay, &mut staged);
        debug_assert!(
            (witness.entries().iter()).all(|(unknown, bound)| {
                base.bound(*unknown).is_none() && matches!(bound, Bound::Type(_))
            }),
            "a witnessed name is a type name its base leaves unbound"
        );
        let checked = fresh(lay, scratch, &base, binding);
        let mut everything = BumpVec::new_in(scratch);
        everything.extend(binding.entries().iter().copied());
        everything.extend(witness.entries().iter().copied());
        let everything = Binding::laid(scratch, &mut everything);
        LayerStore {
            lay,
            scratch,
            levels: Levels::of(scratch, base.shapes(), everything),
            base,
            binding,
            witness,
            wakes: Wakes::new(checked, witness, binding, scratch),
            writes: KeyedWrites::new(scratch),
        }
    }

    /// What this layer binds: its base's binding and more.
    #[cfg(test)]
    pub(crate) fn bound(&self) -> Binding<'o> {
        self.binding
    }

    /// The records this layer's walk wrote.
    pub(crate) fn writes(&self) -> &KeyedWrites<'o, 'p> {
        &self.writes
    }

    /// The records this layer's walk wrote, the store dropped.
    pub(crate) fn into_writes(self) -> KeyedWrites<'o, 'p> {
        self.writes
    }
}

/// What `binding` binds that `base` leaves unbound, laid through `lay`.
fn fresh<'o, B: RecordSource<'o>>(
    lay: impl Lays<'o>,
    scratch: BumpAllocator<'_>,
    base: &B,
    binding: Binding<'o>,
) -> Binding<'o> {
    let mut fresh = BumpVec::new_in(scratch);
    fresh.extend(
        (binding.entries().iter())
            .filter(|(unknown, _)| base.bound(*unknown).is_none())
            .copied(),
    );
    Binding::laid(lay, &mut fresh)
}

impl<'o, L: Lays<'o>, B: RecordSource<'o>> Store<'o> for LayerStore<'o, '_, L, B> {
    type Lay = L;

    fn lay(&self) -> L {
        self.lay
    }

    fn binding(&self, unknown: Unknown) -> Option<Bound<'o>> {
        self.binding.bound(unknown)
    }

    fn fact(&self, unit: UnitId) -> Option<(&'o UnitFacts<'o>, Frontier<'o>)> {
        self.writes.unit(unit).or_else(|| self.base.unit(unit))
    }

    fn close(&self, body: Segment) -> Option<(&'o Closing<'o>, Frontier<'o>)> {
        (self.writes.closing(body)).or_else(|| self.base.closing(body))
    }

    fn wakes(&self, frontier: Frontier<'_>, types: &Chain<'_, '_, '_, '_>) -> Wake {
        let scratch = self.scratch;
        (self.wakes).wakes(frontier, &|answered| {
            keeps(types, scratch, answered, &self.levels)
        })
    }

    fn checks_open(&self, frontier: Frontier<'_>, types: &Chain<'_, '_, '_, '_>) -> bool {
        let scratch = self.scratch;
        (self.wakes).checks_open(frontier, &|answered| {
            keeps(types, scratch, answered, &self.levels)
        })
    }

    fn answer(
        &self,
        question: Question<'_>,
        unknown_of: &dyn Fn(usize) -> Option<Unknown>,
        types: &Chain<'_, '_, '_, '_>,
        scratch: BumpAllocator<'_>,
    ) -> Option<&'o Answered<'o>> {
        if self.witness.is_empty() {
            return None;
        }
        let mut witnessed = |level| match self.witness.bound(unknown_of(level)?)? {
            Bound::Type(closed) => Some(closed),
            Bound::Module { .. } => None,
        };
        let (substituted, changed) = question.substituted(types, scratch, &mut witnessed);
        if !changed {
            return None;
        }
        let answer = answer(types, scratch, &substituted)?;
        Some(resident(
            self.lay,
            Answered {
                question: question.laid(self.lay),
                answer,
            },
        ))
    }

    fn each_witnessed(&self, each: &mut dyn FnMut(Unknown)) {
        (self.witness.entries().iter()).for_each(|(unknown, _)| each(*unknown));
    }

    fn holding(&self, held: Held) -> Option<UnitId> {
        self.base.holding(held).map(|(unit, _)| unit)
    }

    fn holders(&self, segment: Segment, identity: Identity, each: &mut dyn FnMut(UnitId, Slot)) {
        self.base.holders(segment, identity, each);
    }

    fn write(&self, unit: UnitId, facts: &'o UnitFacts<'o>, frontier: Frontier<'o>) {
        self.writes.write(unit, facts, frontier);
    }

    fn write_close(&self, body: Segment, closing: &'o Closing<'o>, frontier: Frontier<'o>) {
        self.writes.write_close(body, closing, frontier);
    }
    fn write_closed(&self, closed: ClosedTypes<'o>) {
        self.writes.write_closed(closed);
    }
}
