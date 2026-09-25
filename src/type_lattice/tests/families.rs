//! Type-constructor families: the relation between an application and its bare family, the least
//! solution a construction takes, and the contravariance probe a declaration is refused by. Each
//! builds its own registry over a region of its own, which doubles as its scratch.

use crate::memory::{Bump, BumpAllocator};
use crate::symbols::{BinderSymbol, SymbolInterner, TypeSymbol};

use crate::type_lattice::handle::KType;
use crate::type_lattice::kind::KKind;
use crate::type_lattice::order::is_subtype_of;
use crate::type_lattice::registry::TypeRegistry;
use crate::type_lattice::unify::{Collector, admits_with};
use crate::type_lattice::walk::Variance;
use crate::type_lattice::window::{RecursiveGroupWindow, RelativeSchema};

/// A sealed one-parameter family named `name` over parameter `parameter`, wrapping the parameter.
fn family(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    name: TypeSymbol,
    parameter: TypeSymbol,
) -> KType {
    let window = RecursiveGroupWindow::new(scratch, &[(name, KKind::TypeConstructor)]);
    let representation = Some(types.quantified(0, KType::ANY));
    let schema = RelativeSchema::constructor(scratch, scratch, representation, &[parameter]);
    window
        .fill_member(0, schema, types, scratch)
        .and_then(|sealed| sealed.member(0))
        .expect("a singleton window seals on its fill")
}

/// The generated laws cover the arm's consistency with the rest of the order, but not that it is
/// strict or that it is keyed on the one family applied: a law has no way to name "a different
/// family" for its operands.
#[test]
fn an_application_lies_strictly_under_its_family() {
    let symbols = SymbolInterner::new();
    let declare = |text| TypeSymbol::declared(text, &symbols).expect("a Type token");
    let (boxed, other, parameter) = (declare("Boxed"), declare("Other"), declare("Elt"));
    let bump = Bump::new();
    let region = &bump;
    let types = TypeRegistry::in_region(region);
    let f = family(&types, region, boxed, parameter);
    let g = family(&types, region, other, parameter);
    assert_ne!(f, g);
    let apply = |constructor, argument| {
        types.constructor_apply(
            region,
            constructor,
            &[(BinderSymbol::Type(parameter), argument)],
        )
    };
    let number = apply(f, KType::NUMBER);
    assert!(is_subtype_of(&types, region, number, f));
    assert!(!is_subtype_of(&types, region, f, number));
    assert!(is_subtype_of(&types, region, number, apply(f, KType::ANY)));
    assert!(!is_subtype_of(&types, region, number, g));
    assert!(!is_subtype_of(
        &types,
        region,
        number,
        apply(g, KType::NUMBER)
    ));
}

/// A worked edge of the solver rather than a law: the two collectors differ only on a variable
/// nothing reached, which no generated admission names.
#[test]
fn least_solves_an_unreached_variable_to_never() {
    let symbols = SymbolInterner::new();
    let a = BinderSymbol::declared("a", &symbols).expect("a bindable token");
    let bump = Bump::new();
    let region = &bump;
    let types = TypeRegistry::in_region(region);
    let declared = types.record(region, &[(a, types.quantified(0, KType::ANY))]);
    let carried = types.record(region, &[(a, KType::NUMBER)]);
    let solve = |mut collector: Collector<'_>| {
        admits_with(
            &types,
            region,
            declared,
            carried,
            Variance::Co,
            &mut collector,
        )
        .expect("the record admits");
        collector
            .solve(&types)
            .expect("one contribution solves")
            .to_vec()
    };
    assert_eq!(
        solve(Collector::least(region, 2)),
        [KType::NUMBER, KType::NEVER]
    );
    assert_eq!(
        solve(Collector::new(region, 2)),
        [KType::NUMBER, KType::ANY]
    );
}

/// No law: the probe's polarity bookkeeping is a fact about where a variable sits, which a
/// property over generated types would restate rather than check.
#[test]
fn a_contravariant_occurrence_is_found() {
    let symbols = SymbolInterner::new();
    let x = BinderSymbol::declared("x", &symbols).expect("a bindable token");
    let f = BinderSymbol::declared("f", &symbols).expect("a bindable token");
    let parameter = TypeSymbol::declared("Elt", &symbols).expect("a Type token");
    let name = TypeSymbol::declared("Boxed", &symbols).expect("a Type token");
    let bump = Bump::new();
    let region = &bump;
    let types = TypeRegistry::in_region(region);
    let variable = types.quantified(0, KType::ANY);
    let function = |params: &[(BinderSymbol, KType)], ret| {
        types.function_type(region, &[], params, ret).handle
    };
    let sink = function(&[(x, variable)], KType::NULL);
    assert!(types.quantifies_contravariantly(region, sink, 1));
    let give = function(&[], variable);
    assert!(!types.quantifies_contravariantly(region, give, 1));
    let flipped_twice = function(&[(f, sink)], KType::NULL);
    assert!(!types.quantifies_contravariantly(region, flipped_twice, 1));
    let boxed = family(&types, region, name, parameter);
    let applied =
        types.constructor_apply(region, boxed, &[(BinderSymbol::Type(parameter), variable)]);
    assert!(!types.quantifies_contravariantly(region, applied, 1));
}
