//! Walks over a value as deep as a program may build one: a chain of newtypes over records, each
//! naming the next, far deeper than any recursion over it would fit on the default test thread.
//! Each walk runs over an explicit stack, so each test here runs on that thread.

use crate::memory::{CellGraph, ReleaseAbsorption, Writer};
use crate::symbols::{BinderSymbol, Symbol};
use crate::type_lattice::{KType, RecursiveGroupWindow, RelativeSchema};
use crate::values::{TypeValue, cross};

use super::{Fixture, Record, Step, Tagged, Value, copy, with_fixture};

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
            None,
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

/// How many levels `value` descends through `next` before it reaches `Null`.
fn levels(value: Value<'_>, next: Symbol) -> usize {
    let mut value = value;
    let mut levels = 0;
    while let Some(tagged) = value.as_tagged() {
        let record = tagged.payload().as_record().expect("a node holds a record");
        value = *record.field(next).expect("a node's record holds `next`");
        levels += 1;
    }
    assert!(matches!(value, Value::Null), "a chain ends in `Null`");
    levels
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
                let crossed = cross(context, dest, &source).unwrap();
                context.keep(crossed)
            })
            .unwrap();
        graph.release(home, ReleaseAbsorption::IntoHolder).unwrap();
        graph
            .enter(dest, |context| {
                let carrier = context.redeem(dormant).unwrap();
                let copied = context.read(&carrier).value();
                assert_eq!(levels(copied, fixture.next().symbol()), DEPTH);
            })
            .unwrap();
        graph.release(dest, ReleaseAbsorption::IntoHolder).unwrap();
        assert!(graph.is_empty());
    });
}
