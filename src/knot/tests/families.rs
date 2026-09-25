//! A construction through a family inside a knot: a nested one built through the checked door, a
//! tagged node whose memo is derived from its payload, the cycle such nodes alone make, and the
//! refusals the construction rule gives.

use crate::type_lattice::KType;
use crate::values::{Circular, ConstructionRefused, Knotted as _, Link, Value};

use super::super::Untieable;
use super::birth::tie_of;
use super::{Fixture, bound, callable, circular, declared, pin, with_fixture};

/// `family` applied at `Type = argument`.
fn boxed_at(fixture: &Fixture<'_, '_>, family: KType, argument: KType) -> KType {
    fixture.types.constructor_apply(
        fixture.scratch(),
        family,
        &[(fixture.name("Type"), argument)],
    )
}

#[test]
fn a_nested_family_construction_takes_its_solved_application() {
    with_fixture(|fixture| {
        let lines = fixture
            .parse("NEWTYPE (Type AS Boxed)\nLET a = [(Boxed 7) f]\nLET f = (FN :{} -> Any = (a))");
        fixture.in_cell(pin, |context| {
            let activation = fixture.run(context.writer(), &lines, &[]);
            let boxed = declared(fixture, activation, "Boxed");
            let (_, Circular::List(list)) = circular(bound(fixture, activation, "a")) else {
                panic!("`a` is a list node");
            };
            let Link::Value(Value::Tagged(tagged)) = list.cells()[0] else {
                panic!("the construction is an ordinary tagged value");
            };
            assert_eq!(tagged.ktype(), boxed_at(fixture, boxed, KType::NUMBER));
        });
    });
}

#[test]
fn a_family_node_derives_its_type_from_its_payload() {
    with_fixture(|fixture| {
        let lines = fixture
            .parse("NEWTYPE (Type AS Boxed)\nLET a = (Boxed [f])\nLET f = (FN :{} -> Any = (a))");
        fixture.in_cell(pin, |context| {
            let activation = fixture.run(context.writer(), &lines, &[]);
            let boxed = declared(fixture, activation, "Boxed");
            let f = callable(fixture, activation, "f");
            let (_, Circular::Tagged(tagged)) = circular(bound(fixture, activation, "a")) else {
                panic!("`a` is a tagged node");
            };
            let listed = fixture.types.list(f.ktype());
            assert_eq!(tagged.ktype(), boxed_at(fixture, boxed, listed));
        });
    });
}

#[test]
fn a_cycle_through_a_family_construction_alone_refuses() {
    with_fixture(|fixture| {
        let lines = fixture.parse("NEWTYPE (Type AS Boxed)\nLET a = (Boxed [a])");
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let activation = fixture.run(writer, &lines, &["a"]);
            let Err(Untieable::TypeCycle { names }) = tie_of(fixture, writer, activation, "a")
            else {
                panic!("a family node holding itself has no finite type");
            };
            assert_eq!(names, [fixture.name("a")]);
        });
    });
}

#[test]
fn a_family_construction_the_rule_refuses_refuses_the_tie() {
    with_fixture(|fixture| {
        let lines = fixture.parse(
            "UNION (Elem AS Opt) = (Some :{value :Elem} None :Null)\nLET SomeOf = :(Opt.Some)\n\
             LET a = (SomeOf {other = f})\nLET f = (FN :{} -> Any = (a))\n\
             NEWTYPE (Key Val AS Pair)\nLET c = (Pair [g])\nLET g = (FN :{} -> Any = (c))",
        );
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let activation = fixture.run(writer, &lines, &["a", "f", "c", "g"]);
            let some = declared(fixture, activation, "SomeOf");
            assert!(matches!(
                tie_of(fixture, writer, activation, "a"),
                Err(Untieable::Construction {
                    refused: ConstructionRefused::Unsolved { family, .. },
                    ..
                }) if family == some
            ));
            let pair = declared(fixture, activation, "Pair");
            assert!(matches!(
                tie_of(fixture, writer, activation, "c"),
                Err(Untieable::Construction {
                    refused: ConstructionRefused::NotConstructible(head),
                    ..
                }) if head == pair
            ));
        });
    });
}
