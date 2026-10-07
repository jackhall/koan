//! The view door: what `:!` and `:|` build over a module, and what refuses one.

use crate::knot::tests::{declared, pin, with_fixture};
use crate::symbols::BinderSymbol;
use crate::type_lattice::{FitsFailure, KType, TypeNode, sig_fits};
use crate::values::{Value, satisfies};

use super::super::layout;
use super::super::view::{Ascription, Unascribable, ascribe};
use super::{member, module, schema};

/// `Ord` names one of `m`'s four members, and its head parameter `Carrier`, which *fits* solves
/// from what `m` offers for `zero`.
const PROGRAM: &str = "\
SIG Ord FOR ALL #[Carrier] = #[(VAL zero :Carrier)]
MODULE m = ((LET Carrier = Number) (LET zero = 0) (LET name = \"m\") (NEWTYPE Dist = Number))";

#[test]
fn a_transparent_view_carries_what_the_signature_names_and_drops_the_rest() {
    with_fixture(|fixture| {
        let lines = fixture.parse(PROGRAM);
        let (types, scratch) = (fixture.types, fixture.scratch());
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let activation = fixture.run(writer, &lines, &[]);
            let m = module(fixture, activation, "m");
            // `Ord` declares `zero :Carrier`; `m` binds `zero` to a number, so *fits* solves
            // `Carrier` to `Number`.
            let ord = declared(fixture, activation, "Ord");
            let view = ascribe(writer, m, ord, Ascription::Transparent, types, scratch)
                .unwrap_or_else(|error| panic!("`m` satisfies `Ord`: {error:?}"));

            let node = view.module().expect("a view is a module");
            assert_eq!(node.members().len(), 2, "one value slot, one type member");
            assert!(
                matches!(
                    member(fixture, view, "zero", types, scratch),
                    Value::Number(zero) if zero == 0.0
                ),
                "a transparent view carries the source's own word"
            );
            let Value::Type(carrier) = member(fixture, view, "Carrier", types, scratch) else {
                panic!("`Carrier` is a type member");
            };
            assert_eq!(carrier.handle(), KType::NUMBER);

            // A member the signature does not name is not carried: a view is a narrowing.
            let view_schema = schema(node.ktype(), types);
            assert_eq!(view_schema.value_slots.len(), 1);
            assert!(
                view_schema.parameters.is_empty(),
                "a module's signature has no parameters"
            );
            assert!(
                sig_fits(types, scratch, node.ktype(), ord).is_ok(),
                "the view still fits what it was ascribed"
            );
        })
    });
}

/// `m`, a twin of equal content built apart, and a module of other content, each fitting `Ord`.
const TWINS: &str = "\
SIG Ord FOR ALL #[Carrier] = #[(VAL zero :Carrier)]
MODULE m = (LET zero = 0)
MODULE twin = (LET zero = 0)
MODULE other = (LET zero = 1)";

#[test]
fn an_opaque_view_keys_its_carrier_on_content() {
    with_fixture(|fixture| {
        let lines = fixture.parse(TWINS);
        let (types, scratch) = (fixture.types, fixture.scratch());
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let activation = fixture.run(writer, &lines, &[]);
            let ord = declared(fixture, activation, "Ord");
            let opaque = |name| {
                let source = module(fixture, activation, name);
                ascribe(writer, source, ord, Ascription::Opaque, types, scratch)
                    .unwrap_or_else(|error| panic!("`{name}` satisfies `Ord`: {error:?}"))
            };
            let carrier = |view| match member(fixture, view, "Carrier", types, scratch) {
                Value::Type(carrier) => carrier.handle(),
                _ => panic!("`Carrier` is a type member"),
            };
            let (first, second) = (opaque("m"), opaque("m"));
            let key = carrier(first);
            assert!(
                matches!(
                    types.node(key),
                    TypeNode::Parameter {
                        carrier: Some(_),
                        ..
                    }
                ),
                "an opaque view's carrier is keyed on content"
            );
            assert_ne!(key, KType::NUMBER);
            assert_eq!(key, carrier(second), "two applications over one module");
            assert_eq!(
                first.module().expect("a module").ktype(),
                second.module().expect("a module").ktype(),
            );
            assert_eq!(key, carrier(opaque("twin")), "two modules of equal content");
            assert_ne!(
                key,
                carrier(opaque("other")),
                "two modules of other content"
            );

            // The value member is sealed at the carrier, so it no longer reads as a number.
            let zero = member(fixture, first, "zero", types, scratch);
            assert_eq!(zero.concrete_ktype(), key);
            assert!(
                matches!(zero, Value::Tagged(_)),
                "the member is behind the carrier, not a bare number"
            );
        })
    });
}

#[test]
fn a_carrier_carries_its_parameters_name_and_bound() {
    with_fixture(|fixture| {
        let lines = fixture.parse(PROGRAM);
        let (types, scratch) = (fixture.types, fixture.scratch());
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let activation = fixture.run(writer, &lines, &[]);
            let m = module(fixture, activation, "m");
            let ord = declared(fixture, activation, "Ord");
            let view = ascribe(writer, m, ord, Ascription::Opaque, types, scratch)
                .unwrap_or_else(|error| panic!("`m` satisfies `Ord`: {error:?}"));
            let Value::Type(carrier) = member(fixture, view, "Carrier", types, scratch) else {
                panic!("`Carrier` is a type member");
            };
            let TypeNode::Parameter {
                bound,
                carrier: key,
                name,
            } = types.node(carrier.handle())
            else {
                panic!("an opaque view's carrier is a parameter");
            };
            assert!(key.is_some(), "a carrier is keyed on content");
            assert_eq!(BinderSymbol::Type(name), fixture.name("Carrier"));
            assert_eq!(
                bound,
                KType::ANY,
                "the parameter declares no bound, so the carrier is bounded by Any"
            );
        })
    });
}

#[test]
fn a_view_refuses_what_it_cannot_be() {
    let source = "\
SIG Ord FOR ALL #[Carrier] = #[(VAL zero :Carrier)]
SIG Wider FOR ALL #[Carrier] = #[(VAL zero :Carrier) (VAL one :Carrier)]
MODULE m = (LET zero = 0)
LET f = (FN :{} -> Number = #(1))";
    with_fixture(|fixture| {
        let lines = fixture.parse(source);
        let (types, scratch) = (fixture.types, fixture.scratch());
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let activation = fixture.run(writer, &lines, &[]);
            let m = module(fixture, activation, "m");
            let ord = declared(fixture, activation, "Ord");
            let wider = declared(fixture, activation, "Wider");
            let f = crate::knot::tests::callable(fixture, activation, "f");

            assert!(matches!(
                ascribe(writer, f, ord, Ascription::Opaque, types, scratch),
                Err(Unascribable::NotAModule),
            ));
            assert!(matches!(
                ascribe(writer, m, KType::NUMBER, Ascription::Opaque, types, scratch),
                Err(Unascribable::NotASignature(_)),
            ));
            assert!(matches!(
                ascribe(writer, m, wider, Ascription::Opaque, types, scratch),
                Err(Unascribable::Unsatisfied(_)),
            ));
        })
    });
}

#[test]
fn a_view_copies_across_a_cell_like_any_module() {
    use crate::knot::KValueFamily;
    use crate::knot::tests::{Step, copy};
    use crate::memory::{CellGraph, ReleaseAbsorption};
    use crate::values::cross;

    with_fixture(|fixture| {
        let lines = fixture.parse(PROGRAM);
        let (types, scratch) = (fixture.types, fixture.scratch());
        let mut graph: CellGraph<'_, Step> = CellGraph::new(2, copy);
        let home = graph.create(None).unwrap();
        let dest = graph.create(None).unwrap();
        let (dormant, mint) = graph
            .enter(home, |context| {
                let writer = context.writer();
                let activation = fixture.run(writer, &lines, &[]);
                let m = module(fixture, activation, "m");
                let ord = declared(fixture, activation, "Ord");
                let view = ascribe(writer, m, ord, Ascription::Opaque, types, scratch)
                    .unwrap_or_else(|error| panic!("`m` satisfies `Ord`: {error:?}"));
                let Value::Type(carrier) = member(fixture, view, "Carrier", types, scratch) else {
                    panic!("`Carrier` is a type member");
                };
                let source = context.lift::<KValueFamily>(Value::Knotted(view));
                let crossed = cross(context, dest, &source, types).unwrap();
                (context.keep(crossed), carrier.handle())
            })
            .unwrap();
        graph.release(home, ReleaseAbsorption::IntoHolder).unwrap();
        graph
            .enter(dest, |context| {
                let carrier = context.redeem(dormant).unwrap();
                let Value::Knotted(view) = context.read(&carrier).value() else {
                    panic!("the kept view redeems as a module");
                };
                let node = view.module().expect("a view is a module");
                assert_eq!(node.members().len(), 2);
                // The mint is a lattice handle, not a region word, so it rides over verbatim and
                // the sealed member still reads at it.
                let Value::Tagged(zero) = node.members()[0] else {
                    panic!("the sealed member crosses as a tagged value");
                };
                assert_eq!(zero.ktype(), mint);
                assert!(matches!(zero.payload(), Value::Number(n) if *n == 0.0));
            })
            .unwrap();
        graph.release(dest, ReleaseAbsorption::IntoHolder).unwrap();
        assert!(graph.is_empty());
    });
}

/// A carrier records the bound its source met, and the view still fits the bounded signature; but
/// outside the view the bound reveals nothing, so what is sealed behind it lies under `Any` alone.
#[test]
fn a_bounded_member_records_its_bound_and_reveals_it_nowhere() {
    let source = "\
SIG Ord FOR ALL #{Carrier: Value} = #[(VAL zero :Carrier)]
SIG Counted FOR ALL #{Carrier: Number} = #[(VAL zero :Carrier)]
MODULE m = ((LET Carrier = Number) (LET zero = 0))
MODULE s = ((LET Carrier = Str) (LET zero = \"\"))";
    with_fixture(|fixture| {
        let lines = fixture.parse(source);
        let (types, scratch) = (fixture.types, fixture.scratch());
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let activation = fixture.run(writer, &lines, &[]);
            let (m, s) = (
                module(fixture, activation, "m"),
                module(fixture, activation, "s"),
            );
            for (name, bound) in [("Ord", KType::ANY_VALUE), ("Counted", KType::NUMBER)] {
                let sig = declared(fixture, activation, name);
                let view = ascribe(writer, m, sig, Ascription::Opaque, types, scratch)
                    .unwrap_or_else(|error| panic!("`m` satisfies `{name}`: {error:?}"));
                let Value::Type(carrier) = member(fixture, view, "Carrier", types, scratch) else {
                    panic!("`Carrier` is a type member");
                };
                assert!(matches!(
                    types.node(carrier.handle()),
                    TypeNode::Parameter { bound: recorded, .. } if recorded == bound
                ));
                let zero = member(fixture, view, "zero", types, scratch);
                assert!(!satisfies(bound, &zero, types, scratch), "{name}");
                assert!(
                    sig_fits(
                        types,
                        scratch,
                        view.module().expect("a module").ktype(),
                        sig
                    )
                    .is_ok(),
                    "an opaque view of a bounded signature still fits it"
                );
            }
            let counted = declared(fixture, activation, "Counted");
            assert!(matches!(
                ascribe(writer, s, counted, Ascription::Opaque, types, scratch),
                Err(Unascribable::Unsatisfied(FitsFailure::Unsolved { .. })),
            ));
        })
    });
}

/// A pinned parameter keeps its pin under `:|` too: only an unpinned one is minted.
#[test]
fn an_opaque_view_keeps_a_pin() {
    with_fixture(|fixture| {
        let lines = fixture.parse(PROGRAM);
        let (types, scratch) = (fixture.types, fixture.scratch());
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let activation = fixture.run(writer, &lines, &[]);
            let m = module(fixture, activation, "m");
            let ord = declared(fixture, activation, "Ord");
            let pinned =
                types.signature_apply(scratch, ord, &[(fixture.name("Carrier"), KType::NUMBER)]);
            let view = ascribe(writer, m, pinned, Ascription::Opaque, types, scratch)
                .unwrap_or_else(|error| {
                    panic!("`m` fits `Ord WITH {{Carrier = Number}}`: {error:?}")
                });
            let Value::Type(carrier) = member(fixture, view, "Carrier", types, scratch) else {
                panic!("`Carrier` is a type member");
            };
            assert_eq!(carrier.handle(), KType::NUMBER);
            assert!(matches!(
                member(fixture, view, "zero", types, scratch),
                Value::Number(zero) if zero == 0.0
            ));
        })
    });
}

/// `Boxes` declares one keyworded head; `two` offers two overloads at its key and one elsewhere.
const KEYWORDED: &str = "\
SIG Boxes = #[(EXPR #(BOX _ :Number) -> :(LIST OF Number))]
SIG Stack FOR ALL #[Elt] = #[(EXPR #(PUSH _ :Elt) -> :(LIST OF Elt))]
MODULE two = ((EXPR #(BOX x :Number) -> :(LIST OF Number) = #([x])) (EXPR #(BOX x :Str) -> :(LIST OF Str) = #([x])) (EXPR #(UNBOX x :Number) -> Number = #(x)))
MODULE one = (EXPR #(PUSH x :Number) -> :(LIST OF Number) = #([x]))";

#[test]
fn a_view_carries_each_overload_its_keyworded_members_admit() {
    with_fixture(|fixture| {
        let lines = fixture.parse(KEYWORDED);
        let (types, scratch) = (fixture.types, fixture.scratch());
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let activation = fixture.run(writer, &lines, &[]);
            let two = module(fixture, activation, "two");
            let boxes = declared(fixture, activation, "Boxes");
            let view = ascribe(writer, two, boxes, Ascription::Transparent, types, scratch)
                .unwrap_or_else(|error| panic!("`two` satisfies `Boxes`: {error:?}"));
            // `BOX _ :Str` is no overload the member admits, and `UNBOX` sits at another key.
            let carried = layout::registrations(view, types, scratch);
            assert_eq!(carried.len(), 1, "the one overload `Boxes` admits");
            assert!(
                carried[0]
                    .as_callable()
                    .and_then(|f| f.function())
                    .is_some(),
                "a member reading no parameter is carried as it is"
            );
            let node = view.module().expect("a view is a module");
            assert_eq!(schema(node.ktype(), types).keyworded.len(), 1);
            assert!(
                sig_fits(types, scratch, node.ktype(), boxes).is_ok(),
                "the view fits the signature it was ascribed to"
            );
        })
    });
}

#[test]
fn an_opaque_view_wraps_an_overload_over_a_carrier_in_a_barrier() {
    with_fixture(|fixture| {
        let lines = fixture.parse(KEYWORDED);
        let (types, scratch) = (fixture.types, fixture.scratch());
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let activation = fixture.run(writer, &lines, &[]);
            let one = module(fixture, activation, "one");
            let stack = declared(fixture, activation, "Stack");
            let view = ascribe(writer, one, stack, Ascription::Opaque, types, scratch)
                .unwrap_or_else(|error| panic!("`one` satisfies `Stack`: {error:?}"));
            let carried = layout::registrations(view, types, scratch);
            assert_eq!(carried.len(), 1);
            assert!(
                carried[0].as_callable().and_then(|f| f.coerced()).is_some(),
                "a head reading an unpinned parameter sits behind a barrier"
            );
            let node = view.module().expect("a view is a module");
            assert!(sig_fits(types, scratch, node.ktype(), stack).is_ok());
        })
    });
}
