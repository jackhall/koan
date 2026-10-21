//! The **one walk**: everything the load decides about a program's bodies, computed unit by unit
//! into a [`Store`] ([the one-walk build](../../roadmap/gradual-build/one-walk-build.md)).
//!
//! An owning body — the program, a callable, a quote's code — runs its entry, then each of its
//! units in order. Each unit computes its facts in five steps: its paths, its declarations, its
//! types and registrations, its value statics, then its binder facts. An inline body — an arm, a
//! block, a `USING` body, a `MODULE` or `GROUP` body — runs as a step inside the unit that runs
//! it, and its facts land in that unit's record. A callable is walked right after the unit that
//! births it, while the bodies around it are still on the chain, and a quote's code where its
//! birth unit meets the quote. Once its units have run, a body's closing record holds what reads
//! the body as a whole: its return check, exits, class and summary.
//!
//! A unit whose record the store holds runs again only where the store's binding wakes it: else
//! the walk absorbs the record. Every unit's record rests beside its **frontier**: the unknowns
//! every fact it read waits on, gathered at unit grain.

use crate::knot::KBuiltins;
use crate::memory::{BumpAllocator, BumpVec, resident};
use crate::scope::{
    BodyShape, Closing, Frontier, Segment, ShapeError, ShapeKind, Shapes, Store, UnitFacts, UnitId,
    UnitIndex, UnitWork,
};
use crate::values::Chain;

/// Walk every body of `shapes` into `store`, interning through `types`: the language's load, or a
/// resumption under the store's binding.
pub(super) fn walk<'graph: 'o, 'o, S: Store<'o>>(
    #[allow(dead_code)] shapes: &'graph Shapes<'graph>,
    builtins: &'graph KBuiltins<'graph, 'graph>,
    store: &S,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
) -> Result<(), ShapeError<'o>> {
    let mut walk = Walk {
        shapes,
        builtins,
        store,
        types,
        scratch,
        walked: scratch.alloc_slice_fill_with(shapes.bodies().len(), |_| false),
        records: std::marker::PhantomData,
    };
    walk.body(shapes.root())
}

/// The walk's state: every value a record keeps is laid through the store's `lay`, at `'o`, and
/// only transient work in `scratch`, at `'p`.
struct Walk<'x, 'p, 'o, 'graph, 'tier, 'pair, 'reg, S> {
    #[allow(dead_code)]
    shapes: &'graph Shapes<'graph>,
    #[allow(dead_code)]
    builtins: &'graph KBuiltins<'graph, 'graph>,
    store: &'x S,
    #[allow(dead_code)]
    types: &'x Chain<'tier, 'pair, 'reg, 'x>,
    scratch: BumpAllocator<'p>,
    /// Each owning body this walk has walked, by segment.
    walked: &'p mut [bool],
    records: std::marker::PhantomData<&'o ()>,
}

impl<'p, 'graph: 'o, 'o, S: Store<'o>> Walk<'_, 'p, 'o, 'graph, '_, '_, '_, S> {
    /// Walk the owning body `shape`: its entry, each unit and the callables it births, then its
    /// closing record.
    fn body(&mut self, shape: &'graph BodyShape<'graph>) -> Result<(), ShapeError<'o>> {
        let segment = shape.segment();
        if std::mem::replace(&mut self.walked[segment.index() as usize], true) {
            return Ok(());
        }
        self.unit(UnitId {
            segment,
            unit: UnitIndex::ENTRY,
        })?;
        for (index, unit) in shape.units().iter().enumerate() {
            self.unit(UnitId {
                segment,
                unit: UnitIndex::of_run(index),
            })?;
            for born in self.births(shape, unit.work) {
                self.body(born)?;
            }
        }
        self.close(segment)
    }

    /// Run `unit`, or absorb its record where the store holds one its binding does not wake.
    fn unit(&mut self, unit: UnitId) -> Result<(), ShapeError<'o>> {
        if let Some((_, frontier)) = self.store.fact(unit)
            && !self.store.wakes(frontier)
        {
            return Ok(());
        }
        let facts = resident(self.store.lay(), UnitFacts::EMPTY);
        self.store.write(unit, facts, Frontier::EMPTY);
        Ok(())
    }

    /// Compute the closing record of the owning body `segment`.
    fn close(&mut self, segment: Segment) -> Result<(), ShapeError<'o>> {
        if let Some((_, frontier)) = self.store.close(segment)
            && !self.store.wakes(frontier)
        {
            return Ok(());
        }
        let closing = resident(
            self.store.lay(),
            Closing {
                class: None,
                exits: false,
                summary: None,
                recursive: false,
                ran: None,
                refusal: None,
            },
        );
        self.store.write_close(segment, closing, Frontier::EMPTY);
        Ok(())
    }

    /// Each callable and quote's code the unit doing `work` of `shape` births, its inline bodies'
    /// included, in program order.
    fn births(
        &self,
        shape: &'graph BodyShape<'graph>,
        work: UnitWork,
    ) -> BumpVec<'p, &'graph BodyShape<'graph>> {
        let mut born = BumpVec::new_in(self.scratch);
        match work {
            UnitWork::Statement(index) => births_within(shape, index as usize, &mut born),
            UnitWork::Component(index) => {
                for member in shape.components()[index.index()].members {
                    if let Some(index) = shape.position_of(*member).statement_index() {
                        births_within(shape, index, &mut born);
                    }
                }
            }
        }
        born
    }
}

/// Each callable and quote's code statement `index` of `shape` births, through the inline bodies
/// it runs, in program order.
fn births_within<'graph>(
    shape: &'graph BodyShape<'graph>,
    index: usize,
    born: &mut BumpVec<'_, &'graph BodyShape<'graph>>,
) {
    for (_, nested) in shape.nested_in(index) {
        match nested.kind() {
            ShapeKind::Callable | ShapeKind::Code => born.push(nested),
            ShapeKind::Block | ShapeKind::Module => {
                for inner in 0..nested.body().len() {
                    births_within(nested, inner, born);
                }
            }
            ShapeKind::Program => unreachable!("the program nests in nothing"),
        }
    }
}
