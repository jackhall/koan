//! Members born coerced: what each shape of declared slot does when the view makes its carrier.

use std::ptr;

use crate::knot::tests::{declared, pin, with_fixture};
use crate::symbols::BinderSymbol;
use crate::type_lattice::{ContentKey, KType, Members, TypeNode};
use crate::values::tests::parts;
use crate::values::{Knotted as _, Record, Resolved, Value};

use super::super::coerce::{Coercion, CoercionRefused, coerce};
use super::super::view::{Ascription, Unascribable, ascribe};
use super::{member, module};

const BAG: &str = "\
SIG Bag FOR ALL #[Carrier] = #[\
(VAL one :Carrier) \
(VAL many :(LIST OF Carrier)) \
(VAL none :(LIST OF Carrier)) \
(VAL by_name :(MAP Str -> Carrier)) \
(VAL pair :{a :Carrier, b :Number}) \
(VAL wide :{a :Carrier}) \
(VAL maybe :(Carrier | Null)) \
(VAL step :(FN :{x :Carrier} -> Carrier)) \
(VAL plain :Number)]
MODULE m = ((LET Carrier = Number) \
(LET one = 1) \
(LET many = [1 2]) \
(LET none = []) \
(LET by_name = {\"a\": 1}) \
(LET pair = {a = 1 b = 2}) \
(LET wide = {a = 1 b = 2}) \
(LET maybe = 3) \
(LET step = (FN :{x :Number} -> Number = #(x))) \
(LET plain = 7))";

#[test]
fn every_slot_that_names_the_carrier_is_born_at_it() {
    with_fixture(|fixture| {
        let lines = fixture.parse(BAG);
        let (types, scratch) = (fixture.types, fixture.scratch());
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let activation = fixture.run(writer, &lines, &[]);
            let m = module(fixture, activation, "m");
            let bag = declared(fixture, activation, "Bag");
            let view = ascribe(writer, m, bag, Ascription::Opaque, types, scratch)
                .unwrap_or_else(|error| panic!("`m` satisfies `Bag`: {error:?}"));
            let read = |name| member(fixture, view, name, types, scratch);

            let Value::Type(carrier) = read("Carrier") else {
                panic!("`Carrier` is a type member");
            };
            let hidden = carrier.handle();
            let list_of_hidden = types.list(hidden);

            // A bare slot seals: one tagged layer at the carrier over the source's own word.
            let Value::Tagged(one) = read("one") else {
                panic!("a slot at the carrier seals");
            };
            assert_eq!(one.ktype(), hidden);
            assert!(matches!(one.payload(), Value::Number(n) if *n == 1.0));

            // A container is rebuilt cell by cell and re-stamped.
            let Value::List(many) = read("many") else {
                panic!("a list slot stays a list");
            };
            assert_eq!(many.ktype(), list_of_hidden);
            assert_eq!(many.len(), 2);
            for cell in parts(Value::List(many), types, scratch) {
                assert_eq!(cell.concrete_ktype(), hidden);
            }

            // An empty list has no cell to join, so the plain door lands on `LIST OF Never` and
            // the re-stamp is what gives it the declared memo.
            let Value::List(none) = read("none") else {
                panic!("an empty list slot stays a list");
            };
            assert!(none.is_empty());
            assert_eq!(none.ktype(), list_of_hidden);

            let Value::Dict(by_name) = read("by_name") else {
                panic!("a dict slot stays a dict");
            };
            assert_eq!(by_name.ktype(), types.dict(KType::STR, hidden));
            assert_eq!(
                parts(Value::Dict(by_name), types, scratch)[0].concrete_ktype(),
                hidden
            );

            // A record coerces each field against its own declared type, so `b :Number` is
            // carried while `a :Carrier` seals.
            let Value::Record(pair) = read("pair") else {
                panic!("a record slot stays a record");
            };
            let a = fixture.name("a").symbol();
            let b = fixture.name("b").symbol();
            let field = |name| {
                Value::Record(pair)
                    .field(name, types, scratch)
                    .expect("the field")
                    .value()
            };
            assert_eq!(field(a).concrete_ktype(), hidden);
            assert_eq!(field(b).concrete_ktype(), KType::NUMBER);
            assert_eq!(
                pair.ktype(),
                types.record(
                    scratch,
                    &[
                        (fixture.name("a"), hidden),
                        (fixture.name("b"), KType::NUMBER),
                    ]
                )
            );

            // A record coerces the fields its slot declares, and keeps no other.
            let Value::Record(wide) = read("wide") else {
                panic!("a record slot stays a record");
            };
            assert_eq!(
                wide.ktype(),
                types.record(scratch, &[(fixture.name("a"), hidden)])
            );
            let fields = parts(Value::Record(wide), types, scratch);
            assert_eq!(fields[0].concrete_ktype(), hidden);
            let alone = Record::new(writer, &[(fixture.name("a"), fields[0])], types, scratch);
            assert_eq!(wide.weight(), alone.weight(), "`b` is not laid down");

            // A union coerces by the one declared member the value's type admits.
            let Value::Tagged(maybe) = read("maybe") else {
                panic!("the union's number arm seals");
            };
            assert_eq!(maybe.ktype(), hidden);

            // A slot naming no parameter is carried verbatim: both sides agree, so the walk
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
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let activation = fixture.run(writer, &lines, &[]);
            let m = module(fixture, activation, "m");
            let bag = declared(fixture, activation, "Bag");
            let source_step = member(fixture, m, "step", types, scratch);
            let view = ascribe(writer, m, bag, Ascription::Opaque, types, scratch)
                .unwrap_or_else(|error| panic!("`m` satisfies `Bag`: {error:?}"));

            let Value::Type(carrier) = member(fixture, view, "Carrier", types, scratch) else {
                panic!("`Carrier` is a type member");
            };
            let hidden = carrier.handle();
            let Value::Knotted(step) = member(fixture, view, "step", types, scratch) else {
                panic!("a function slot stays a knot member");
            };
            let barrier = step.coerced().expect("a function member is wrapped");
            assert!(matches!(step.resolve(), Resolved::Barrier));
            assert!(
                step.function().is_none(),
                "the wrapper is not itself a function"
            );

            // The caller sees the function at the view's types; the declaration it recurses on and
            // the two substitutions ride along for the call to use.
            let ktype = barrier
                .ktype()
                .as_type()
                .expect("the member is unquantified");
            let TypeNode::KFunction { params, ret, .. } = types.node(ktype) else {
                panic!("a barrier stands for a function");
            };
            assert_eq!(ret, hidden);
            assert_eq!(params.get(fixture.name("x").symbol()), Some(hidden));

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
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let activation = fixture.run(writer, &lines, &[]);
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
fn same_referent<'graph>(
    left: Value<'_, crate::knot::Knotted<'graph, '_>>,
    right: Value<'_, crate::knot::Knotted<'graph, '_>>,
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
fn a_signature_typed_slot_naming_no_member_is_carried() {
    // A declared signature is closed, so one standing in another's slot names none of the
    // enclosing signature's parameters unless an application pins one to it. Where none does,
    // both sides agree and the module is carried.
    let source = "\
SIG Inner FOR ALL #[Elem] = #[(VAL v :Elem)]
SIG Outer FOR ALL #[Carrier] = #[(VAL one :Carrier) (VAL inner :Inner)]
MODULE m = ((LET Carrier = Number) (LET one = 1) \
(MODULE inner = ((LET Elem = Number) (LET v = 5))))";
    with_fixture(|fixture| {
        let lines = fixture.parse(source);
        let (types, scratch) = (fixture.types, fixture.scratch());
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let activation = fixture.run(writer, &lines, &[]);
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
                "a signature naming no carrier carries its module"
            );
            // The carrier is still made, so the view is opaque; only this slot is untouched.
            assert_ne!(
                member(fixture, view, "one", types, scratch).concrete_ktype(),
                KType::NUMBER
            );
        })
    });
}

#[test]
fn a_nested_module_is_re_viewed_at_the_outer_carrier() {
    // A nested signature reaches an enclosing parameter only through an application — `VAL inner
    // :(Inner WITH {Elem = Carrier})`. The declared type is built here as that elaboration builds
    // it, and the coercion door is driven directly: what is pinned is that the nested view's
    // `Elem` *is* the outer carrier, arriving through the declaration rather than made again at
    // the boundary.
    let source = "\
SIG Inner FOR ALL #[Elem] = #[(VAL v :Elem)]
MODULE m = ((MODULE inner = ((LET Elem = Number) (LET v = 5))))";
    with_fixture(|fixture| {
        let lines = fixture.parse(source);
        let (types, scratch) = (fixture.types, fixture.scratch());
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let activation = fixture.run(writer, &lines, &[]);
            let m = module(fixture, activation, "m");
            let inner = declared(fixture, activation, "Inner");

            let BinderSymbol::Type(carrier) = fixture.name("Carrier") else {
                panic!("`Carrier` is a Type token");
            };
            // What `Inner WITH {Elem = Carrier}` elaborates to: `Elem` pinned to the enclosing
            // signature's own parameter `Carrier`.
            let reference = types.head_parameter(carrier, KType::ANY);
            let slot = types.signature_apply(scratch, inner, &[(fixture.name("Elem"), reference)]);

            let hidden = types.carrier(carrier, KType::ANY, ContentKey(7));
            let cx = Coercion {
                writer,
                types,
                scratch,
                from: Members::from_pairs(scratch, [(carrier, KType::NUMBER)]),
                to: Members::from_pairs(scratch, [(carrier, hidden)]),
            };

            let held = member(fixture, m, "inner", types, scratch);
            let Value::Knotted(view) =
                coerce(&cx, held, slot.into()).expect("the nested module re-views")
            else {
                panic!("a signature slot stays a module");
            };
            let Value::Knotted(before) = held else {
                panic!("`m.inner` is a module member");
            };
            assert!(
                !ptr::eq(view.node(), before.node()),
                "a nested module naming the carrier is re-viewed, not carried"
            );

            let Value::Type(nested_elem) = member(fixture, view, "Elem", types, scratch) else {
                panic!("`Elem` is a type member");
            };
            assert_eq!(
                nested_elem.handle(),
                hidden,
                "no carrier is made at a nested boundary"
            );
            assert_eq!(
                member(fixture, view, "v", types, scratch).concrete_ktype(),
                hidden,
                "and the nested member is sealed at it"
            );
        })
    });
}

#[test]
fn a_signature_slot_over_something_that_is_no_module_is_refused() {
    let source = "\
SIG Inner FOR ALL #[Elem] = #[(VAL v :Elem)]
LET f = (FN :{} -> Number = #(1))";
    with_fixture(|fixture| {
        let lines = fixture.parse(source);
        let (types, scratch) = (fixture.types, fixture.scratch());
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let activation = fixture.run(writer, &lines, &[]);
            let inner = declared(fixture, activation, "Inner");
            let BinderSymbol::Type(carrier) = fixture.name("Carrier") else {
                panic!("`Carrier` is a Type token");
            };
            let reference = types.head_parameter(carrier, KType::ANY);
            let slot = types.signature_apply(scratch, inner, &[(fixture.name("Elem"), reference)]);
            let hidden = types.carrier(carrier, KType::ANY, ContentKey(7));
            let cx = Coercion {
                writer,
                types,
                scratch,
                from: Members::from_pairs(scratch, [(carrier, KType::NUMBER)]),
                to: Members::from_pairs(scratch, [(carrier, hidden)]),
            };
            let f = crate::knot::tests::callable(fixture, activation, "f");
            for value in [Value::Knotted(f), Value::Null] {
                assert_eq!(
                    coerce(&cx, value, slot.into()).err(),
                    Some(CoercionRefused::NotAModule)
                );
            }
        })
    });
}

#[test]
fn a_cyclic_data_member_refuses_the_barrier() {
    // A container that is a knot's data node is a knot member, not a container word, so the arm
    // its declaration takes has nothing to rebuild. Nobody yet rebuilds a cycle through a barrier.
    let source = "\
SIG Bag FOR ALL #[Carrier] = #[(VAL ring :(LIST OF Carrier))]
MODULE m = ((LET Carrier = Any) (LET ring = [1 spin]) \
(LET spin = (FN :{} -> Any = #(ring))))";
    with_fixture(|fixture| {
        let lines = fixture.parse(source);
        let (types, scratch) = (fixture.types, fixture.scratch());
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let activation = fixture.run(writer, &lines, &[]);
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

/// A slot over `Carrier | Number` where the source binds `Carrier` to `Number`, and one over two
/// parameters the source binds alike.
const UNIONS: &str = "\
SIG Either FOR ALL #[Carrier] = #[(VAL v :(Carrier | Number))]
SIG Twins FOR ALL #[Carrier Other] = #[(VAL a :Carrier) (VAL b :Other) (VAL v :(Carrier | Other))]
MODULE m = ((LET a = 1) (LET b = 2) (LET v = 3))";

#[test]
fn a_union_member_naming_a_hidden_parameter_takes_the_value_whatever_the_union_s_order() {
    with_fixture(|fixture| {
        let lines = fixture.parse(UNIONS);
        let (types, scratch) = (fixture.types, fixture.scratch());
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let activation = fixture.run(writer, &lines, &[]);
            let m = module(fixture, activation, "m");
            let either = declared(fixture, activation, "Either");
            let view = ascribe(writer, m, either, Ascription::Opaque, types, scratch)
                .unwrap_or_else(|error| panic!("`m` satisfies `Either`: {error:?}"));
            let Value::Type(carrier) = member(fixture, view, "Carrier", types, scratch) else {
                panic!("`Carrier` is a type member");
            };
            assert_eq!(
                member(fixture, view, "v", types, scratch).concrete_ktype(),
                carrier.handle(),
                "both members admit a number, and the one over the carrier is taken"
            );

            let twins = declared(fixture, activation, "Twins");
            let refused = ascribe(writer, m, twins, Ascription::Opaque, types, scratch);
            assert!(
                matches!(
                    refused,
                    Err(Unascribable::Coercion {
                        refused: CoercionRefused::TiedUnion,
                        ..
                    })
                ),
                "two members over hidden parameters both admit the value: {refused:?}"
            );
        })
    });
}
