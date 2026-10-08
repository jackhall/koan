//! Entering a `USING … SCOPE` block, and each member read by its name.

use crate::knot::KActivation;
use crate::knot::tests::{pin, with_fixture};
use crate::memory::{Writer, resident};
use crate::scope::{Activation, BodyShape, Coordinate, ShapeKind, Slot, Target};
use crate::type_lattice::KType;
use crate::values::Value;

use super::super::layout;
use super::super::surface::{Unsurfaceable, surface};
use super::module;

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

/// The block of `enclosing`'s shape, activated with every slot empty.
fn entered<'graph, 'cell>(
    writer: Writer<'cell>,
    enclosing: &'cell KActivation<'graph, 'cell>,
) -> &'cell KActivation<'graph, 'cell> {
    let block = only_block(enclosing.shape());
    resident(writer, Activation::of_block(writer, block, enclosing))
}

const PROGRAM: &str = "\
MODULE m = ((LET zero = 0) (LET name = \"m\") (NEWTYPE Dist = Number))
USING m SCOPE (zero name Dist)";

#[test]
fn a_surfaced_name_reads_the_member_it_names() {
    with_fixture(|fixture| {
        let lines = fixture.parse(PROGRAM);
        let (types, scratch) = (fixture.types, fixture.scratch());
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let activation = fixture.run(writer, &lines, &[]);
            let m = module(fixture, activation, "m");
            let block = entered(writer, activation);

            surface(writer, m, block, types, scratch)
                .expect("the block's parameters are `m`'s members");

            let read = |name: &str| {
                let name = fixture.name(name);
                let (slot, _) = block.shape().slot(name).expect("a surfaced parameter");
                block.read(local(slot))
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
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let activation = fixture.run(writer, &lines, &[]);
            let block = entered(writer, activation);

            assert_eq!(
                surface(
                    writer,
                    module(fixture, activation, "n"),
                    block,
                    types,
                    scratch
                ),
                Err(Unsurfaceable::Count {
                    expected: 3,
                    found: 2
                }),
            );
            assert_eq!(
                surface(
                    writer,
                    module(fixture, activation, "o"),
                    block,
                    types,
                    scratch
                ),
                Err(Unsurfaceable::Unnamed {
                    name: fixture.name("zero")
                }),
            );
            // A slot takes a bind exactly when it was empty.
            for slot in 0..block.shape().slots() {
                assert!(
                    block.bind(Slot(slot as u32), Value::Null).is_ok(),
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
LET f = (FN :{} -> Number = #(1))
USING m SCOPE (zero)";
    with_fixture(|fixture| {
        let lines = fixture.parse(source);
        let (types, scratch) = (fixture.types, fixture.scratch());
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let activation = fixture.run(writer, &lines, &[]);
            let f = crate::knot::tests::callable(fixture, activation, "f");
            let block = entered(writer, activation);
            assert_eq!(
                surface(writer, f, block, types, scratch),
                Err(Unsurfaceable::NotAModule)
            );
        })
    });
}

/// What a test program binds a member to.
#[derive(Clone, Copy, Debug)]
enum Bound {
    Number(f64),
    Str(&'static str),
    Type(KType),
}

/// Whether `value` is what `bound` names.
fn holds(value: Value<'_, impl crate::values::Knotted>, bound: Bound) -> bool {
    match (value, bound) {
        (Value::Number(held), Bound::Number(number)) => held == number,
        (Value::Type(held), Bound::Type(ktype)) => held.handle() == ktype,
        (value, Bound::Str(text)) => value.as_str() == Some(text),
        _ => false,
    }
}

/// The signature alone places a member: the module's member and the block's parameter of each
/// name hold the value the body bound under it, whatever order the body's or the block's slots
/// run in.
#[test]
fn every_member_is_read_by_its_name() {
    use Bound::{Number, Str, Type};
    let programs: [(&str, &[(&str, Bound)]); 4] = [
        (
            "MODULE m = ((LET zero = 0) (LET name = \"m\") (LET Dist = Number))\n\
             USING m SCOPE (zero name Dist)",
            &[
                ("zero", Number(0.0)),
                ("name", Str("m")),
                ("Dist", Type(KType::NUMBER)),
            ],
        ),
        (
            "MODULE m = ((LET aa = 1) (LET zz = 2) (LET Aa = Number) (LET Zz = Str))\n\
             USING m SCOPE (aa zz Aa Zz)",
            &[
                ("aa", Number(1.0)),
                ("zz", Number(2.0)),
                ("Aa", Type(KType::NUMBER)),
                ("Zz", Type(KType::STR)),
            ],
        ),
        (
            "MODULE m = ((LET only = 1))\nUSING m SCOPE (only)",
            &[("only", Number(1.0))],
        ),
        (
            "MODULE m = ((LET alpha = 1) (LET beta = 2) (LET gamma = 3) (LET delta = 4) \
             (LET Alpha = Number) (LET Beta = Str) (LET Gamma = Bool))\n\
             USING m SCOPE (alpha beta gamma delta Alpha Beta Gamma)",
            &[
                ("alpha", Number(1.0)),
                ("beta", Number(2.0)),
                ("gamma", Number(3.0)),
                ("delta", Number(4.0)),
                ("Alpha", Type(KType::NUMBER)),
                ("Beta", Type(KType::STR)),
                ("Gamma", Type(KType::BOOL)),
            ],
        ),
    ];
    let mut reordered = false;
    for (source, bound) in programs {
        with_fixture(|fixture| {
            let lines = fixture.parse(source);
            let (types, scratch) = (fixture.types, fixture.scratch());
            fixture.in_cell(pin, |context| {
                let writer = context.writer();
                let activation = fixture.run(writer, &lines, &[]);
                let m = module(fixture, activation, "m");
                let block = entered(writer, activation);
                surface(writer, m, block, types, scratch)
                    .expect("the block's parameters are `m`'s members");
                for (text, bound) in bound {
                    let name = fixture.name(text);
                    let member = layout::member(m, name, types, scratch).expect("a member");
                    assert!(holds(member, *bound), "`{source}`: `{text}` is {bound:?}");
                    let (slot, _) = block.shape().slot(name).expect("a surfaced parameter");
                    assert!(
                        holds(block.read(local(slot)), *bound),
                        "`{source}`: the block's `{text}` is {bound:?}"
                    );
                }
                // The names' symbol order is not their text order somewhere, so no reader can be
                // leaning on a sort by text.
                let symbols: Vec<_> = (bound.iter())
                    .map(|(text, _)| fixture.name(text).symbol())
                    .collect();
                reordered |= !symbols.is_sorted() && !symbols.is_sorted_by(|a, b| a >= b);
            })
        });
    }
    assert!(
        reordered,
        "some program's names intern out of their written order"
    );
}
