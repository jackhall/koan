//! Shared scaffolding for `function`'s suites: program storage with a registry and an interner, a
//! builtin table, and a runner that activates a program in a cell and brings every binding it can
//! into being — each component of type binders through the declaration door, each cyclic component
//! of value binders and each component of callable binders through the tie, and each lone data
//! binder whose right-hand side lowers — beside readers for what a slot then holds.
//!
//! A module binder is brought body-first: the runner builds its body's activation, claims and runs
//! every component of that body, and only then ties the binder with the finished activation. The
//! layer above reads this fixture, so its doors are `pub(crate)`.

mod birth;
mod boundary;
mod coerced;
mod copy;
mod equality;
mod module;
mod properties;

use crate::elaborate::type_declarations;
use crate::memory::{
    Bump, BumpAllocator, CellGraph, CellHandle, Prices, ProgramBrand, ReleaseAbsorption,
    StepContext, Verdict, Writer, program_storage, reattachable, resident,
};
use crate::parse::builtin_shapes::role::{BodyKind, Role};
use crate::parse::{KExpression, parse};
use crate::scope::{Binding, BodyShape, Builtins, Component, Slot};
use crate::symbols::{BinderSymbol, SymbolInterner, TypeSymbol, ValueSymbol};
use crate::type_lattice::{KType, TypeRegistry};
use crate::values::{Circular, Knotted as _, Link, TypeValue, Value};

use super::module::module_activation;
use super::{KActivation, KValue, Knotted, Supplied, tie};

/// A continuation family for a graph whose cells only store.
pub(crate) struct Step;
reattachable!(Step => ());

pub(crate) struct Fixture<'f, 'graph> {
    pub program: ProgramBrand<'graph>,
    pub types: &'f TypeRegistry<'graph>,
    pub symbols: &'f SymbolInterner,
    scratch: &'f Bump,
}

/// Run `test` against a fresh fixture.
pub(crate) fn with_fixture<R>(test: impl for<'f, 'graph> FnOnce(&Fixture<'f, 'graph>) -> R) -> R {
    let storage = program_storage();
    let program = storage.brand();
    let types = TypeRegistry::in_region(program.allocator());
    let symbols = SymbolInterner::new();
    let scratch = Bump::new();
    test(&Fixture {
        program,
        types: &types,
        symbols: &symbols,
        scratch: &scratch,
    })
}

impl<'graph> Fixture<'_, 'graph> {
    pub fn scratch(&self) -> BumpAllocator<'_> {
        self.scratch
    }

    /// Every top-level line of `source`, parsed into program storage.
    pub fn parse(&self, source: &str) -> Vec<KExpression<'graph>> {
        parse(self.program, self.symbols, source)
            .unwrap_or_else(|error| panic!("`{source}` parses: {error:?}"))
    }

    /// Run `step` in one cell of a graph built under `verdict`, beside the handle of a second cell
    /// that stands for a running binder.
    pub fn in_cell<R>(
        &self,
        verdict: fn(Prices) -> Verdict,
        step: impl for<'step, 'here, 'scratch> FnOnce(
            &mut StepContext<'graph, 'step, 'here, 'scratch, Step>,
            CellHandle,
        ) -> R,
    ) -> R {
        let mut graph: CellGraph<'graph, Step> = CellGraph::new(2, verdict);
        let cell = graph.create(None).expect("the graph has a free slot");
        let other = graph.create(None).expect("the graph has a free slot");
        let out = graph
            .enter(cell, |context| step(context, other.into()))
            .expect("a fresh cell is enterable");
        for released in [cell, other] {
            graph
                .release(released, ReleaseAbsorption::IntoHolder)
                .expect("a cell outside its step releases");
        }
        out
    }

    pub fn name(&self, text: &str) -> BinderSymbol {
        BinderSymbol::declared(text, self.symbols).expect("a binder name")
    }

    /// `origin = 0` and the scalar types, laid down in `writer`'s region.
    pub fn builtins<'cell>(
        &self,
        writer: Writer<'cell>,
    ) -> &'cell Builtins<'graph, 'cell, Knotted<'graph, 'cell>> {
        let origin = ValueSymbol::declared("origin", self.symbols).expect("a value token");
        let types: Vec<_> = [
            ("Number", KType::NUMBER),
            ("Str", KType::STR),
            ("Bool", KType::BOOL),
            ("Null", KType::NULL),
            ("Any", KType::ANY),
        ]
        .into_iter()
        .map(|(name, handle)| {
            let name = TypeSymbol::declared(name, self.symbols).expect("a Type token");
            (
                name,
                Value::Type(TypeValue::new(writer, handle, self.types)),
            )
        })
        .collect();
        Builtins::new(
            writer,
            self.scratch,
            &[(origin, Value::Number(0.0))],
            &types,
        )
    }

    /// `lines` shaped and activated in `writer`'s region with every slot claimed by `binder`, then
    /// run: each component, in the order the shape emitted them, is brought into being unless one
    /// of its members is named in `leave`.
    pub fn run<'cell>(
        &self,
        writer: Writer<'cell>,
        lines: &[KExpression<'graph>],
        binder: CellHandle,
        leave: &[&str],
    ) -> &'cell KActivation<'graph, 'cell> {
        let builtins = self.builtins(writer);
        let shape = BodyShape::of_program(self.program, lines, builtins, self.scratch)
            .unwrap_or_else(|error| panic!("the program shapes: {}", error.display(self.symbols)));
        let activation = resident(writer, KActivation::of_program(writer, shape, builtins));
        for slot in 0..shape.slots() {
            activation
                .claim(Slot(slot as u32), binder)
                .expect("a fresh slot claims");
        }
        let left: Vec<BinderSymbol> = leave.iter().map(|name| self.name(name)).collect();
        let statements: Vec<&KExpression<'graph>> = lines.iter().collect();
        for component in shape.components() {
            let names = component.members.iter().map(|slot| shape.slot_name(*slot));
            if names.clone().any(|name| left.contains(&name)) {
                continue;
            }
            self.bring_in(writer, activation, &statements, binder, component);
        }
        activation
    }

    /// Bring one component of `activation` into being, if the runner can: declare a component of
    /// type binders through the door, tie a component of value binders when it is cyclic or every
    /// member births a callable, run and tie a module binder, else bind each member whose
    /// right-hand side lowers. `statements` is the body `activation`'s shape was built from.
    fn bring_in<'cell>(
        &self,
        writer: Writer<'cell>,
        activation: &KActivation<'graph, 'cell>,
        statements: &[&KExpression<'graph>],
        binder: CellHandle,
        component: &Component<'graph>,
    ) {
        let shape = activation.shape();
        let values = component
            .members
            .iter()
            .all(|slot| matches!(shape.slot_name(*slot), BinderSymbol::Value(_)));
        if !values {
            let handles = type_declarations(component, activation, self.types, self.scratch)
                .expect("the component declares its types");
            for (slot, handle) in component.members.iter().zip(handles) {
                let value = Value::Type(TypeValue::new(writer, *handle, self.types));
                activation.bind(*slot, value).expect("a claimed slot binds");
            }
            return;
        }
        if let [slot] = component.members
            && shape
                .births(*slot)
                .is_some_and(|body| body.kind() == crate::scope::ShapeKind::Module)
        {
            self.bring_module(writer, activation, statements, binder, component, *slot);
            return;
        }
        let births = component
            .members
            .iter()
            .all(|slot| shape.births(*slot).is_some());
        if component.cyclic || births {
            let knot = tie(
                writer,
                activation,
                component,
                self.types,
                self.scratch,
                &mut |_| None,
            )
            .expect("the component ties");
            for (index, slot) in component.members.iter().enumerate() {
                activation
                    .bind(*slot, Value::Knotted(Knotted::of(knot, index)))
                    .expect("a claimed slot binds");
            }
            return;
        }
        for slot in component.members {
            let Some(statement) = self.statement_of(shape, *slot) else {
                continue;
            };
            let spine = statements[statement].statement_spine();
            let Some(rhs) = spine.parts.get(3) else {
                continue;
            };
            if let Some(value) = Value::lower_part(writer, &rhs.value, self.types, self.scratch) {
                activation.bind(*slot, value).expect("a claimed slot binds");
            }
        }
    }

    /// Bring a module binder into being, body first: its body's activation laid down with every
    /// slot claimed, each of the body's own components brought in, then the binder tied.
    fn bring_module<'cell>(
        &self,
        writer: Writer<'cell>,
        activation: &KActivation<'graph, 'cell>,
        statements: &[&KExpression<'graph>],
        binder: CellHandle,
        component: &Component<'graph>,
        slot: Slot,
    ) {
        let shape = activation.shape();
        let statement = self
            .statement_of(shape, slot)
            .expect("a module binder binds a statement");
        let body_node = module_body_node(statements[statement].statement_spine());
        let inner: Vec<&KExpression<'graph>> =
            body_node.body_statements().map(|(node, _)| node).collect();
        let body = resident(
            writer,
            module_activation(writer, activation, slot, self.scratch)
                .expect("the module body's captures are bound"),
        );
        for index in 0..body.shape().slots() {
            body.claim(Slot(index as u32), binder)
                .expect("a fresh slot claims");
        }
        for inner_component in body.shape().components() {
            self.bring_in(writer, body, &inner, binder, inner_component);
        }
        let knot = tie(
            writer,
            activation,
            component,
            self.types,
            self.scratch,
            &mut |_| Some(Supplied::Body(body)),
        )
        .expect("the module ties");
        activation
            .bind(slot, Value::Knotted(Knotted::of(knot, 0)))
            .expect("a claimed slot binds");
    }

    /// The statement `slot`'s binder is declared at, if it is not a parameter.
    fn statement_of(&self, shape: &BodyShape<'graph>, slot: Slot) -> Option<usize> {
        let (_, position) = shape
            .slot(shape.slot_name(slot))
            .expect("a member is declared");
        position.0.checked_sub(1).map(|at| at as usize)
    }
}

/// The body a `MODULE` or `GROUP` node births.
fn module_body_node<'graph>(node: &KExpression<'graph>) -> &'graph KExpression<'graph> {
    let form = node
        .cache()
        .builtin_shape()
        .expect("a module binder is a builtin form");
    let (_, part) = form
        .roles()
        .zip(node.parts)
        .find(|(role, _)| matches!(role, Role::Body(BodyKind::Module)))
        .expect("a module binder has a module body");
    match part.value {
        crate::parse::ExpressionPart::Expression(body) => body.reference(),
        _ => panic!("a module body is a node"),
    }
}

/// Pin every operand.
pub(crate) fn pin(_: Prices) -> Verdict {
    Verdict::Pin
}

/// Copy every operand.
pub(crate) fn copy(_: Prices) -> Verdict {
    Verdict::Copy
}

/// What `activation` holds under `name`.
pub(crate) fn read<'graph, 'cell>(
    fixture: &Fixture<'_, 'graph>,
    activation: &KActivation<'graph, 'cell>,
    name: &str,
) -> Binding<'graph, 'cell, Knotted<'graph, 'cell>> {
    let shape = activation.shape();
    let (slot, _) = shape.slot(fixture.name(name)).expect("a declared binder");
    activation.read(crate::scope::Coordinate::Activation {
        hops: 0,
        target: crate::scope::Target::Local(slot),
    })
}

/// The value bound under `name`, which must be bound.
pub(crate) fn bound<'graph, 'cell>(
    fixture: &Fixture<'_, 'graph>,
    activation: &KActivation<'graph, 'cell>,
    name: &str,
) -> KValue<'graph, 'cell> {
    match read(fixture, activation, name) {
        Binding::Bound(value) => value,
        Binding::Pending(_) => panic!("`{name}` is still pending"),
    }
}

/// The type handle bound under the type name `name`.
pub(crate) fn declared<'graph>(
    fixture: &Fixture<'_, 'graph>,
    activation: &KActivation<'graph, '_>,
    name: &str,
) -> KType {
    match bound(fixture, activation, name) {
        Value::Type(value) => value.handle(),
        _ => panic!("`{name}` is bound to a type"),
    }
}

/// The callable bound under `name`.
pub(crate) fn callable<'graph, 'cell>(
    fixture: &Fixture<'_, 'graph>,
    activation: &KActivation<'graph, 'cell>,
    name: &str,
) -> Knotted<'graph, 'cell> {
    bound(fixture, activation, name)
        .as_callable()
        .unwrap_or_else(|| panic!("`{name}` is bound to a callable"))
}

/// The data node `value` is, which must be one.
pub(crate) fn circular<'graph, 'cell>(
    value: KValue<'graph, 'cell>,
) -> (
    Knotted<'graph, 'cell>,
    Circular<'cell, 'cell, Knotted<'graph, 'cell>>,
) {
    value.as_circular().expect("a data node")
}

/// The link at the edge `link` names, resolved through `holder` to the data node it is.
pub(crate) fn follow<'graph, 'cell>(
    holder: Knotted<'graph, 'cell>,
    link: Link<'_, '_, Knotted<'graph, 'cell>>,
) -> Knotted<'graph, 'cell> {
    match link {
        Link::Edge(edge) => holder.sibling(edge),
        Link::Value(_) => panic!("an edge"),
    }
}
