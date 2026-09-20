//! Shared scaffolding for `elaborate`'s suites: a program shaped over a builtin table of type
//! values, activated in a cell with each slot bound — or claimed — as the test asks.

mod boundary;
mod builtin;
mod declarations;
mod examples;
mod module;

use crate::memory::{
    Bump, BumpAllocator, CellGraph, CellHandle, ProgramBrand, ReleaseAbsorption, SlabHandle,
    Verdict, Writer, program_storage, reattachable, resident,
};
use crate::parse::{KExpression, parse};
use crate::scope::{
    Activation, Binding, BodyShape, Builtins, ClosureBindings, Coordinate, Slot, Target,
};
use crate::symbols::{BinderSymbol, SymbolInterner, TypeSymbol};
use crate::type_lattice::{KType, TypeRegistry};
use crate::values::{TypeValue, Value};

use super::{Elaboration, type_declarations};

/// This activation's own `slot`.
fn local(slot: Slot) -> Coordinate {
    Coordinate::Activation {
        hops: 0,
        target: Target::Local(slot),
    }
}

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
    pub symbols: &'p SymbolInterner,
    pub scratch: BumpAllocator<'p>,
    pub lines: &'p [KExpression<'graph>],
    pub activation: &'cell Activation<'graph, 'cell>,
    pub writer: Writer<'cell>,
    /// The cell a pending slot is claimed by.
    pub binder: CellHandle,
}

impl<'graph, 'cell> Program<'_, 'graph, 'cell> {
    pub fn type_name(&self, text: &str) -> TypeSymbol {
        TypeSymbol::declared(text, self.symbols).expect("a Type token")
    }

    /// Bring every component of type binders into being through the door, binding each member's
    /// slot as a type value. The first refusal comes back whole, having bound nothing of its own
    /// component.
    pub fn declare(&self) -> Result<(), Elaboration> {
        let shape = self.activation.shape();
        for component in shape.components() {
            let types_only = component
                .members
                .iter()
                .all(|slot| matches!(shape.slot_name(*slot), BinderSymbol::Type(_)));
            if !types_only {
                continue;
            }
            let handles = type_declarations(component, self.activation, self.types, self.scratch)?;
            for (slot, handle) in component.members.iter().zip(handles) {
                let value = Value::Type(TypeValue::new(self.writer, *handle, self.types));
                self.activation
                    .bind(*slot, value)
                    .expect("a claimed slot binds");
            }
        }
        Ok(())
    }

    /// The handle the type name `name` is bound to.
    pub fn bound(&self, name: &str) -> KType {
        let name = BinderSymbol::Type(self.type_name(name));
        let (slot, _) = self
            .activation
            .shape()
            .slot(name)
            .expect("a declared type binder");
        match self.activation.read(local(slot)) {
            Binding::Bound(Value::Type(value)) => value.handle(),
            _ => panic!("`{name:?}` is bound to a type"),
        }
    }

    /// Whether the type name `name` is still claimed by its binder.
    pub fn unbound(&self, name: &str) -> bool {
        let name = BinderSymbol::Type(self.type_name(name));
        let (slot, _) = self
            .activation
            .shape()
            .slot(name)
            .expect("a declared type binder");
        matches!(self.activation.read(local(slot)), Binding::Pending(_))
    }

    /// The activation of the module body the binder `name` births, every slot claimed. The body
    /// must capture nothing: a test binds its slots by hand.
    pub fn module_body(&self, name: &str) -> &'cell Activation<'graph, 'cell> {
        let body = self.birth(name);
        assert!(body.captures().is_empty(), "this body captures nothing");
        let activation = resident(
            self.writer,
            Activation::of_module(
                self.writer,
                body,
                ClosureBindings::empty(),
                self.activation.builtins(),
            ),
        );
        for slot in 0..body.slots() {
            activation
                .claim(Slot(slot as u32), self.binder)
                .expect("a fresh slot claims");
        }
        activation
    }

    /// Bind `name`'s slot in `body` to `value`.
    pub fn bind_member(
        &self,
        body: &Activation<'graph, 'cell>,
        name: &str,
        value: Value<'graph, 'cell>,
    ) {
        let name = BinderSymbol::classify(name).expect("a binder name");
        let (slot, _) = body.shape().slot(name).expect("a declared binder");
        body.bind(slot, value).expect("a claimed slot binds");
    }

    /// The body shape the binder `name` births.
    pub fn birth(&self, name: &str) -> &'graph BodyShape<'graph> {
        let name = BinderSymbol::classify(name).expect("a binder name");
        let (slot, _) = self
            .activation
            .shape()
            .slot(name)
            .expect("a declared binder");
        self.activation
            .shape()
            .births(slot)
            .expect("the binder births a body")
    }
}

/// Parse and shape `source` over the scalar types and `extra` builtin types, bind every slot to what
/// `hold` gives for its name, and run `check`.
pub(super) fn with_program<R>(
    source: &str,
    extra: impl FnOnce(
        &TypeRegistry<'_>,
        BumpAllocator<'_>,
        &SymbolInterner,
    ) -> Vec<(&'static str, KType)>,
    hold: impl for<'graph, 'cell> Fn(&str, Writer<'cell>, &TypeRegistry<'graph>) -> Held<'graph, 'cell>,
    check: impl for<'p, 'graph, 'cell> FnOnce(Program<'p, 'graph, 'cell>) -> R,
) -> R {
    let storage = program_storage();
    let program: ProgramBrand<'_> = storage.brand();
    let types = TypeRegistry::in_region(program.allocator());
    let symbols = SymbolInterner::new();
    let scratch = Bump::new();
    let lines = parse(program, &symbols, source)
        .unwrap_or_else(|error| panic!("`{source}` parses: {error:?}"));
    let extra = extra(&types, &scratch, &symbols);
    let mut graph: CellGraph<'_, Step> = CellGraph::new(2, |_| Verdict::Pin);
    let cells: Vec<SlabHandle> = (0..2)
        .map(|_| graph.create(None).expect("the graph has a free slot"))
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
                    let name = TypeSymbol::declared(name, &symbols).expect("a Type token");
                    (name, Value::Type(TypeValue::new(writer, *handle, &types)))
                })
                .collect();
            let builtins: &Builtins = Builtins::new(writer, &scratch, &[], &table);
            let shape = BodyShape::of_program(program, &lines, builtins, &scratch)
                .unwrap_or_else(|error| panic!("`{source}` shapes: {}", error.display(&symbols)));
            let activation = resident(writer, Activation::of_program(writer, shape, builtins));
            for slot in 0..shape.slots() {
                let slot = Slot(slot as u32);
                let name = symbols.display(shape.slot_name(slot).symbol()).to_string();
                activation.claim(slot, binder).expect("a fresh slot claims");
                if let Held::Bound(value) = hold(&name, writer, &types) {
                    activation.bind(slot, value).expect("a claimed slot binds");
                }
            }
            check(Program {
                types: &types,
                symbols: &symbols,
                scratch: &scratch,
                // A reader takes a body's statements from its shape, never from the parse: the
                // shape owns them rewritten, and every site it records is an address inside them.
                lines: shape.body(),
                activation,
                writer,
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
    _: &SymbolInterner,
) -> Vec<(&'static str, KType)> {
    Vec::new()
}
