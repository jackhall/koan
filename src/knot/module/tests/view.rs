//! The view door: what `:|` builds over a module, and what refuses one; and what a retype at a
//! signature reads.

use std::ptr;

use crate::knot::tests::{TestGraph, declared, pin, with_fixture};
use crate::scope::Revealed;
use crate::symbols::BinderSymbol;
use crate::type_lattice::{
    DeclaredType, FitsFailure, KType, Parametric, SchemaDraft, SigOrigin, TypeNode, sig_fits,
};
use crate::values::{Value, satisfies};

use super::super::layout;
use super::super::view::{Unascribable, ascribe};
use super::{member, module, schema};

/// `Ord` names one of `m`'s four members, and its head parameter `Carrier`, which *fits* solves
/// from what `m` offers for `zero`.
const PROGRAM: &str = "\
SIG Ord FOR ALL #[Carrier] = #[(VAL zero :Carrier)]
MODULE m = ((LET Carrier = Number) (LET zero = 0) (LET name = \"m\") (NEWTYPE Dist = Number))";

#[test]
fn a_retype_at_a_signature_reads_what_the_signature_names_and_drops_the_rest() {
    with_fixture(|fixture| {
        let lines = fixture.parse(PROGRAM);
        let scratch = fixture.scratch();
        fixture.in_cell(pin, |context| {
            let types = &fixture.tiers.of(context);
            let writer = context.writer();
            let activation = fixture.run(writer, types, &lines, &[]);
            let m = module(fixture, activation, "m");
            // `Ord` declares `zero :Carrier`; `m` binds `zero` to a number, so *fits* solves
            // `Carrier` to `Number`, and `m` is re-stamped at `Ord` pinned there.
            let ord = declared(fixture, activation, "Ord");
            let retyped = Value::Knotted(m).retyped(writer, ord, types, scratch);
            let carrier: BinderSymbol = fixture.name("Carrier");
            let pinned = types.signature_apply(scratch, ord, &[(carrier, KType::NUMBER)]);
            assert!(
                matches!(retyped, Value::Retyped(_)),
                "a header over the bare node"
            );
            assert_eq!(retyped.ktype(), pinned.into());
            assert!(
                std::ptr::eq(
                    retyped
                        .as_module()
                        .expect("a module")
                        .module()
                        .expect("a node"),
                    m.module().expect("a node")
                ),
                "the header stands over `m`'s own node"
            );

            let read = |name: &str| {
                layout::member(writer, retyped, (fixture.name(name), None), types, scratch)
            };
            assert!(matches!(read("zero"), Some(Value::Number(zero)) if zero == 0.0));
            let Some(Value::Type(carrier)) = read("Carrier") else {
                panic!("`Carrier` is a type member");
            };
            assert_eq!(carrier.handle(), KType::NUMBER);
            // A member the signature does not name is not read: a retype is a narrowing.
            assert!(read("name").is_none(), "`Ord` names no `name`");
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
        let scratch = fixture.scratch();
        fixture.in_cell(pin, |context| {
            let types = &fixture.tiers.of(context);
            let writer = context.writer();
            let activation = fixture.run(writer, types, &lines, &[]);
            let ord = declared(fixture, activation, "Ord");
            let opaque = |name| {
                let source = module(fixture, activation, name);
                ascribe(writer, Value::Knotted(source), ord, None, types, scratch)
                    .unwrap_or_else(|error| panic!("`{name}` satisfies `Ord`: {error:?}"))
            };
            let carrier = |view| match member(fixture, view, "Carrier", types, scratch) {
                Value::Type(carrier) => carrier.handle(),
                _ => panic!("`Carrier` is a type member"),
            };
            let (first, second) = (opaque("m"), opaque("m"));
            let key = carrier(first);
            assert!(
                matches!(types.node(key), TypeNode::Carrier { .. }),
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

/// A view is keyed on its underlying and what its fit solves over it: through any header the
/// source is under, one view, since the header only admits the view.
#[test]
fn an_opaque_views_carrier_is_keyed_on_its_own_content() {
    with_fixture(|fixture| {
        let lines = fixture.parse(PROGRAM);
        let scratch = fixture.scratch();
        fixture.in_cell(pin, |context| {
            let types = &fixture.tiers.of(context);
            let writer = context.writer();
            let activation = fixture.run(writer, types, &lines, &[]);
            let m = module(fixture, activation, "m");
            let ord = declared(fixture, activation, "Ord");
            let carrier: BinderSymbol = fixture.name("Carrier");
            let wide = types.union_of(scratch, &[KType::NUMBER, KType::STR]);
            let wide = types.signature_apply(scratch, ord, &[(carrier, wide)]);
            let view = |source| {
                ascribe(writer, source, ord, None, types, scratch)
                    .unwrap_or_else(|error| panic!("`m` satisfies `Ord`: {error:?}"))
            };
            let content = |view: crate::knot::Knotted<'_, '_>| {
                (view.module().expect("a view is a module").content())
                    .expect("a view's content is known")
            };
            let bare = view(Value::Knotted(m));
            let Value::Type(key) = member(fixture, bare, "Carrier", types, scratch) else {
                panic!("`Carrier` is a type member");
            };
            assert!(matches!(
                types.node(key.handle()),
                TypeNode::Carrier { key, .. } if key == content(bare).carrier_key()
            ));
            let through = view(Value::Knotted(m).retyped(writer, ord, types, scratch));
            assert_eq!(content(bare), content(through), "a header that fits alike");
            let widened = view(Value::Knotted(m).retyped(writer, wide, types, scratch));
            assert_eq!(
                content(bare),
                content(widened),
                "a header pinning the parameter otherwise"
            );
        })
    });
}

#[test]
fn a_carrier_carries_its_parameters_name_and_bound() {
    with_fixture(|fixture| {
        let lines = fixture.parse(PROGRAM);
        let scratch = fixture.scratch();
        fixture.in_cell(pin, |context| {
            let types = &fixture.tiers.of(context);
            let writer = context.writer();
            let activation = fixture.run(writer, types, &lines, &[]);
            let m = module(fixture, activation, "m");
            let ord = declared(fixture, activation, "Ord");
            let view = ascribe(writer, Value::Knotted(m), ord, None, types, scratch)
                .unwrap_or_else(|error| panic!("`m` satisfies `Ord`: {error:?}"));
            let Value::Type(carrier) = member(fixture, view, "Carrier", types, scratch) else {
                panic!("`Carrier` is a type member");
            };
            let TypeNode::Carrier { name, met, .. } = types.node(carrier.handle()) else {
                panic!("an opaque view's carrier is a carrier node");
            };
            assert_eq!(BinderSymbol::Type(name), fixture.name("Carrier"));
            assert_eq!(
                met,
                KType::ANY,
                "the parameter declares no bound, so the carrier meets Any"
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
        let scratch = fixture.scratch();
        fixture.in_cell(pin, |context| {
            let types = &fixture.tiers.of(context);
            let writer = context.writer();
            let activation = fixture.run(writer, types, &lines, &[]);
            let m = module(fixture, activation, "m");
            let ord = declared(fixture, activation, "Ord");
            let wider = declared(fixture, activation, "Wider");
            let f = crate::knot::tests::callable(fixture, activation, "f");

            assert!(matches!(
                ascribe(writer, Value::Knotted(f), ord, None, types, scratch),
                Err(Unascribable::NotAModule),
            ));
            assert!(matches!(
                ascribe(
                    writer,
                    Value::Knotted(m),
                    KType::NUMBER,
                    None,
                    types,
                    scratch
                ),
                Err(Unascribable::NotASignature(_)),
            ));
            assert!(matches!(
                ascribe(writer, Value::Knotted(m), wider, None, types, scratch),
                Err(Unascribable::Unsatisfied(_)),
            ));
        })
    });
}

#[test]
fn a_view_copies_across_a_cell_like_any_module() {
    use crate::knot::KValueFamily;
    use crate::knot::tests::copy;
    use crate::memory::{CellGraph, ReleaseAbsorption};

    with_fixture(|fixture| {
        let lines = fixture.parse(PROGRAM);
        let scratch = fixture.scratch();
        let mut graph: TestGraph<'_> = CellGraph::new(2, copy);
        let home = graph.create(None).unwrap();
        let dest = graph.create(None).unwrap();
        let (dormant, mint) = graph
            .enter(home, |context| {
                let types = &fixture.tiers.of(context);
                let writer = context.writer();
                let activation = fixture.run(writer, types, &lines, &[]);
                let m = module(fixture, activation, "m");
                let ord = declared(fixture, activation, "Ord");
                let view = ascribe(writer, Value::Knotted(m), ord, None, types, scratch)
                    .unwrap_or_else(|error| panic!("`m` satisfies `Ord`: {error:?}"));
                let Value::Type(carrier) = member(fixture, view, "Carrier", types, scratch) else {
                    panic!("`Carrier` is a type member");
                };
                let source = context.lift::<KValueFamily>(Value::Knotted(view));
                let crossed = fixture.tiers.cross(context, dest, &source).unwrap();
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
        let scratch = fixture.scratch();
        fixture.in_cell(pin, |context| {
            let types = &fixture.tiers.of(context);
            let writer = context.writer();
            let activation = fixture.run(writer, types, &lines, &[]);
            let (m, s) = (
                module(fixture, activation, "m"),
                module(fixture, activation, "s"),
            );
            for (name, bound) in [("Ord", KType::ANY_VALUE), ("Counted", KType::NUMBER)] {
                let sig = declared(fixture, activation, name);
                let view = ascribe(writer, Value::Knotted(m), sig, None, types, scratch)
                    .unwrap_or_else(|error| panic!("`m` satisfies `{name}`: {error:?}"));
                let Value::Type(carrier) = member(fixture, view, "Carrier", types, scratch) else {
                    panic!("`Carrier` is a type member");
                };
                assert!(matches!(
                    types.node(carrier.handle()),
                    TypeNode::Carrier { met: recorded, .. } if recorded == bound
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
                ascribe(writer, Value::Knotted(s), counted, None, types, scratch),
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
        let scratch = fixture.scratch();
        fixture.in_cell(pin, |context| {
            let types = &fixture.tiers.of(context);
            let writer = context.writer();
            let activation = fixture.run(writer, types, &lines, &[]);
            let m = module(fixture, activation, "m");
            let ord = declared(fixture, activation, "Ord");
            let pinned =
                types.signature_apply(scratch, ord, &[(fixture.name("Carrier"), KType::NUMBER)]);
            let view = ascribe(writer, Value::Knotted(m), pinned, None, types, scratch)
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

/// `Boxes` declares one keyworded head; `two` offers three overloads at its key, two covering it,
/// and one elsewhere.
const KEYWORDED: &str = "\
SIG Boxes = #[(EXPR #(BOX _ :Number) -> :(LIST OF Number))]
SIG Stack FOR ALL #[Elt] = #[(EXPR #(PUSH _ :Elt) -> :(LIST OF Elt))]
MODULE two = ((EXPR #(BOX x :Number) -> :(LIST OF Number) = #([x])) (EXPR #(BOX x :Any) -> :(LIST OF Number) = #([0])) (EXPR #(BOX x :Str) -> :(LIST OF Str) = #([x])) (EXPR #(UNBOX x :Number) -> Number = #(x)))
MODULE one = (EXPR #(PUSH x :Number) -> :(LIST OF Number) = #([x]))";

#[test]
fn a_view_carries_the_overload_each_keyworded_member_picks() {
    with_fixture(|fixture| {
        let lines = fixture.parse(KEYWORDED);
        let scratch = fixture.scratch();
        fixture.in_cell(pin, |context| {
            let types = &fixture.tiers.of(context);
            let writer = context.writer();
            let activation = fixture.run(writer, types, &lines, &[]);
            let two = module(fixture, activation, "two");
            let boxes = declared(fixture, activation, "Boxes");
            let view = ascribe(writer, Value::Knotted(two), boxes, None, types, scratch)
                .unwrap_or_else(|error| panic!("`two` satisfies `Boxes`: {error:?}"));
            // `BOX _ :Str` covers no member, `BOX _ :Any` is outranked by `BOX _ :Number`, and
            // `UNBOX` sits at another key.
            let carried = layout::registrations(Value::Knotted(view), types, scratch);
            assert_eq!(carried.len(), 1, "the one overload `Boxes` picks");
            let shape = layout::registered_shape(carried[0]).expect("a function");
            assert!(
                matches!(carried[0], Value::Retyped(_))
                    && DeclaredType::<Parametric>::from(shape) == schema(boxes, types).keyworded[0],
                "the pick is retyped to the member's shape"
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
        let scratch = fixture.scratch();
        fixture.in_cell(pin, |context| {
            let types = &fixture.tiers.of(context);
            let writer = context.writer();
            let activation = fixture.run(writer, types, &lines, &[]);
            let one = module(fixture, activation, "one");
            let stack = declared(fixture, activation, "Stack");
            let view = ascribe(writer, Value::Knotted(one), stack, None, types, scratch)
                .unwrap_or_else(|error| panic!("`one` satisfies `Stack`: {error:?}"));
            let carried = layout::registrations(Value::Knotted(view), types, scratch);
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

/// `Counter`, `Stepper` naming its function member alone, `Shown` showing `zero` at `Any`, and a
/// module fitting all three: the views of a view below take them over `ints :| Counter`.
const COUNTER: &str = "\
SIG Counter FOR ALL #[Carrier] = #[(VAL zero :Carrier) (VAL succ :(FN :{x :Carrier} -> Carrier))]
SIG Stepper FOR ALL #[Carrier] = #[(VAL succ :(FN :{x :Carrier} -> Carrier))]
SIG Shown = #[(VAL zero :Any)]
MODULE ints = ((LET zero = 4) (LET succ = (FN :{x :Number} -> Number = #(x))))";

#[test]
fn a_view_of_a_view_at_its_exposure_is_its_source() {
    with_fixture(|fixture| {
        let lines = fixture.parse(COUNTER);
        let scratch = fixture.scratch();
        fixture.in_cell(pin, |context| {
            let types = &fixture.tiers.of(context);
            let writer = context.writer();
            let activation = fixture.run(writer, types, &lines, &[]);
            let ints = module(fixture, activation, "ints");
            let counter = declared(fixture, activation, "Counter");
            let view = |source| {
                ascribe(writer, source, counter, None, types, scratch)
                    .unwrap_or_else(|error| panic!("the source satisfies `Counter`: {error:?}"))
            };
            let c = view(Value::Knotted(ints));
            assert!(ptr::eq(view(Value::Knotted(c)).node(), c.node()));
            // Through a header widening the parameter, which only admits the view.
            let carrier: BinderSymbol = fixture.name("Carrier");
            let wide = types.signature_apply(scratch, counter, &[(carrier, KType::ANY)]);
            let headed = Value::Knotted(c).retyped(writer, wide, types, scratch);
            assert!(ptr::eq(view(headed).node(), c.node()));
        })
    });
}

#[test]
fn a_view_of_a_view_is_built_over_its_underlying_behind_one_barrier() {
    with_fixture(|fixture| {
        let lines = fixture.parse(COUNTER);
        let scratch = fixture.scratch();
        fixture.in_cell(pin, |context| {
            let types = &fixture.tiers.of(context);
            let writer = context.writer();
            let activation = fixture.run(writer, types, &lines, &[]);
            let ints = module(fixture, activation, "ints");
            let (counter, stepper) = (
                declared(fixture, activation, "Counter"),
                declared(fixture, activation, "Stepper"),
            );
            let c = ascribe(writer, Value::Knotted(ints), counter, None, types, scratch)
                .unwrap_or_else(|error| panic!("`ints` satisfies `Counter`: {error:?}"));
            let s = ascribe(writer, Value::Knotted(c), stepper, None, types, scratch)
                .unwrap_or_else(|error| panic!("`c` satisfies `Stepper`: {error:?}"));
            let node = s.module().expect("a view is a module");
            assert!(
                node.underlying()
                    .is_some_and(|underlying| ptr::eq(underlying.node(), ints.node())),
                "the view keeps the module under every view"
            );
            let Value::Knotted(succ) = member(fixture, s, "succ", types, scratch) else {
                panic!("`succ` is a function member");
            };
            let Value::Knotted(own) = member(fixture, ints, "succ", types, scratch) else {
                panic!("`succ` is a function member");
            };
            assert_eq!(succ.barriers().count(), 1, "one barrier");
            assert!(ptr::eq(succ.behind_barriers().node(), own.node()));
        })
    });
}

#[test]
fn a_view_of_a_view_that_shows_what_its_source_hides_is_refused() {
    with_fixture(|fixture| {
        let lines = fixture.parse(COUNTER);
        let scratch = fixture.scratch();
        fixture.in_cell(pin, |context| {
            let types = &fixture.tiers.of(context);
            let writer = context.writer();
            let activation = fixture.run(writer, types, &lines, &[]);
            let ints = module(fixture, activation, "ints");
            let (counter, shown) = (
                declared(fixture, activation, "Counter"),
                declared(fixture, activation, "Shown"),
            );
            let c = ascribe(writer, Value::Knotted(ints), counter, None, types, scratch)
                .unwrap_or_else(|error| panic!("`ints` satisfies `Counter`: {error:?}"));
            let BinderSymbol::Value(zero) = fixture.name("zero") else {
                panic!("`zero` is a value name");
            };
            assert!(matches!(
                ascribe(writer, Value::Knotted(c), shown, None, types, scratch),
                Err(Unascribable::Reveals { member: Revealed::Slot(member) }) if member == zero
            ));
            assert!(ascribe(writer, Value::Knotted(ints), shown, None, types, scratch).is_ok());
        })
    });
}

#[test]
fn a_view_of_a_view_naming_a_sources_carrier_inside_a_signature_is_read_back() {
    // A signature declaring a slot at the source's own carrier is read back onto the module the
    // view is built over, inside the signature too: the view is built, its member at that carrier.
    with_fixture(|fixture| {
        let lines = fixture.parse(COUNTER);
        let scratch = fixture.scratch();
        fixture.in_cell(pin, |context| {
            let types = &fixture.tiers.of(context);
            let writer = context.writer();
            let activation = fixture.run(writer, types, &lines, &[]);
            let ints = module(fixture, activation, "ints");
            let counter = declared(fixture, activation, "Counter");
            let c = ascribe(writer, Value::Knotted(ints), counter, None, types, scratch)
                .unwrap_or_else(|error| panic!("`ints` satisfies `Counter`: {error:?}"));
            let Value::Type(carrier) = member(fixture, c, "Carrier", types, scratch) else {
                panic!("`Carrier` is a type member");
            };
            let BinderSymbol::Value(zero) = fixture.name("zero") else {
                panic!("`zero` is a value name");
            };
            let mut draft = SchemaDraft::new(scratch);
            draft.origin = SigOrigin::Declared;
            draft.insert_value_slot(zero, carrier.handle());
            let named = types.signature(scratch, draft);
            let d = ascribe(writer, Value::Knotted(c), named, None, types, scratch)
                .unwrap_or_else(|error| panic!("`c` satisfies its carrier's signature: {error:?}"));
            let zero = member(fixture, d, "zero", types, scratch);
            assert_eq!(zero.concrete_ktype(), carrier.handle());
        })
    });
}

const NESTED_COUNTER: &str = "\
SIG Counter FOR ALL #[Carrier] = #[(VAL zero :Carrier)]
SIG Outer FOR ALL #[Carrier] = #[(VAL zero :Carrier) (VAL m :(Counter WITH {Carrier = Carrier}))]
SIG Wrap FOR ALL #[Elem] = #[(VAL m :Elem)]
MODULE ints = ((LET zero = 0) (MODULE m = ((LET zero = 0))))";

#[test]
fn a_view_of_a_view_pinning_a_signature_at_a_sources_carrier_is_read_back() {
    // A pin holding a signature that names the source's carrier hides the nested member again
    // behind that carrier, and is read back onto the module the view is built over.
    with_fixture(|fixture| {
        let lines = fixture.parse(NESTED_COUNTER);
        let scratch = fixture.scratch();
        fixture.in_cell(pin, |context| {
            let types = &fixture.tiers.of(context);
            let writer = context.writer();
            let activation = fixture.run(writer, types, &lines, &[]);
            let ints = module(fixture, activation, "ints");
            let (outer, wrap) = (
                declared(fixture, activation, "Outer"),
                declared(fixture, activation, "Wrap"),
            );
            let c = ascribe(writer, Value::Knotted(ints), outer, None, types, scratch)
                .unwrap_or_else(|error| panic!("`ints` satisfies `Outer`: {error:?}"));
            let Value::Type(carrier) = member(fixture, c, "Carrier", types, scratch) else {
                panic!("`Carrier` is a type member");
            };
            let BinderSymbol::Value(zero) = fixture.name("zero") else {
                panic!("`zero` is a value name");
            };
            let mut draft = SchemaDraft::new(scratch);
            draft.origin = SigOrigin::Declared;
            draft.insert_value_slot(zero, carrier.handle());
            let named = types.signature(scratch, draft);
            let wrapped = types.signature_apply(scratch, wrap, &[(fixture.name("Elem"), named)]);
            let d = ascribe(writer, Value::Knotted(c), wrapped, None, types, scratch)
                .unwrap_or_else(|error| panic!("`c` satisfies the wrapped signature: {error:?}"));
            let Value::Knotted(m) = member(fixture, d, "m", types, scratch) else {
                panic!("`m` is a module member");
            };
            let zero = member(fixture, m, "zero", types, scratch);
            assert_eq!(zero.concrete_ktype(), carrier.handle());
        })
    });
}
