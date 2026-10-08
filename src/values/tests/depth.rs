//! Walks over a value as deep as a program may build one: a chain of newtypes over records, each
//! naming the next, and a chain of knots, each holding a member of the next, far deeper than any
//! recursion over it would fit on the default test thread. Each walk runs over an explicit stack,
//! so each test here runs on that thread.

use crate::memory::{Bump, CellGraph, ReleaseAbsorption, Writer};
use crate::symbols::BinderSymbol;
use crate::type_lattice::{KType, RecursiveGroupWindow, RelativeSchema};
use crate::values::{TypeValue, cross};

use super::{Fixture, Holding, Link, Record, Step, Tagged, Value, copy, pin, tie, with_fixture};

/// How deep each chain is: about a hundred times what a recursive walk reaches in a debug build.
const DEPTH: usize = 100_000;

impl Fixture<'_, '_> {
    /// `NEWTYPE Node = :{next :(Node | Null)}`, sealed as a singleton recursive group.
    fn chain_type(&self) -> KType {
        let (types, scratch) = (self.types, self.scratch());
        let next = self.next();
        let link = types.union_of(scratch, &[types.sibling(0), KType::NULL]);
        let representation = types.record(scratch, &[(next, link)]);
        RecursiveGroupWindow::seal_singleton(
            scratch,
            crate::symbols::TypeSymbol::declared("Node", self.symbols).unwrap(),
            RelativeSchema::NewType(representation),
            types,
            scratch,
        )
    }

    fn next(&self) -> BinderSymbol {
        BinderSymbol::declared("next", self.symbols).unwrap()
    }

    /// A chain `depth` deep in `writer`'s region, `Null` at the bottom.
    fn chain<'cell>(&self, writer: Writer<'cell>, node: KType, depth: usize) -> Value<'cell> {
        let (types, scratch) = (self.types, self.scratch());
        let head = TypeValue::new(writer, node, types);
        let next = self.next();
        (0..depth).fold(Value::Null, |below, _| {
            let record = Record::new(writer, &[(next, below)], types, scratch);
            let tagged = Tagged::construct(writer, head, Value::Record(record), types, scratch);
            Value::Tagged(tagged.expect("a record over `Node | Null` constructs a `Node`"))
        })
    }
}

impl Fixture<'_, '_> {
    /// A chain of one-node knots `depth` deep in `writer`'s region: each a record node under
    /// `{next :Any}` whose `next` is a value link to the member of the knot below, `Null` at the
    /// bottom.
    fn knots<'cell>(&self, writer: Writer<'cell>, depth: usize) -> Holding<'cell> {
        let (types, scratch) = (self.types, self.scratch());
        let next = self.next();
        let memo = types.record(scratch, &[(next, KType::ANY)]);
        (0..depth).fold(Holding::Null, |below, _| {
            let node = tie(writer, 1, |_, _| {
                let fields = [(next, Link::Value(below))];
                crate::values::Circular::Record(crate::values::Record::linked(
                    writer, &fields, memo, scratch,
                ))
            })[0];
            Holding::Knotted(node)
        })
    }
}

/// `value` as `PRINT` writes it.
fn rendered(fixture: &Fixture<'_, '_>, value: Value<'_>) -> String {
    let mut out = String::new();
    value
        .render(&mut out, fixture.types, fixture.symbols, fixture.scratch())
        .unwrap();
    out
}

/// How a chain `depth` deep renders.
fn chain_text(depth: usize) -> String {
    format!(
        "{}null{}",
        "Node({next = ".repeat(depth),
        "})".repeat(depth)
    )
}

#[test]
fn a_chain_deeper_than_the_stack_crosses() {
    with_fixture(|fixture| {
        let node = fixture.chain_type();
        let mut graph: CellGraph<'_, Step> = CellGraph::new(2, copy);
        let home = graph.create(None).unwrap();
        let dest = graph.create(None).unwrap();
        let dormant = graph
            .enter(home, |context| {
                let chain = fixture.chain(context.writer(), node, DEPTH);
                let source = context.lift::<crate::values::ValueFamily>(chain);
                let crossed = cross(context, dest, &source, fixture.types).unwrap();
                context.keep(crossed)
            })
            .unwrap();
        graph.release(home, ReleaseAbsorption::IntoHolder).unwrap();
        graph
            .enter(dest, |context| {
                let carrier = context.redeem(dormant).unwrap();
                let copied = context.read(&carrier).value();
                assert_eq!(rendered(fixture, copied), chain_text(DEPTH));
            })
            .unwrap();
        graph.release(dest, ReleaseAbsorption::IntoHolder).unwrap();
        assert!(graph.is_empty());
    });
}

#[test]
fn a_chain_deeper_than_the_stack_compares() {
    with_fixture(|fixture| {
        let node = fixture.chain_type();
        let (types, scratch) = (fixture.types, fixture.scratch());
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let chain = fixture.chain(writer, node, DEPTH);
            let same = fixture.chain(writer, node, DEPTH);
            let shorter = fixture.chain(writer, node, DEPTH - 1);
            assert_eq!(chain.equals(&same, types, scratch), Ok(true));
            assert_eq!(chain.equals(&shorter, types, scratch), Ok(false));
        });
    });
}

#[test]
fn a_chain_deeper_than_the_stack_renders() {
    with_fixture(|fixture| {
        let node = fixture.chain_type();
        fixture.in_cell(pin, |context| {
            let chain = fixture.chain(context.writer(), node, DEPTH);
            assert_eq!(rendered(fixture, chain), chain_text(DEPTH));
        });
    });
}

#[test]
fn a_chain_deeper_than_the_stack_digests() {
    with_fixture(|fixture| {
        let node = fixture.chain_type();
        // A demand stages a frame per level, so each digest gets a scratch of its own, released
        // after it.
        let digest = |value: Value<'_>| value.digest(fixture.types, &Bump::new());
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let chain = fixture.chain(writer, node, DEPTH);
            let same = fixture.chain(writer, node, DEPTH);
            let shorter = fixture.chain(writer, node, DEPTH - 1);
            assert_eq!(digest(chain), digest(same));
            assert_ne!(digest(chain), digest(shorter));
        });
    });
}

/// A knot member's digest is its knot's, and a knot holding a member of another as a value is a
/// frame of the same walk, so a chain of knots digests on one explicit stack.
#[test]
fn a_chain_of_knots_deeper_than_the_stack_digests() {
    with_fixture(|fixture| {
        let digest = |value: Holding<'_>| value.digest(fixture.types, &Bump::new());
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let chain = fixture.knots(writer, DEPTH);
            assert_eq!(digest(chain), digest(fixture.knots(writer, DEPTH)));
            assert_ne!(digest(chain), digest(fixture.knots(writer, DEPTH - 1)));
        });
    });
}
