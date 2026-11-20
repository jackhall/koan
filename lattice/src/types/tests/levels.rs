//! Lexical variables: a name only a run binds, read at load. The examples pin what no law states —
//! that no binder captures one, that the order reads it as a rigid variable, that no solve binds it,
//! that its identity is every field, and how a projection of one is spelled.

use crate::symbols::{BinderSymbol, SymbolInterner, TypeSymbol, ValueSymbol};
use bumpstorage::{Bump, BumpAllocator};

use super::raw::RawRegistry;
use super::raw::order::is_subtype_of;
use super::raw::substitute::{
    bound_above, substitute_levels, substitute_paths, substitute_quantified,
};
use super::raw::unify::admits;
use crate::types::chain::TypeChain;
use crate::types::handle::{Handle, KType, TypeHandle};
use crate::types::node::{ContentKey, PathClass, Step};
use crate::types::render::display_handle;
use crate::types::schema::{SchemaDraft, SigOrigin};
use crate::types::unify::Collector;
use crate::types::walk::Variance;

use super::RawSchemes;

/// A chain, an interner and a region.
struct World<'r> {
    region: BumpAllocator<'r>,
    types: TypeChain<'r, crate::types::tiers::BumpHalf<'r>>,
    symbols: SymbolInterner,
}

impl<'r> World<'r> {
    fn new(region: BumpAllocator<'r>) -> Self {
        World {
            region,
            types: crate::types::tests::tiered(region),
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
            .function_scheme_raw(
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
        substitute_levels(&world.types, world.region, f, &[Some(KType::NUMBER.raw())]),
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
        substitute_levels(
            &world.types,
            world.region,
            past,
            &[Some(KType::NUMBER.raw())]
        ),
        past,
        "a level past the bindings is kept",
    );
    let g = world.function(&["Tee"], "t", elt, past);
    assert_eq!(
        substitute_levels(
            &world.types,
            world.region,
            g,
            &[None, Some(KType::NUMBER.raw())]
        ),
        world.function(&["Tee"], "t", elt, KType::NUMBER.raw()),
        "a level left unbound keeps its variable beside one bound",
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
    let mut collector = Collector::<Handle>::new(types, region, &[KType::ANY]);
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

/// A union keeps a variable beside a member under its lower end, so meets over such contributions
/// reach equivalent but distinct handles in different orders: the solve must not read them in the
/// order the slots arrive.
#[test]
fn a_solve_over_a_lexical_variable_ignores_slot_order() {
    let bump = Bump::new();
    let world = World::new(&bump);
    let (types, region) = (&world.types, world.region);
    let lists = types.union_of(region, &[types.list(KType::NUMBER), types.list(KType::STR)]);
    let item = types
        .lexical_between(
            region,
            0,
            world.name("Item"),
            types.list(KType::NUMBER),
            lists,
        )
        .raw();
    let taking = |param: Handle| world.function(&[], "x", param, KType::NULL.raw());
    let declared = taking(types.quantified(0, KType::LIST_OF_ANY).raw());
    let arguments = [
        taking(types.union_of(region, &[item, KType::NUMBER.raw()])),
        taking(types.union_of(region, &[item, KType::STR.raw()])),
        taking(lists.raw()),
    ];
    let solve = |order: &mut dyn Iterator<Item = &Handle>| {
        let mut collector = Collector::<Handle>::new(types, region, &[KType::LIST_OF_ANY]);
        for argument in order {
            let admitted = admits(
                types,
                region,
                declared,
                *argument,
                Variance::Co,
                &mut collector,
            );
            assert_eq!(admitted, Ok(()));
        }
        collector.solve(types).map(|solution| solution.to_vec())
    };
    let forward = solve(&mut arguments.iter());
    assert!(forward.is_ok());
    assert_eq!(forward, solve(&mut arguments.iter().rev()));
}

#[test]
fn no_solve_binds_a_lexical_variable() {
    let bump = Bump::new();
    let world = World::new(&bump);
    let elt = world.lexical(0, "Elt", KType::NUMBER);
    assert!(world.types.contains_rigid(elt));
    assert!(!world.types.contains_quantified(elt));
    let mut collector = Collector::<Handle>::new(&world.types, world.region, &[]);
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
    let mut collector = Collector::<Handle>::new(&world.types, world.region, &[]);
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

#[test]
fn every_field_of_a_path_variable_is_identity() {
    let bump = Bump::new();
    let world = World::new(&bump);
    let path = |class, text: &str, bound| {
        (world.types)
            .path_variable(PathClass([class, 0]), world.name(text), bound)
            .raw()
    };
    let carrier = path(0, "Carrier", KType::ANY);
    assert_eq!(carrier, path(0, "Carrier", KType::ANY));
    for other in [
        path(1, "Carrier", KType::ANY),
        path(0, "Elt", KType::ANY),
        path(0, "Carrier", KType::NUMBER),
        world.lexical(0, "Carrier", KType::ANY),
    ] {
        assert_ne!(carrier.digest(), other.digest());
    }
    assert_eq!(
        display_handle(carrier, &world.types, &world.symbols).to_string(),
        "Carrier"
    );
}

#[test]
fn substituting_paths_replaces_each_class_it_answers_for_and_no_other_variable() {
    let bump = Bump::new();
    let world = World::new(&bump);
    let carrier = |class| {
        (world.types)
            .path_variable(PathClass([class, 0]), world.name("Carrier"), KType::ANY)
            .raw()
    };
    let (answered, kept) = (carrier(0), carrier(1));
    let elt = world.lexical(0, "Elt", KType::ANY);
    let number = KType::NUMBER.raw();
    let mut answer = |class| (class == PathClass([0, 0])).then_some(number);
    let mut substituted = |kt| substitute_paths(&world.types, world.region, kt, &mut answer);
    assert_eq!(substituted(answered), number);
    assert_eq!(
        substituted(world.types.list(answered)),
        world.types.list(number)
    );
    assert_eq!(
        substituted(kept),
        kept,
        "a class it does not answer for is kept"
    );
    assert_eq!(substituted(elt), elt, "a lexical variable is no path's");
    // No scheme captures a path variable: one in a quantified function's return is replaced.
    let tee = world.types.quantified(0, KType::ANY).raw();
    assert_eq!(
        substituted(world.function(&["Tee"], "t", tee, answered)),
        world.function(&["Tee"], "t", tee, number),
    );
}

#[test]
fn substituting_carriers_replaces_each_carrier_it_answers_for_at_its_position() {
    let bump = Bump::new();
    let world = World::new(&bump);
    let (types, region) = (&world.types, world.region);
    let carrier = |key| types.carrier(world.name("Carrier"), KType::ANY, ContentKey(key));
    let (answered, kept) = (carrier(1), carrier(2));
    let mut answer = |held: KType| (held == answered).then_some(KType::NUMBER);
    let mut substituted =
        |kt: KType| crate::types::substitute_carriers(types, region, kt, &mut answer);
    assert_eq!(substituted(answered), KType::NUMBER);
    assert_eq!(substituted(types.list(answered)), types.list(KType::NUMBER));
    assert_eq!(
        substituted(kept),
        kept,
        "a carrier it does not answer for is kept"
    );
    // A union re-interns through its canonical door: the answer is absorbed where it lies below
    // another member.
    let either = types.union_of(region, &[answered, KType::STR]);
    assert_eq!(
        substituted(either),
        types.union_of(region, &[KType::NUMBER, KType::STR])
    );
    let both = types.union_of(region, &[answered, KType::NUMBER]);
    assert_eq!(substituted(both), KType::NUMBER);
}

/// A carrier a signature holds is read back inside it — in a value slot, and in an application's
/// signature — and the signature re-interned through its canonical door: one type, one handle.
#[test]
fn substituting_carriers_reads_a_carrier_inside_a_signature() {
    let bump = Bump::new();
    let world = World::new(&bump);
    let (types, region) = (&world.types, world.region);
    let carrier = types.carrier(world.name("Carrier"), KType::ANY, ContentKey(1));
    let zero = ValueSymbol::declared("zero", &world.symbols).expect("a value token");
    let elem = world.name("Elem");
    let declared = |slot: KType| {
        let mut draft = SchemaDraft::new(region);
        draft.origin = SigOrigin::Declared;
        draft.insert_parameter(elem, types.head_parameter(elem, KType::ANY));
        draft.insert_value_slot(zero, slot);
        types.signature(region, draft)
    };
    let applied = |slot: KType| {
        types.signature_apply(
            region,
            declared(slot),
            &[(BinderSymbol::Type(elem), KType::STR)],
        )
    };
    let mut answer = |held: KType| (held == carrier).then_some(KType::NUMBER);
    let mut substituted =
        |kt: KType| crate::types::substitute_carriers(types, region, kt, &mut answer);
    assert_eq!(substituted(declared(carrier)), declared(KType::NUMBER));
    assert_eq!(substituted(applied(carrier)), applied(KType::NUMBER));
    assert_eq!(
        substituted(types.list(applied(carrier))),
        types.list(applied(KType::NUMBER))
    );
}

/// A projection renders by its step over its base, chained outward. No law states a spelling: the
/// words are render-only, so no parser round-trips them.
#[test]
fn a_projection_renders_its_step_over_its_base() {
    let bump = Bump::new();
    let world = World::new(&bump);
    let (types, region) = (&world.types, world.region);
    let (f, g) = (world.field("f"), world.field("g"));
    let inner = types.record(region, &[(g, KType::NUMBER)]);
    let elt = world.lexical(0, "Elt", types.record(region, &[(f, inner)]));
    let ll = world.lexical(0, "Ll", types.list(KType::NUMBER));
    let dd = world.lexical(0, "Dd", types.dict(KType::STR, KType::NUMBER));
    let project = |base: Handle, step| {
        types
            .projection(region, crate::types::handle::wrap(base), step)
            .expect("the bound guarantees the piece")
    };
    let render = |kt: crate::types::handle::Parametric| {
        display_handle(kt.raw(), types, &world.symbols).to_string()
    };
    let field = project(elt, Step::Field(f));
    assert_eq!(
        render(project(field.raw(), Step::Field(g))),
        "(FIELD g OF (FIELD f OF Elt))"
    );
    assert_eq!(render(project(ll, Step::Element)), "(ELEMENT OF Ll)");
    assert_eq!(render(project(dd, Step::Key)), "(KEY OF Dd)");
    assert_eq!(render(project(dd, Step::Value)), "(VALUE OF Dd)");
}
