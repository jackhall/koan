//! Shared scaffolding for `elaborate`'s suites: a program shaped over a builtin table of type
//! values, activated in a cell with each slot bound — or claimed — as the test asks.

mod boundary;
mod examples;

use crate::memory::{
    Bump, BumpAllocator, CellGraph, CellHandle, ProgramBrand, ReleaseAbsorption, SlabHandle,
    Verdict, Writer, program_storage, reattachable, resident,
};
use crate::parse::{BinderSymbol, KExpression, LabelInterner, TypeSymbol, parse};
use crate::scope::{Activation, Builtins, Shape, Slot};
use crate::type_lattice::{KType, TypeRegistry};
use crate::values::{TypeValue, Value};

/// A continuation family for a graph whose cells only store.
struct Step;
reattachable!(Step => ());

/// What a slot of the program holds when the check runs.
pub(super) enum Held<'graph, 'cell> {
    Bound(Value<'graph, 'cell>),
    /// Claimed by a binder still running.
    Pending,
}

/// What a check reads: the fixture's storage and the activated program.
pub(super) struct Program<'p, 'graph, 'cell> {
    pub types: &'p TypeRegistry<'graph>,
    pub labels: &'p LabelInterner,
    pub scratch: BumpAllocator<'p>,
    pub lines: &'p [KExpression<'graph>],
    pub activation: &'cell Activation<'graph, 'cell>,
    /// The cell a pending slot is claimed by.
    pub binder: CellHandle,
}

impl<'graph> Program<'_, 'graph, '_> {
    pub fn type_name(&self, text: &str) -> TypeSymbol {
        TypeSymbol::declared(text, self.labels).expect("a Type token")
    }

    /// The body shape the binder `name` births.
    pub fn birth(&self, name: &str) -> &'graph Shape<'graph> {
        let name = BinderSymbol::classify(name).expect("a binder name");
        let (slot, _) = self
            .activation
            .shape()
            .slot(name)
            .expect("a declared binder");
        self.activation
            .shape()
            .births(slot)
            .expect("the binder births a callable")
    }
}

/// Parse and shape `source` over the scalar types and `extra` builtin types, bind every slot to what
/// `hold` gives for its name, and run `check`.
pub(super) fn with_program<R>(
    source: &str,
    extra: impl FnOnce(
        &TypeRegistry<'_>,
        BumpAllocator<'_>,
        &LabelInterner,
    ) -> Vec<(&'static str, KType)>,
    hold: impl for<'graph, 'cell> Fn(&str, Writer<'cell>, &TypeRegistry<'graph>) -> Held<'graph, 'cell>,
    check: impl for<'p, 'graph, 'cell> FnOnce(Program<'p, 'graph, 'cell>) -> R,
) -> R {
    let storage = program_storage();
    let program: ProgramBrand<'_> = storage.brand();
    let types = TypeRegistry::in_region(program.allocator());
    let labels = LabelInterner::new();
    let scratch = Bump::new();
    let lines = parse(program, &labels, source)
        .unwrap_or_else(|error| panic!("`{source}` parses: {error:?}"));
    let extra = extra(&types, &scratch, &labels);
    let mut graph: CellGraph<'_, Step> = CellGraph::new(2, |_| Verdict::Pin);
    let cells: Vec<SlabHandle> = (0..2)
        .map(|_| graph.create(None, None).expect("the graph has a free slot"))
        .collect();
    let binder: CellHandle = cells[1].into();
    let out = graph
        .enter(cells[0], |context| {
            let writer = context.writer();
            let scalars = [
                ("Number", KType::NUMBER),
                ("Str", KType::STR),
                ("Bool", KType::BOOL),
                ("Null", KType::NULL),
                ("Any", KType::ANY),
            ];
            let table: Vec<_> = scalars
                .iter()
                .chain(extra.iter())
                .map(|(name, handle)| {
                    let name = TypeSymbol::declared(name, &labels).expect("a Type token");
                    (name, Value::Type(TypeValue::new(writer, *handle, &types)))
                })
                .collect();
            let builtins: &Builtins = Builtins::new(writer, &scratch, &[], &table);
            let shape = Shape::of_program(program, &lines, builtins, &scratch)
                .unwrap_or_else(|error| panic!("`{source}` shapes: {}", error.display(&labels)));
            let activation = resident(writer, Activation::of_program(writer, shape, builtins));
            for slot in 0..shape.slots() {
                let slot = Slot(slot as u32);
                let name = labels.display(shape.slot_name(slot).symbol()).to_string();
                activation.claim(slot, binder).expect("a fresh slot claims");
                if let Held::Bound(value) = hold(&name, writer, &types) {
                    activation.bind(slot, value).expect("a claimed slot binds");
                }
            }
            check(Program {
                types: &types,
                labels: &labels,
                scratch: &scratch,
                lines: &lines,
                activation,
                binder,
            })
        })
        .expect("a fresh cell is enterable");
    for cell in cells {
        graph
            .release(cell, ReleaseAbsorption::IntoHolder)
            .expect("a cell outside its step releases");
    }
    out
}

/// Every slot bound to `Null`.
pub(super) fn nulls<'graph, 'cell>(
    _: &str,
    _: Writer<'cell>,
    _: &TypeRegistry<'graph>,
) -> Held<'graph, 'cell> {
    Held::Bound(Value::Null)
}

/// No builtin type past the scalars.
pub(super) fn scalars(
    _: &TypeRegistry<'_>,
    _: BumpAllocator<'_>,
    _: &LabelInterner,
) -> Vec<(&'static str, KType)> {
    Vec::new()
}
