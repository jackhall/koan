//! Lexical variables: a name only a run binds, read at load. The examples pin what no law states —
//! that no binder captures one, that the order reads it as a rigid variable, that no solve binds it,
//! and that its identity is every field.

use crate::memory::{Bump, BumpAllocator};
use crate::symbols::{BinderSymbol, SymbolInterner, TypeSymbol};

use crate::type_lattice::handle::{Handle, KType, TypeHandle};
use crate::type_lattice::order::is_subtype_of;
use crate::type_lattice::registry::TypeRegistry;
use crate::type_lattice::render::display_handle;
use crate::type_lattice::substitute::{bound_above, substitute_levels, substitute_quantified};
use crate::type_lattice::unify::{Collector, admits};
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

    fn name(&self, text: &str) -> TypeSymbol {
        TypeSymbol::declared(text, &self.symbols).expect("a Type token")
    }

    fn field(&self, text: &str) -> BinderSymbol {
        BinderSymbol::declared(text, &self.symbols).expect("a bindable token")
    }

    /// The lexical variable at `level`, named `text`, bounded by `bound`.
    fn lexical(&self, level: usize, text: &str, bound: KType) -> Handle {
        self.types.lexical(level, self.name(text), bound).raw()
    }

    /// `FOR ALL #[<group>] FN :{<name> :<param>} -> <ret>`, each variable bounded by `Any`.
    fn function(&self, group: &[&str], name: &str, param: Handle, ret: Handle) -> Handle {
        let names: Vec<TypeSymbol> = group.iter().map(|text| self.name(text)).collect();
        let bounds = vec![KType::ANY; names.len()];
        self.types
            .function_group(
                self.region,
                &names,
                &bounds,
                &[(self.field(name), param)],
                ret,
            )
            .0
    }

    fn below(&self, a: Handle, b: Handle) -> bool {
        is_subtype_of(&self.types, self.region, a, b)
    }
}

#[test]
fn no_binder_captures_a_lexical_variable() {
    let bump = Bump::new();
    let world = World::new(&bump);
    let tee = world.types.quantified(0, KType::ANY).raw();
    let elt = world.lexical(0, "Elt", KType::ANY);
    let f = world.function(&["Tee"], "t", tee, elt);
    assert_eq!(
        substitute_levels(&world.types, world.region, f, &[KType::NUMBER.raw()]),
        world.function(&["Tee"], "t", tee, KType::NUMBER.raw()),
        "a level substitutes under a quantified function",
    );
    assert_eq!(
        substitute_quantified(&world.types, world.region, f, &[KType::NUMBER.raw()]),
        f,
        "a quantifier substitution leaves a lexical variable alone",
    );
    let past = world.lexical(1, "Item", KType::ANY);
    assert_eq!(
        substitute_levels(&world.types, world.region, past, &[KType::NUMBER.raw()]),
        past,
        "a level past the bindings is kept",
    );
}

#[test]
fn a_lexical_variable_is_ordered_as_a_rigid_variable() {
    let bump = Bump::new();
    let world = World::new(&bump);
    let number_or_str = world
        .types
        .union_of(world.region, &[KType::NUMBER.raw(), KType::STR.raw()]);
    let elt = world.lexical(0, "Elt", KType::NUMBER);
    assert!(world.below(elt, elt));
    assert!(world.below(elt, KType::NUMBER.raw()));
    assert!(world.below(elt, number_or_str));
    assert!(!world.below(KType::NUMBER.raw(), elt));
    assert!(world.below(Handle::NEVER, elt));
    for other in [
        world.lexical(1, "Elt", KType::NUMBER),
        world.lexical(0, "Item", KType::NUMBER),
        world.lexical(0, "Elt", KType::ANY),
    ] {
        assert!(!world.below(elt, other) && !world.below(other, elt));
    }
}

/// A lexical variable with a lower end — what a class-by-class walk over static types mints: below
/// it lie itself and what lies under its lower end, and a contravariant read takes that end.
#[test]
fn a_lexical_variable_with_a_lower_end() {
    let bump = Bump::new();
    let world = World::new(&bump);
    let (types, region) = (&world.types, world.region);
    let number_or_str = types.union_of(region, &[KType::NUMBER, KType::STR]);
    let x = types
        .lexical_between(region, 0, world.name("Elt"), KType::NUMBER, number_or_str)
        .raw();
    let number_or_str = number_or_str.raw();
    assert!(world.below(Handle::NEVER, x));
    assert!(world.below(KType::NUMBER.raw(), x));
    assert!(!world.below(KType::STR.raw(), x));
    assert!(world.below(x, number_or_str));
    assert!(!world.below(x, KType::NUMBER.raw()));
    assert_eq!(
        bound_above(
            types,
            region,
            world.function(&[], "y", x, KType::NULL.raw())
        ),
        world.function(&[], "y", KType::NUMBER.raw(), KType::NULL.raw()),
    );
    // A carried function over such a variable fills a declared parameter under its lower end.
    let elt = types.quantified(0, KType::ANY).raw();
    let declared = world.function(&[], "y", types.list(elt), KType::NULL.raw());
    let w = types
        .lexical_between(
            region,
            0,
            world.name("Elt"),
            types.list(KType::NUMBER),
            KType::LIST_OF_ANY,
        )
        .raw();
    let carried = world.function(&[], "y", w, KType::NULL.raw());
    let mut collector = Collector::<Handle>::new(region, &[KType::ANY]);
    assert_eq!(
        admits(
            types,
            region,
            declared,
            carried,
            Variance::Co,
            &mut collector
        ),
        Ok(())
    );
    assert_eq!(
        collector.solve(types).map(|solution| solution.to_vec()),
        Ok(vec![KType::NUMBER.raw()])
    );
}

#[test]
fn no_solve_binds_a_lexical_variable() {
    let bump = Bump::new();
    let world = World::new(&bump);
    let elt = world.lexical(0, "Elt", KType::NUMBER);
    assert!(world.types.contains_rigid(elt));
    assert!(!world.types.contains_quantified(elt));
    let mut collector = Collector::<Handle>::new(world.region, &[]);
    assert_eq!(
        admits(
            &world.types,
            world.region,
            elt,
            elt,
            Variance::Co,
            &mut collector
        ),
        Ok(())
    );
    assert!(collector.solve(&world.types).unwrap().is_empty());
    let mut collector = Collector::<Handle>::new(world.region, &[]);
    assert!(
        admits(
            &world.types,
            world.region,
            elt,
            KType::NUMBER.raw(),
            Variance::Co,
            &mut collector
        )
        .is_err()
    );
}

#[test]
fn bound_above_reads_a_lexical_variable_by_variance() {
    let bump = Bump::new();
    let world = World::new(&bump);
    let elt = world.lexical(0, "Elt", KType::NUMBER);
    let f = world.function(&[], "y", elt, elt);
    assert_eq!(
        bound_above(&world.types, world.region, f),
        world.function(&[], "y", Handle::NEVER, KType::NUMBER.raw()),
    );
}

#[test]
fn every_field_of_a_lexical_variable_is_identity() {
    let bump = Bump::new();
    let world = World::new(&bump);
    let elt = world.lexical(0, "Elt", KType::ANY);
    assert_eq!(elt, world.lexical(0, "Elt", KType::ANY));
    for other in [
        world.lexical(1, "Elt", KType::ANY),
        world.lexical(0, "Item", KType::ANY),
        world.lexical(0, "Elt", KType::NUMBER),
    ] {
        assert_ne!(elt.digest(), other.digest());
    }
    assert_eq!(
        display_handle(elt, &world.types, &world.symbols).to_string(),
        "Elt"
    );
}
