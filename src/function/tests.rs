//! Shared scaffolding for `function`'s suites: program storage with a registry and an interner, a
//! builtin table, and a runner that activates a program in a cell and brings every binding it can
//! into being — each lone data binder whose right-hand side lowers, each type binder whose
//! right-hand side elaborates, each cyclic component of value binders and each component of
//! callable binders through the tie — beside helpers that seal the newtypes a program constructs.

mod birth;
mod boundary;
mod copy;
mod equality;
mod properties;

use crate::elaborate::type_expression;
use crate::memory::{
    Bump, BumpAllocator, CellGraph, CellHandle, Prices, ProgramBrand, ReleaseAbsorption,
    StepContext, Verdict, Writer, program_storage, reattachable, resident,
};
use crate::parse::{BinderSymbol, KExpression, LabelInterner, TypeSymbol, ValueSymbol, parse};
use crate::scope::{Binding, BodyShape, Builtins, Component, Slot};
use crate::type_lattice::{KType, RecursiveGroupWindow, RelativeSchema, TypeRegistry};
use crate::values::{Circular, Knotted as _, Link, TypeValue, Value};

use super::{KActivation, KValue, Knotted, tie};

/// A continuation family for a graph whose cells only store.
pub(super) struct Step;
reattachable!(Step => ());

pub(super) struct Fixture<'f, 'graph> {
    pub program: ProgramBrand<'graph>,
    pub types: &'f TypeRegistry<'graph>,
    pub labels: &'f LabelInterner,
    scratch: &'f Bump,
}

/// Run `test` against a fresh fixture.
pub(super) fn with_fixture<R>(test: impl for<'f, 'graph> FnOnce(&Fixture<'f, 'graph>) -> R) -> R {
    let storage = program_storage();
    let program = storage.brand();
    let types = TypeRegistry::in_region(program.allocator());
    let labels = LabelInterner::new();
    let scratch = Bump::new();
    test(&Fixture {
        program,
        types: &types,
        labels: &labels,
        scratch: &scratch,
    })
}

impl<'graph> Fixture<'_, 'graph> {
    pub fn scratch(&self) -> BumpAllocator<'_> {
        self.scratch
    }

    /// Every top-level line of `source`, parsed into program storage.
    pub fn parse(&self, source: &str) -> Vec<KExpression<'graph>> {
        parse(self.program, self.labels, source)
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
        BinderSymbol::declared(text, self.labels).expect("a binder name")
    }

    /// `origin = 0`, the scalar types and `nominals`, laid down in `writer`'s region.
    pub fn builtins<'cell>(
        &self,
        writer: Writer<'cell>,
        nominals: &[(&str, KType)],
    ) -> &'cell Builtins<'graph, 'cell, Knotted<'graph, 'cell>> {
        let origin = ValueSymbol::declared("origin", self.labels).expect("a value token");
        let types: Vec<_> = [
            ("Number", KType::NUMBER),
            ("Str", KType::STR),
            ("Bool", KType::BOOL),
            ("Null", KType::NULL),
            ("Any", KType::ANY),
        ]
        .into_iter()
        .chain(nominals.iter().copied())
        .map(|(name, handle)| {
            let name = TypeSymbol::declared(name, self.labels).expect("a Type token");
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
        self.run_with(writer, lines, binder, leave, &[])
    }

    /// [`run`](Self::run) with `nominals` in the builtin table beside the scalar types.
    pub fn run_with<'cell>(
        &self,
        writer: Writer<'cell>,
        lines: &[KExpression<'graph>],
        binder: CellHandle,
        leave: &[&str],
        nominals: &[(&str, KType)],
    ) -> &'cell KActivation<'graph, 'cell> {
        let builtins = self.builtins(writer, nominals);
        let shape = BodyShape::of_program(self.program, lines, builtins, self.scratch)
            .unwrap_or_else(|error| panic!("the program shapes: {}", error.display(self.labels)));
        let activation = resident(writer, KActivation::of_program(writer, shape, builtins));
        for slot in 0..shape.slots() {
            activation
                .claim(Slot(slot as u32), binder)
                .expect("a fresh slot claims");
        }
        let left: Vec<BinderSymbol> = leave.iter().map(|name| self.name(name)).collect();
        for component in shape.components() {
            let names = component.members.iter().map(|slot| shape.slot_name(*slot));
            if names.clone().any(|name| left.contains(&name)) {
                continue;
            }
            self.bring(writer, activation, lines, component);
        }
        activation
    }

    /// Bring one component into being, if the runner can: tie a component of value binders when it
    /// is cyclic or every member births a callable, else bind each member whose right-hand side
    /// lowers or elaborates.
    fn bring<'cell>(
        &self,
        writer: Writer<'cell>,
        activation: &KActivation<'graph, 'cell>,
        lines: &[KExpression<'graph>],
        component: &Component<'graph>,
    ) {
        let shape = activation.shape();
        let values = component
            .members
            .iter()
            .all(|slot| matches!(shape.slot_name(*slot), BinderSymbol::Value(_)));
        let births = component
            .members
            .iter()
            .all(|slot| shape.births(*slot).is_some());
        if values && (component.cyclic || births) {
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
            let (_, position) = shape
                .slot(shape.slot_name(*slot))
                .expect("a member is declared");
            let Some(statement) = position.0.checked_sub(1) else {
                continue;
            };
            let spine = lines[statement as usize].statement_spine();
            let Some(rhs) = spine.parts.get(3) else {
                continue;
            };
            let value =
                Value::lower_part(writer, &rhs.value, self.types, self.scratch).or_else(|| {
                    let handle =
                        type_expression(&rhs.value, activation, &[], self.types, self.scratch)
                            .ok()?;
                    Some(Value::Type(TypeValue::new(writer, handle, self.types)))
                });
            if let Some(value) = value {
                activation.bind(*slot, value).expect("a claimed slot binds");
            }
        }
    }
}

impl Fixture<'_, '_> {
    /// `NEWTYPE <name> = :{<field> :<name>}`, sealed as a singleton recursive group.
    pub fn ring_type(&self, name: &str, field: &str) -> KType {
        let (types, scratch) = (self.types, self.scratch());
        let representation = types.record(scratch, &[(self.name(field), types.sibling(0))]);
        self.newtype(name, representation)
    }

    /// `NEWTYPE <name> = <representation>`, sealed as a singleton recursive group.
    pub fn newtype(&self, name: &str, representation: KType) -> KType {
        RecursiveGroupWindow::seal_singleton(
            self.scratch(),
            TypeSymbol::declared(name, self.labels).expect("a Type token"),
            RelativeSchema::NewType(representation),
            None,
            self.types,
            self.scratch(),
        )
    }
}

/// Pin every operand.
pub(super) fn pin(_: Prices) -> Verdict {
    Verdict::Pin
}

/// Copy every operand.
pub(super) fn copy(_: Prices) -> Verdict {
    Verdict::Copy
}

/// What `activation` holds under `name`.
pub(super) fn read<'graph, 'cell>(
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
pub(super) fn bound<'graph, 'cell>(
    fixture: &Fixture<'_, 'graph>,
    activation: &KActivation<'graph, 'cell>,
    name: &str,
) -> KValue<'graph, 'cell> {
    match read(fixture, activation, name) {
        Binding::Bound(value) => value,
        Binding::Pending(_) => panic!("`{name}` is still pending"),
    }
}

/// The callable bound under `name`.
pub(super) fn callable<'graph, 'cell>(
    fixture: &Fixture<'_, 'graph>,
    activation: &KActivation<'graph, 'cell>,
    name: &str,
) -> Knotted<'graph, 'cell> {
    bound(fixture, activation, name)
        .as_callable()
        .unwrap_or_else(|| panic!("`{name}` is bound to a callable"))
}

/// The data node `value` is, which must be one.
pub(super) fn circular<'graph, 'cell>(
    value: KValue<'graph, 'cell>,
) -> (
    Knotted<'graph, 'cell>,
    Circular<'cell, 'cell, Knotted<'graph, 'cell>>,
) {
    value.as_circular().expect("a data node")
}

/// The link at the edge `link` names, resolved through `holder` to the data node it is.
pub(super) fn follow<'graph, 'cell>(
    holder: Knotted<'graph, 'cell>,
    link: Link<'_, '_, Knotted<'graph, 'cell>>,
) -> Knotted<'graph, 'cell> {
    match link {
        Link::Edge(edge) => holder.sibling(edge),
        Link::Value(_) => panic!("an edge"),
    }
}
