//! Intervals: a collector's bounds, the interval a solve reports per variable, and a type read
//! through intervals. The worked examples a law cannot name; the interval law itself is in
//! [`properties`](super::properties).

use crate::memory::{Bump, BumpAllocator};
use crate::symbols::{BinderSymbol, SymbolInterner};

use crate::type_lattice::handle::KType;
use crate::type_lattice::registry::TypeRegistry;
use crate::type_lattice::substitute::{Side, read_through};
use crate::type_lattice::unify::{Collector, Interval, admits_with, intervals};
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
    fn function(&self, name: &str, param: KType, ret: KType) -> KType {
        let name = BinderSymbol::declared(name, &self.symbols).expect("a bindable token");
        self.types
            .function_type(self.region, &[], &[], &[(name, param)], ret)
            .handle
    }

    fn union(&self, members: &[KType]) -> KType {
        self.types.union_of(self.region, members)
    }

    /// The one variable of a group bounded by `bound`, solved over `declared` against `carried`,
    /// and its interval.
    fn interval(&self, bound: KType, declared: KType, carried: KType, exact: bool) -> Interval {
        let mut collector = Collector::new(self.region, &[bound]);
        admits_with(
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
    let elt = world.types.quantified(0, KType::NUMBER);
    let slot = world.union(&[elt, KType::NULL]);
    let mut collector = Collector::new(world.region, &[KType::NUMBER]);
    admits_with(
        &world.types,
        world.region,
        slot,
        KType::NULL,
        Variance::Co,
        &mut collector,
    )
    .expect("null admits");
    assert_eq!(
        collector.solve(&world.types).unwrap().as_slice(),
        [KType::NUMBER]
    );
}

#[test]
fn a_solve_reports_an_interval_per_variable() {
    let bump = Bump::new();
    let world = World::new(&bump);
    let elt = world.types.quantified(0, KType::ANY);
    assert_eq!(
        world.interval(KType::ANY, elt, KType::NUMBER, false),
        Interval::within(KType::NUMBER),
        "a bare slot reaches the variable from below",
    );
    assert_eq!(
        world.interval(KType::ANY, elt, KType::NUMBER, true),
        Interval::point(KType::NUMBER),
        "an exact argument pins it",
    );
    assert_eq!(
        world.interval(
            KType::ANY,
            world.function("x", elt, KType::NULL),
            world.function("x", KType::NUMBER, KType::NULL),
            false,
        ),
        Interval {
            lower: KType::NUMBER,
            upper: KType::ANY
        },
        "a contravariant position bounds it from below alone",
    );
    let list_of_number = world.types.list(KType::NUMBER);
    assert_eq!(
        world.interval(
            KType::ANY,
            world.union(&[world.types.list(elt), KType::NULL]),
            world.union(&[list_of_number, KType::NULL]),
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
    let elt = world.types.quantified(0, KType::ANY);
    let number_or_str = world.union(&[KType::NUMBER, KType::STR]);
    let f = world.function("y", elt, elt);
    let read = |side| {
        read_through(&world.types, world.region, f, side, &mut |_| {
            Some(Interval {
                lower: KType::NUMBER,
                upper: number_or_str,
            })
        })
    };
    assert_eq!(
        read(Side::Above),
        world.function("y", KType::NUMBER, number_or_str)
    );
    assert_eq!(
        read(Side::Below),
        world.function("y", number_or_str, KType::NUMBER)
    );
}
