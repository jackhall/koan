//! Intervals: a collector's bounds, the interval a solve reports per variable, and a type read
//! through intervals. The worked examples a law cannot name; the interval law itself is in
//! [`properties`](super::properties).

use crate::memory::{Bump, BumpAllocator};
use crate::symbols::{BinderSymbol, SymbolInterner};

use crate::type_lattice::handle::{Handle, KType, TypeHandle};
use crate::type_lattice::registry::TypeRegistry;
use crate::type_lattice::substitute::{Side, read_through};
use crate::type_lattice::unify::{Collector, Interval, admits, intervals};
use crate::type_lattice::walk::Variance;

/// A registry, an interner and a region.
struct World<'r> {
    region: BumpAllocator<'r>,
    types: TypeRegistry<'r>,
    symbols: SymbolInterner,
}

impl<'r> World<'r> {
    fn new(region: BumpAllocator<'r>) -> Self {
        World {
            region,
            types: TypeRegistry::in_region(region),
            symbols: SymbolInterner::new(),
        }
    }

    /// `FN :{<name> :<param>} -> <ret>`, binding no group.
    fn function(&self, name: &str, param: Handle, ret: Handle) -> Handle {
        let name = BinderSymbol::declared(name, &self.symbols).expect("a bindable token");
        self.types.function_type(self.region, &[(name, param)], ret)
    }

    fn union(&self, members: &[Handle]) -> Handle {
        self.types.union_of(self.region, members)
    }

    /// The one variable of a group bounded by `bound`, solved over `declared` against `carried`,
    /// and its interval.
    fn interval(
        &self,
        bound: KType,
        declared: Handle,
        carried: Handle,
        exact: bool,
    ) -> Interval<Handle> {
        let mut collector = Collector::<Handle>::new(self.region, &[bound]);
        admits(
            &self.types,
            self.region,
            declared,
            carried,
            Variance::Co,
            &mut collector,
        )
        .expect("the argument admits");
        let solution = collector.solve(&self.types).expect("the group solves");
        intervals(
            &self.types,
            self.region,
            &[declared],
            &[bound],
            &solution,
            exact,
        )[0]
    }
}

/// Under `FOR ALL #{Elt: Number} #(F x :(Elt | Null)) -> Elt`, `F null` solves `Elt` to its bound.
#[test]
fn a_variable_no_contribution_reaches_solves_to_its_bound() {
    let bump = Bump::new();
    let world = World::new(&bump);
    let elt = world.types.quantified(0, KType::NUMBER).raw();
    let slot = world.union(&[elt, KType::NULL.raw()]);
    let mut collector = Collector::<Handle>::new(world.region, &[KType::NUMBER]);
    admits(
        &world.types,
        world.region,
        slot,
        KType::NULL.raw(),
        Variance::Co,
        &mut collector,
    )
    .expect("null admits");
    assert_eq!(
        collector.solve(&world.types).unwrap().as_slice(),
        [KType::NUMBER.raw()]
    );
}

#[test]
fn a_solve_reports_an_interval_per_variable() {
    let bump = Bump::new();
    let world = World::new(&bump);
    let elt = world.types.quantified(0, KType::ANY).raw();
    assert_eq!(
        world.interval(KType::ANY, elt, KType::NUMBER.raw(), false),
        Interval::within(KType::NUMBER),
        "a bare slot reaches the variable from below",
    );
    assert_eq!(
        world.interval(KType::ANY, elt, KType::NUMBER.raw(), true),
        Interval::point(KType::NUMBER.raw()),
        "an exact argument pins it",
    );
    assert_eq!(
        world.interval(
            KType::ANY,
            world.function("x", elt, KType::NULL.raw()),
            world.function("x", KType::NUMBER.raw(), KType::NULL.raw()),
            false,
        ),
        Interval {
            lower: KType::NUMBER.raw(),
            upper: Handle::ANY
        },
        "a contravariant position bounds it from below alone",
    );
    let list_of_number = world.types.list(KType::NUMBER.raw());
    assert_eq!(
        world.interval(
            KType::ANY,
            world.union(&[world.types.list(elt), KType::NULL.raw()]),
            world.union(&[list_of_number, KType::NULL.raw()]),
            false,
        ),
        Interval::within(KType::ANY),
        "an argument may miss a position under a union",
    );
}

/// `FN :{y :Elt} -> Elt` with `Elt` between `Number` and `Number | Str`.
#[test]
fn a_type_reads_through_intervals_by_variance() {
    let bump = Bump::new();
    let world = World::new(&bump);
    let elt = world.types.quantified(0, KType::ANY).raw();
    let number_or_str = world.union(&[KType::NUMBER.raw(), KType::STR.raw()]);
    let f = world.function("y", elt, elt);
    let read = |side| {
        read_through(&world.types, world.region, f, side, &mut |_| {
            Some(Interval {
                lower: KType::NUMBER.raw(),
                upper: number_or_str,
            })
        })
    };
    assert_eq!(
        read(Side::Above),
        world.function("y", KType::NUMBER.raw(), number_or_str)
    );
    assert_eq!(
        read(Side::Below),
        world.function("y", number_or_str, KType::NUMBER.raw())
    );
}
