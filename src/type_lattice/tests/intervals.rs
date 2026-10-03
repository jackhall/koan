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

/// Whether a run reproduces a solve over static types naming the lexical variable `Outer`: a
/// contribution handed whole to a cell does, a read through `Outer`'s ends or a meet over it does
/// not, and a failure does where the failing verdict is concrete.
mod reproducible {
    use super::*;
    use crate::symbols::TypeSymbol;
    use crate::type_lattice::handle::wrap;

    /// The lexical variable `Outer` at level 0 under `bound`.
    fn outer(world: &World<'_>, bound: KType) -> Handle {
        let name = TypeSymbol::declared("Outer", &world.symbols).expect("a Type token");
        world.types.lexical(0, name, bound).raw()
    }

    /// `kt`, which the test builds closed, as a bound.
    fn closed(world: &World<'_>, kt: Handle) -> KType {
        world.types.concrete(wrap(kt)).expect("a closed type")
    }

    /// A one-variable collector after admitting each `(declared, carried)` pair, and whether every
    /// admission succeeded.
    fn collect<'w>(world: &World<'w>, pairs: &[(Handle, Handle)]) -> (Collector<'w, Handle>, bool) {
        let mut collector = Collector::<Handle>::new(world.region, &[KType::ANY]);
        let admitted = pairs.iter().all(|(declared, carried)| {
            admits(
                &world.types,
                world.region,
                *declared,
                *carried,
                Variance::Co,
                &mut collector,
            )
            .is_ok()
        });
        (collector, admitted)
    }

    #[test]
    fn a_contribution_handed_whole_to_a_cell_is_reproducible() {
        let bump = Bump::new();
        let world = World::new(&bump);
        let elt = world.types.quantified(0, KType::ANY).raw();
        let list_of_outer = world.types.list(outer(&world, KType::ANY));
        let (collector, admitted) = collect(&world, &[(elt, list_of_outer)]);
        assert!(admitted);
        assert_eq!(
            collector.solve(&world.types).unwrap().as_slice(),
            [list_of_outer]
        );
        assert!(collector.reproducible(&world.types));
    }

    #[test]
    fn a_read_through_a_variables_bound_is_not_reproducible() {
        let bump = Bump::new();
        let world = World::new(&bump);
        let elt = world.types.quantified(0, KType::ANY).raw();
        let numbers = world.types.list(KType::NUMBER.raw());
        let outer = outer(&world, closed(&world, numbers));
        let (collector, admitted) = collect(&world, &[(world.types.list(elt), outer)]);
        assert!(admitted);
        assert_eq!(
            collector.solve(&world.types).unwrap().as_slice(),
            [KType::NUMBER.raw()]
        );
        assert!(!collector.reproducible(&world.types));
    }

    #[test]
    fn a_split_bound_is_not_reproducible() {
        let bump = Bump::new();
        let world = World::new(&bump);
        let elt = world.types.quantified(0, KType::ANY).raw();
        let numbers = world.types.list(KType::NUMBER.raw());
        let outer = outer(
            &world,
            closed(&world, world.union(&[numbers, KType::STR.raw()])),
        );
        let declared = world.union(&[world.types.list(elt), KType::STR.raw()]);
        let (collector, admitted) = collect(&world, &[(declared, outer)]);
        assert!(admitted);
        assert!(!collector.reproducible(&world.types));
    }

    /// `Number | Elt` tries `Number` first; at a binding of `Outer` under `Number` it admits there.
    #[test]
    fn a_false_verdict_over_a_variable_is_not_reproducible() {
        let bump = Bump::new();
        let world = World::new(&bump);
        let elt = world.types.quantified(0, KType::ANY).raw();
        let outer = outer(&world, KType::ANY);
        let declared = world.union(&[KType::NUMBER.raw(), elt]);
        let (collector, admitted) = collect(&world, &[(declared, outer)]);
        assert!(admitted);
        assert_eq!(collector.solve(&world.types).unwrap().as_slice(), [outer]);
        assert!(!collector.reproducible(&world.types));
    }

    #[test]
    fn a_meet_over_a_variable_is_not_reproducible() {
        let bump = Bump::new();
        let world = World::new(&bump);
        let elt = world.types.quantified(0, KType::ANY).raw();
        let outer = outer(&world, KType::ANY);
        let (collector, admitted) = collect(
            &world,
            &[(
                world.function("x", elt, KType::NULL.raw()),
                world.function("x", outer, KType::NULL.raw()),
            )],
        );
        assert!(admitted);
        assert_eq!(collector.solve(&world.types).unwrap().as_slice(), [outer]);
        assert!(!collector.reproducible(&world.types));
    }

    #[test]
    fn a_failure_over_a_variable_is_not_reproducible() {
        let bump = Bump::new();
        let world = World::new(&bump);
        let elt = world.types.quantified(0, KType::ANY).raw();
        let (mut collector, _) = collect(&world, &[]);
        collector.pin(0, KType::ANY, KType::NUMBER.raw());
        admits(
            &world.types,
            world.region,
            elt,
            outer(&world, KType::ANY),
            Variance::Co,
            &mut collector,
        )
        .expect("a bare variable admits");
        assert!(collector.solve(&world.types).is_err());
        assert!(!collector.reproducible(&world.types));
    }

    #[test]
    fn a_concrete_failure_is_reproducible() {
        let bump = Bump::new();
        let world = World::new(&bump);
        let elt = world.types.quantified(0, KType::ANY).raw();
        let (collector, admitted) = collect(
            &world,
            &[
                (elt, outer(&world, KType::ANY)),
                (KType::STR.raw(), KType::NUMBER.raw()),
            ],
        );
        assert!(!admitted);
        assert!(collector.reproducible(&world.types));
    }
}
