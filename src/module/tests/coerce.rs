//! Members born coerced: what each shape of declared slot does when the view mints its carrier.

use std::ptr;

use crate::function::tests::{declared, pin, with_fixture};
use crate::type_lattice::{KType, TypeNode};
use crate::values::{Knotted as _, Resolved, Value};

use super::super::coerce::CoercionRefused;
use super::super::view::{Ascription, Unascribable, ascribe};
use super::{member, module};

const BAG: &str = "\
SIG Bag = ((TYPE Carrier) \
(VAL one :Carrier) \
(VAL many :(LIST OF Carrier)) \
(VAL none :(LIST OF Carrier)) \
(VAL by_name :(MAP Str -> Carrier)) \
(VAL pair :{a :Carrier, b :Number}) \
(VAL maybe :(Carrier | Null)) \
(VAL step :(FN :{x :Carrier} -> Carrier)) \
(VAL plain :Number))
MODULE m = ((LET Carrier = Number) \
(LET one = 1) \
(LET many = [1 2]) \
(LET none = []) \
(LET by_name = {\"a\": 1}) \
(LET pair = {a = 1 b = 2}) \
(LET maybe = 3) \
(LET step = (FN :{x :Number} -> Number = (x))) \
(LET plain = 7))";

#[test]
fn every_slot_that_names_the_carrier_is_born_at_the_mint() {
    with_fixture(|fixture| {
        let lines = fixture.parse(BAG);
        let (types, scratch) = (fixture.types, fixture.scratch());
        fixture.in_cell(pin, |context, binder| {
            let writer = context.writer();
            let activation = fixture.run(writer, &lines, binder, &[]);
            let m = module(fixture, activation, "m");
            let bag = declared(fixture, activation, "Bag");
            let view = ascribe(writer, m, bag, Ascription::Opaque, types, scratch)
                .unwrap_or_else(|error| panic!("`m` satisfies `Bag`: {error:?}"));
            let read = |name| member(fixture, view, name, types, scratch);

            let Value::Type(carrier) = read("Carrier") else {
                panic!("`Carrier` is a type member");
            };
            let mint = carrier.handle();
            let list_of_mint = types.list(mint);

            // A bare slot seals: one tagged layer at the mint over the source's own word.
            let Value::Tagged(one) = read("one") else {
                panic!("a slot at the carrier seals");
            };
            assert_eq!(one.ktype(), mint);
            assert!(matches!(one.payload(), Value::Number(n) if *n == 1.0));

            // A container is rebuilt cell by cell and re-stamped.
            let Value::List(many) = read("many") else {
                panic!("a list slot stays a list");
            };
            assert_eq!(many.ktype(), list_of_mint);
            assert_eq!(many.len(), 2);
            for cell in many.cells() {
                assert_eq!(cell.ktype(), mint);
            }

            // An empty list has no cell to join, so the plain door lands on `LIST OF Never` and
            // the re-stamp is what gives it the declared memo.
            let Value::List(none) = read("none") else {
                panic!("an empty list slot stays a list");
            };
            assert!(none.is_empty());
            assert_eq!(none.ktype(), list_of_mint);

            let Value::Dict(by_name) = read("by_name") else {
                panic!("a dict slot stays a dict");
            };
            assert_eq!(by_name.ktype(), types.dict(KType::STR, mint));
            assert_eq!(by_name.cells()[0].ktype(), mint);

            // A record coerces each field against its own declared type, so `b :Number` is
            // carried while `a :Carrier` seals.
            let Value::Record(pair) = read("pair") else {
                panic!("a record slot stays a record");
            };
            let a = fixture.name("a").symbol();
            let b = fixture.name("b").symbol();
            assert_eq!(pair.field(a).expect("the field `a`").ktype(), mint);
            assert_eq!(pair.field(b).expect("the field `b`").ktype(), KType::NUMBER);
            assert_eq!(
                pair.ktype(),
                types.record(
                    scratch,
                    &[
                        (fixture.name("a"), mint),
                        (fixture.name("b"), KType::NUMBER),
                    ]
                )
            );

            // A union coerces by the one declared member the value's type admits.
            let Value::Tagged(maybe) = read("maybe") else {
                panic!("the union's number arm seals");
            };
            assert_eq!(maybe.ktype(), mint);

            // A slot naming no abstract member is carried verbatim: both sides agree, so the walk
            // stops at once.
            assert!(matches!(read("plain"), Value::Number(n) if n == 7.0));
        })
    });
}

#[test]
fn a_function_member_is_born_behind_a_barrier() {
    with_fixture(|fixture| {
        let lines = fixture.parse(BAG);
        let (types, scratch) = (fixture.types, fixture.scratch());
        fixture.in_cell(pin, |context, binder| {
            let writer = context.writer();
            let activation = fixture.run(writer, &lines, binder, &[]);
            let m = module(fixture, activation, "m");
            let bag = declared(fixture, activation, "Bag");
            let source_step = member(fixture, m, "step", types, scratch);
            let view = ascribe(writer, m, bag, Ascription::Opaque, types, scratch)
                .unwrap_or_else(|error| panic!("`m` satisfies `Bag`: {error:?}"));

            let Value::Type(carrier) = member(fixture, view, "Carrier", types, scratch) else {
                panic!("`Carrier` is a type member");
            };
            let mint = carrier.handle();
            let Value::Knotted(step) = member(fixture, view, "step", types, scratch) else {
                panic!("a function slot stays a knot member");
            };
            let barrier = step.coerced().expect("a function member is wrapped");
            assert!(matches!(step.resolve(), Resolved::Function));
            assert!(
                step.function().is_none(),
                "the wrapper is not itself a function"
            );

            // The caller sees the function at the view's types; the declaration it recurses on and
            // the two substitutions ride along for the call to use.
            let TypeNode::KFunction { params, ret } = types.node(barrier.ktype()) else {
                panic!("a barrier stands for a function");
            };
            assert_eq!(ret, mint);
            assert_eq!(params.get(fixture.name("x").symbol()), Some(mint));

            let Value::Knotted(source_step) = source_step else {
                panic!("`m.step` is a knot member");
            };
            assert!(
                ptr::eq(barrier.underlying().node(), source_step.node()),
                "the barrier stands over the source's own member"
            );
            assert!(barrier.underlying().function().is_some());
        })
    });
}

#[test]
fn a_transparent_view_coerces_nothing() {
    with_fixture(|fixture| {
        let lines = fixture.parse(BAG);
        let (types, scratch) = (fixture.types, fixture.scratch());
        fixture.in_cell(pin, |context, binder| {
            let writer = context.writer();
            let activation = fixture.run(writer, &lines, binder, &[]);
            let m = module(fixture, activation, "m");
            let bag = declared(fixture, activation, "Bag");
            let view = ascribe(writer, m, bag, Ascription::Transparent, types, scratch)
                .unwrap_or_else(|error| panic!("`m` satisfies `Bag`: {error:?}"));

            // Both sides of every slot substitute to the same type, so each member is the source's
            // own word — the same pointer, not an equal rebuild.
            for name in ["many", "by_name", "pair", "step"] {
                let (before, after) = (
                    member(fixture, m, name, types, scratch),
                    member(fixture, view, name, types, scratch),
                );
                assert_eq!(before.ktype(), after.ktype(), "`{name}` keeps its type");
                assert!(
                    same_referent(before, after),
                    "`{name}` is carried, not rebuilt"
                );
            }
        })
    });
}

/// Whether two values point at the same thing — a stronger claim than equality, and the one a
/// transparent view makes.
fn same_referent(
    left: Value<'_, '_, crate::function::Knotted<'_, '_>>,
    right: Value<'_, '_, crate::function::Knotted<'_, '_>>,
) -> bool {
    match (left, right) {
        (Value::List(left), Value::List(right)) => ptr::eq(left, right),
        (Value::Dict(left), Value::Dict(right)) => ptr::eq(left, right),
        (Value::Record(left), Value::Record(right)) => ptr::eq(left, right),
        (Value::Tagged(left), Value::Tagged(right)) => ptr::eq(left, right),
        (Value::Knotted(left), Value::Knotted(right)) => ptr::eq(left.node(), right.node()),
        _ => false,
    }
}

#[test]
fn a_signature_typed_slot_is_carried_whichever_operator_ascribes() {
    // A `SIG` canonicalizes its own abstract members to one binder, so a signature standing in a
    // slot of another never references the enclosing signature's members: its two sides always
    // agree and the nested module is carried, not re-viewed.
    let source = "\
SIG Inner = ((TYPE Elem) (VAL v :Elem))
SIG Outer = ((TYPE Carrier) (VAL one :Carrier) (VAL inner :Inner))
MODULE m = ((LET Carrier = Number) (LET one = 1) \
(MODULE inner = ((LET Elem = Number) (LET v = 5))))";
    with_fixture(|fixture| {
        let lines = fixture.parse(source);
        let (types, scratch) = (fixture.types, fixture.scratch());
        fixture.in_cell(pin, |context, binder| {
            let writer = context.writer();
            let activation = fixture.run(writer, &lines, binder, &[]);
            let m = module(fixture, activation, "m");
            let outer = declared(fixture, activation, "Outer");
            let source_inner = member(fixture, m, "inner", types, scratch);
            let view = ascribe(writer, m, outer, Ascription::Opaque, types, scratch)
                .unwrap_or_else(|error| panic!("`m` satisfies `Outer`: {error:?}"));

            let (Value::Knotted(before), Value::Knotted(after)) =
                (source_inner, member(fixture, view, "inner", types, scratch))
            else {
                panic!("`inner` is a module member");
            };
            assert!(
                ptr::eq(before.node(), after.node()),
                "a signature-typed slot names no mint, so its member is carried"
            );
            // The carrier still mints, so the view is genuinely opaque; only this slot is untouched.
            assert_ne!(
                member(fixture, view, "one", types, scratch).ktype(),
                KType::NUMBER
            );
        })
    });
}

#[test]
fn a_cyclic_data_member_refuses_the_barrier() {
    // A container that is a knot's data node is a knot member, not a container word, so the arm
    // its declaration takes has nothing to rebuild. Nobody yet rebuilds a cycle through a barrier.
    let source = "\
SIG Bag = ((TYPE Carrier) (VAL ring :(LIST OF Carrier)))
MODULE m = ((LET Carrier = Any) (LET ring = [1 spin]) \
(LET spin = (FN :{} -> Any = (ring))))";
    with_fixture(|fixture| {
        let lines = fixture.parse(source);
        let (types, scratch) = (fixture.types, fixture.scratch());
        fixture.in_cell(pin, |context, binder| {
            let writer = context.writer();
            let activation = fixture.run(writer, &lines, binder, &[]);
            let m = module(fixture, activation, "m");
            let bag = declared(fixture, activation, "Bag");
            assert!(
                matches!(
                    ascribe(writer, m, bag, Ascription::Opaque, types, scratch),
                    Err(Unascribable::Coercion {
                        refused: CoercionRefused::Unsupported(_),
                        ..
                    }),
                ),
                "a cyclic member is nobody's to coerce yet"
            );
        })
    });
}
