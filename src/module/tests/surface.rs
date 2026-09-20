//! Entering a `USING … SCOPE` block, and the layout law the whole item rests on.

use crate::function::tests::{pin, with_fixture};
use crate::memory::{CellHandle, Writer, resident};
use crate::parse::BinderSymbol;
use crate::scope::{Activation, Binding, BodyShape, Coordinate, ShapeKind, Slot, Target};
use crate::values::Value;

use super::super::layout;
use super::super::surface::{Unsurfaceable, surface};
use super::{module, schema};

/// The one block shape in `shape`'s tree — the body a `USING` surfaces into.
fn only_block<'graph>(shape: &BodyShape<'graph>) -> &'graph BodyShape<'graph> {
    fn collect<'graph>(shape: &BodyShape<'graph>, found: &mut Vec<&'graph BodyShape<'graph>>) {
        for (_, child) in shape.nested_shapes() {
            if child.kind() == ShapeKind::Block {
                found.push(child);
            }
            collect(child, found);
        }
    }
    let mut found = Vec::new();
    collect(shape, &mut found);
    assert_eq!(found.len(), 1, "the program holds one block");
    found[0]
}

/// This activation's own `slot`.
fn local(slot: Slot) -> Coordinate {
    Coordinate::Activation {
        hops: 0,
        target: Target::Local(slot),
    }
}

/// The block of `enclosing`'s shape, activated with every slot claimed.
fn entered<'graph, 'cell, X: crate::values::Knotted>(
    writer: Writer<'cell>,
    enclosing: &'cell Activation<'graph, 'cell, X>,
    binder: CellHandle,
) -> &'cell Activation<'graph, 'cell, X> {
    let block = only_block(enclosing.shape());
    let activation = resident(writer, Activation::of_block(writer, block, enclosing));
    for slot in 0..block.slots() {
        activation
            .claim(Slot(slot as u32), binder)
            .expect("a fresh slot claims");
    }
    activation
}

const PROGRAM: &str = "\
MODULE m = ((LET zero = 0) (LET name = \"m\") (NEWTYPE Dist = Number))
USING m SCOPE (zero name Dist)";

#[test]
fn a_surfaced_name_reads_the_member_it_names() {
    with_fixture(|fixture| {
        let lines = fixture.parse(PROGRAM);
        let (types, scratch) = (fixture.types, fixture.scratch());
        fixture.in_cell(pin, |context, binder| {
            let writer = context.writer();
            let activation = fixture.run(writer, &lines, binder, &[]);
            let m = module(fixture, activation, "m");
            let block = entered(writer, activation, binder);

            surface(m, block, types, scratch).expect("the block's parameters are `m`'s members");

            let read = |name: &str| {
                let name = fixture.name(name);
                let (slot, _) = block.shape().slot(name).expect("a surfaced parameter");
                match block.read(local(slot)) {
                    Binding::Bound(value) => value,
                    Binding::Pending(_) => panic!("`{name:?}` is bound by surfacing"),
                }
            };
            assert!(matches!(read("zero"), Value::Number(zero) if zero == 0.0));
            assert_eq!(read("name").as_str(), Some("m"));
            let Value::Type(dist) = read("Dist") else {
                panic!("`Dist` surfaces as a type value");
            };
            assert_ne!(dist.handle(), crate::type_lattice::KType::NUMBER);
        })
    });
}

#[test]
fn a_module_that_is_not_the_blocks_own_is_refused_and_binds_nothing() {
    // The block's parameters are read off `m`'s declaration where the `USING` is shaped, so a
    // block and the module it surfaces cannot disagree: these guard a caller that hands the door
    // some *other* module. `n` holds `m`'s two names and a type of its own, so every parameter
    // still lands on the member it names and only the count catches it; `o` holds neither name.
    let source = "\
MODULE m = ((LET zero = 0) (LET name = \"m\"))
MODULE n = ((LET zero = 0) (LET name = \"n\") (NEWTYPE Dist = Number))
MODULE o = ((LET other = 0) (LET third = 1))
USING m SCOPE (zero name)";
    with_fixture(|fixture| {
        let lines = fixture.parse(source);
        let (types, scratch) = (fixture.types, fixture.scratch());
        fixture.in_cell(pin, |context, binder| {
            let writer = context.writer();
            let activation = fixture.run(writer, &lines, binder, &[]);
            let block = entered(writer, activation, binder);

            assert_eq!(
                surface(module(fixture, activation, "n"), block, types, scratch),
                Err(Unsurfaceable::Count {
                    expected: 3,
                    found: 2
                }),
            );
            assert_eq!(
                surface(module(fixture, activation, "o"), block, types, scratch),
                Err(Unsurfaceable::Unnamed {
                    name: fixture.name("zero")
                }),
            );
            for slot in 0..block.shape().slots() {
                assert!(
                    matches!(block.read(local(Slot(slot as u32))), Binding::Pending(_)),
                    "a refusal binds nothing"
                );
            }
        })
    });
}

#[test]
fn a_value_that_is_no_module_cannot_be_surfaced() {
    let source = "\
MODULE m = (LET zero = 0)
LET f = (FN :{} -> Number = (1))
USING m SCOPE (zero)";
    with_fixture(|fixture| {
        let lines = fixture.parse(source);
        let (types, scratch) = (fixture.types, fixture.scratch());
        fixture.in_cell(pin, |context, binder| {
            let writer = context.writer();
            let activation = fixture.run(writer, &lines, binder, &[]);
            let f = crate::function::tests::callable(fixture, activation, "f");
            let block = entered(writer, activation, binder);
            assert_eq!(
                surface(f, block, types, scratch),
                Err(Unsurfaceable::NotAModule)
            );
        })
    });
}

/// The law the design rests on: three readers agree on where a member sits, per channel, and none
/// of them consults the others.
#[test]
fn the_block_the_body_and_the_signature_agree_on_every_members_place() {
    let programs = [
        PROGRAM,
        "MODULE m = ((LET aa = 1) (LET zz = 2) (NEWTYPE Aa = Number) (NEWTYPE Zz = Str))\n\
         USING m SCOPE (aa zz Aa Zz)",
        "MODULE m = ((LET only = 1))\nUSING m SCOPE (only)",
    ];
    for source in programs {
        with_fixture(|fixture| {
            let lines = fixture.parse(source);
            let (types, scratch) = (fixture.types, fixture.scratch());
            fixture.in_cell(pin, |context, binder| {
                let writer = context.writer();
                let activation = fixture.run(writer, &lines, binder, &[]);
                let m = module(fixture, activation, "m");
                let sig = schema(m.module().expect("a module").ktype(), types);
                let body = activation
                    .shape()
                    .births(
                        activation
                            .shape()
                            .slot(fixture.name("m"))
                            .expect("`m` is declared")
                            .0,
                    )
                    .expect("`m` births its body");
                let block = only_block(activation.shape());

                assert_eq!(body.slots(), layout::member_count(&sig, scratch));
                assert_eq!(block.slots(), layout::member_count(&sig, scratch));
                for index in 0..body.slots() {
                    let name = body.slot_name(Slot(index as u32));
                    assert_eq!(
                        layout::member_index(&sig, scratch, name),
                        Some(index),
                        "`{source}`: the signature places `{name:?}` where the body does",
                    );
                    let (block_slot, _) = block.slot(name).expect("the block surfaces the name");
                    assert_eq!(
                        block_slot.index(),
                        index,
                        "`{source}`: the block places `{name:?}` where the body does",
                    );
                }
                // And the order really is by interned symbol, not by the text of the name: the
                // value channel comes first and each channel is sorted by symbol.
                assert!(channel_sorted(body));
            })
        });
    }
}

/// Whether `body`'s slots run value-channel-first with each channel symbol-sorted.
fn channel_sorted(body: &BodyShape<'_>) -> bool {
    let names: Vec<BinderSymbol> = (0..body.slots())
        .map(|slot| body.slot_name(Slot(slot as u32)))
        .collect();
    let split = names
        .iter()
        .position(|name| matches!(name, BinderSymbol::Type(_)))
        .unwrap_or(names.len());
    let (values, types) = names.split_at(split);
    values
        .iter()
        .all(|name| matches!(name, BinderSymbol::Value(_)))
        && values.is_sorted_by_key(|name| name.symbol())
        && types.is_sorted_by_key(|name| name.symbol())
}
