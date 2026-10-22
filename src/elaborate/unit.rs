//! The type half of the one walk: the type channel's rules, run unit by unit over a
//! [`TypeContext`] the walk implements ([the one-walk build](../../roadmap/gradual-build/one-walk-build.md)).
//!
//! Each entry point types one fact — a component of type binders, a recorded type expression, a
//! registration, a callable's type, a callable body's own group — through the one load-time
//! reader, [`Reader`], and hands it back; every unknown it read is left in the context's
//! gathering ([`TypeContext::note`]). The elaborator's doors do the typing: nothing here mints a
//! level or a path. A run-bound name's level is the shape's ([`Shapes::level`]), and a path is read
//! through the resolution the walk fixed before the unit's declarations
//! ([`TypeContext::resolution`]).
//!
//! The reader answers a coordinate with a builtin's value; else with the closed type the binding
//! gives it; else, for a type binder, with its declaration: closed as it is, otherwise the binder
//! is an unknown read as a lexical variable; else, for any other run-bound type name, with its
//! lexical variable at its shape level. A path reads its resolution: closed as it is, and in
//! `Typing` mode a class as its representative's path variable. In `Declaring` mode anything not
//! closed reads unknown, since a type binder is closed or unknown, never rigid.
//!
//! A fact's values are laid through the walk's `lay`, at the records' lifetime; every other value
//! rests in the scratch the rules are handed.

use std::cell::Cell;

use crate::memory::{BumpAllocator, BumpVec, Lays, collect};
use crate::parse::quoted_body;
use crate::parse::{KExpression, Role};
use crate::scope::{
    Address, Binder, BodyShape, Bound, Builtins, Callable, CaptureSlot, CaptureSource, Component,
    Coordinate, Elaboration, FunctionGroupMap, ParameterBinding, Registered, Registration,
    Resolved, Segment, ShapeError, ShapeGroupMap, ShapeKind, Shapes, Site, Slot, Source, Static,
    StaticCallable, StaticRegistered, StaticType, SurfacedHead, Target, TypeExpression, Unknown,
    Variable, source_of,
};
use crate::source::SourceRef;
use crate::symbols::{BinderSymbol, TypeSymbol, ValueSymbol};
use crate::type_lattice::{
    DeclaredType, KType, Members, Parametric, SigSchema, TypeNode, substitute_parameters,
};
use crate::values::{Chain, Knotted, Value};

use super::declaration::{HeadShape, signature_heads, type_declarations};
use super::expression::{Elaborator, Groups, type_expression};
use super::reads::{Reads, TypeAt};
use super::signature::callable_type;

/// What the type half reads of the walk: the shape table, the binding, and every fact the walk has
/// fixed so far. Each read of a fact gathers what the fact waits on.
pub trait TypeContext<'graph, 'o> {
    fn shapes(&self) -> &'graph Shapes<'graph>;
    /// What the binding gives `unknown`, where it binds it.
    fn binding(&self, unknown: Unknown) -> Option<Bound<'o>>;
    /// A type binder's declaration as far as the walk has it.
    fn declared(&self, binder: Binder) -> StaticType<'o>;
    /// A registration's entry as far as the walk has it.
    fn registered(&self, binder: Binder) -> StaticRegistered<'o>;
    /// A callable body's type as far as the walk has it.
    fn callable(&self, body: Segment) -> StaticCallable<'o>;
    /// A callable body's own `FOR ALL` group as its body reads it, by group order: empty where the
    /// walk has not typed it.
    fn group_levels(&self, body: Segment) -> &'o [Parametric];
    /// A recorded type expression's type, for a surfaced head reading its ascription.
    fn expression(&self, segment: Segment, site: Site) -> StaticType<'o>;
    /// What the path at `level` of the chain `root` roots resolves to, where the walk resolved it.
    fn resolution(&self, root: Segment, level: usize) -> Option<Resolved>;
    /// Gather `unknowns`: each the binding leaves unbound, of a shape on the reader's chain.
    fn note(&self, unknowns: &[Unknown]);
}

/// What every rule of the type half is handed beside its context: the builtin table, the chain it
/// interns through, and its scratch.
pub struct Tools<'e, 'p, 'cell, 'tier, 'pair, 'reg, 'door, X> {
    pub builtins: &'e Builtins<'cell, X>,
    pub types: &'e Chain<'tier, 'pair, 'reg, 'door>,
    pub scratch: BumpAllocator<'p>,
}

impl<X> Clone for Tools<'_, '_, '_, '_, '_, '_, '_, X> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<X> Copy for Tools<'_, '_, '_, '_, '_, '_, '_, X> {}

/// Whether a reader is typing a type binder, which is closed or unknown, or anything else.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Declaring,
    Typing,
}

/// What a coordinate holds, as far as the load can tell.
enum Class {
    Type(KType),
    NotAType,
    /// A run-bound type name of `segment`, held at `address`, bounded by `bound`.
    RunBound {
        segment: Segment,
        address: Address,
        bound: KType,
    },
    /// One of a callable's own `FOR ALL` names: this lexical variable.
    Variable(Parametric),
}

/// The load-time reader: reads the names mentioned in `shape` through the walk's context.
pub struct Reader<'e, 'p, 'graph, 'o, 'cell, 'tier, 'pair, 'reg, 'door, C: ?Sized, X> {
    context: &'e C,
    tools: Tools<'e, 'p, 'cell, 'tier, 'pair, 'reg, 'door, X>,
    shape: &'graph BodyShape<'graph>,
    mode: Mode,
    rigid: Cell<u32>,
    /// Each rigid variable read, once per level.
    variables: Cell<BumpVec<'p, Variable>>,
    records: std::marker::PhantomData<&'o ()>,
}

impl<'e, 'p, 'graph, 'o, 'cell, 'tier, 'pair, 'reg, 'door, C, X>
    Reader<'e, 'p, 'graph, 'o, 'cell, 'tier, 'pair, 'reg, 'door, C, X>
where
    C: TypeContext<'graph, 'o> + ?Sized,
    X: Knotted,
{
    /// A reader of the names `shape` mentions, typing in `mode`.
    pub fn new(
        context: &'e C,
        tools: Tools<'e, 'p, 'cell, 'tier, 'pair, 'reg, 'door, X>,
        shape: &'graph BodyShape<'graph>,
        mode: Mode,
    ) -> Self {
        Reader {
            context,
            tools,
            shape,
            mode,
            rigid: Cell::new(0),
            variables: Cell::new(BumpVec::new_in(tools.scratch)),
            records: std::marker::PhantomData,
        }
    }

    fn shapes(&self) -> &'graph Shapes<'graph> {
        self.context.shapes()
    }

    /// The closed type the binding gives the type name at `address` of `segment`.
    fn bound(&self, segment: Segment, address: Address) -> Option<KType> {
        match self.context.binding(Unknown {
            segment,
            at: address,
        })? {
            Bound::Type(bound) => Some(bound),
            Bound::Module { .. } => None,
        }
    }

    /// What `at`, read in `segment`, holds, each unknown the read reaches gathered.
    fn classify(&self, segment: Segment, at: Coordinate) -> Class {
        let (hops, target) = match at {
            Coordinate::Builtin(index) => {
                return match self.tools.builtins.get(index) {
                    Value::Type(value) => Class::Type(value.handle()),
                    _ => Class::NotAType,
                };
            }
            Coordinate::Activation { hops, target } => (hops, target),
        };
        let segment = (self.shapes().up(segment, hops))
            .expect("a coordinate steps out only through shapes on the chain");
        match target {
            Target::Local(slot) => self.slot_at(segment, slot),
            Target::Capture(capture) => self.capture_at(segment, capture),
        }
    }

    /// What the slot `slot` of `segment` holds.
    fn slot_at(&self, segment: Segment, slot: Slot) -> Class {
        let shape = self.shapes().shape_at(segment);
        let address = Address::Slot(slot);
        if let Some(bound) = self.bound(segment, address) {
            return Class::Type(bound);
        }
        let unknown = Unknown {
            segment,
            at: address,
        };
        if shape.declarations(slot).is_some() {
            return match self.context.declared(Binder {
                body: segment,
                slot,
            }) {
                Static::Closed(handle) => Class::Type(handle),
                _ => {
                    self.context.note(&[unknown]);
                    Class::RunBound {
                        segment,
                        address,
                        bound: KType::ANY,
                    }
                }
            };
        }
        let BinderSymbol::Type(name) = shape.slot_name(slot) else {
            return Class::NotAType;
        };
        self.context.note(&[unknown]);
        if let Some(variable) = self.own(shape, name) {
            return Class::Variable(variable);
        }
        Class::RunBound {
            segment,
            address,
            bound: KType::ANY,
        }
    }

    /// The lexical variable `name`, one of the callable body `shape`'s own `FOR ALL` names, is in
    /// its body: `None` for any other name, or where the walk has not typed the callable.
    fn own(&self, shape: &BodyShape<'graph>, name: TypeSymbol) -> Option<Parametric> {
        if shape.kind() != ShapeKind::Callable {
            return None;
        }
        let map = match self.context.callable(shape.segment()) {
            Static::Closed(callable) => callable.quantifier_map,
            Static::Rigid { value, .. } => value.quantifier_map,
            Static::Unknown => return None,
        };
        let index = map.get(name)?;
        self.context
            .group_levels(shape.segment())
            .get(index)
            .copied()
    }

    /// What the capture `capture` of `segment` holds: what its source reads where the shape is
    /// born — the very name, and so its level, when the shape shares its parent's chain — and
    /// otherwise, crossing into code, a run-bound name of the code's own, bounded as the name it
    /// reads.
    fn capture_at(&self, segment: Segment, capture: CaptureSlot) -> Class {
        let shape = self.shapes().shape_at(segment);
        let address = Address::Capture(capture);
        let root = shape.kind() == ShapeKind::Code;
        if root {
            if let Some(bound) = self.bound(segment, address) {
                return Class::Type(bound);
            }
            // A name crossing into code is one of the code's own, beside the name it reads.
            self.context.note(&[Unknown {
                segment,
                at: address,
            }]);
        }
        let parent = self.shapes().parent(segment).map(|(parent, _)| parent);
        match (shape.captures()[capture.index()].source, parent) {
            (CaptureSource::Read(inner), Some(parent)) => match self.classify(parent, inner) {
                read if !root => read,
                Class::Type(handle) => Class::Type(handle),
                Class::NotAType => Class::NotAType,
                Class::RunBound { bound, .. } => Class::RunBound {
                    segment,
                    address,
                    bound,
                },
                Class::Variable(variable) => Class::RunBound {
                    segment,
                    address,
                    bound: (self.tools.types.node(variable).rigid_bound())
                        .expect("a lexical variable has a bound"),
                },
            },
            // A type name is never a knot member.
            (CaptureSource::Member { .. }, _) => Class::NotAType,
            _ => Class::RunBound {
                segment,
                address,
                bound: KType::ANY,
            },
        }
    }

    /// The lexical variable a run-bound name of `segment` held at `address` is, bounded by
    /// `bound`, at its shape level.
    fn variable(&self, segment: Segment, address: Address, bound: KType) -> Parametric {
        let shape = self.shapes().shape_at(segment);
        let name = match address {
            Address::Slot(slot) => shape.slot_name(slot),
            Address::Capture(capture) => shape.captures()[capture.index()].name,
        };
        let BinderSymbol::Type(name) = name else {
            unreachable!("a run-bound name is a type name");
        };
        let level =
            (self.shapes().level(segment, address)).expect("a run-bound type name has its level");
        self.tools.types.lexical(level, name, bound)
    }

    /// The lexical variable `variable`, read at `at`.
    fn rigid(&self, variable: Parametric, source: Source) -> TypeAt {
        let TypeNode::Lexical { level, .. } = self.tools.types.node(variable) else {
            unreachable!("a run-bound name reads as a lexical variable");
        };
        self.read(level, source);
        TypeAt::Rigid(variable)
    }

    /// Record a rigid read of the variable at `level`, once per level.
    fn read(&self, level: usize, source: Source) {
        self.rigid.set(self.rigid.get() + 1);
        let mut variables = self.variables.replace(BumpVec::new_in(self.tools.scratch));
        if !variables.iter().any(|held| held.level == level) {
            variables.push(Variable { level, source });
        }
        self.variables.set(variables);
    }

    /// The path `self.shape` spells at `site`, read through its resolution: `root` is where the
    /// shape reads its root, `None` for one written in place.
    fn path(&self, site: Site, at: Option<Coordinate>, label: TypeSymbol) -> TypeAt {
        let shapes = self.shapes();
        let segment = self.shape.segment();
        let index = (self.shape.spelled().iter())
            .position(|spelled| Site::of(spelled.part) == site)
            .expect("the builder records every path a type spells");
        let Some(level) = shapes.path_level(segment, index) else {
            return TypeAt::Unknown;
        };
        let root = shapes.chain_root(segment);
        match self.context.resolution(root, level) {
            Some(Resolved::Closed(closed)) => TypeAt::Type(closed),
            Some(Resolved::Class(class)) if self.mode == Mode::Typing => {
                let variable = self.tools.types.lexical(class, label, KType::ANY);
                self.rigid(
                    variable,
                    Source::Path {
                        at,
                        root,
                        level: class,
                    },
                )
            }
            _ => TypeAt::Unknown,
        }
    }

    /// `value`, typed through this reader: closed when it read no lexical variable — then concrete,
    /// through `close` — else rigid over the variables it read, laid through `lay`.
    fn fixed<V, R>(
        &self,
        lay: impl Lays<'o>,
        value: R,
        close: impl FnOnce(R) -> V,
    ) -> Static<'o, V, R> {
        if self.rigid.get() == 0 {
            return Static::Closed(close(value));
        }
        let variables = self.variables.replace(BumpVec::new_in(self.tools.scratch));
        let laid = collect(lay, variables.iter().copied());
        self.variables.set(variables);
        Static::Rigid {
            value,
            variables: laid,
        }
    }
}

impl<'graph, 'o, C, X> Reads<'graph> for Reader<'_, '_, 'graph, 'o, '_, '_, '_, '_, '_, C, X>
where
    C: TypeContext<'graph, 'o> + ?Sized,
    X: Knotted,
{
    fn shape(&self) -> &'graph BodyShape<'graph> {
        self.shape
    }

    fn type_at(&self, at: Coordinate) -> TypeAt {
        match self.classify(self.shape.segment(), at) {
            Class::Type(handle) => TypeAt::Type(handle),
            Class::NotAType => TypeAt::NotAType,
            _ if self.mode == Mode::Declaring => TypeAt::Unknown,
            Class::Variable(variable) => self.rigid(variable, Source::Type(at)),
            Class::RunBound {
                segment,
                address,
                bound,
            } => self.rigid(self.variable(segment, address, bound), Source::Type(at)),
        }
    }

    fn rigid_reads(&self) -> u32 {
        self.rigid.get()
    }

    /// A path through the binder `at` lands at, read through its resolution.
    fn member_type(
        &self,
        at: Coordinate,
        site: Site,
        _chain: &[ValueSymbol],
        label: TypeSymbol,
        _types: &Chain<'_, '_, '_, '_>,
    ) -> Option<(TypeAt, KType)> {
        Some((self.path(site, Some(at), label), KType::EMPTY_SIGNATURE))
    }

    /// A path through the expression at `root`, written in place, read through its resolution.
    fn inline_member(
        &self,
        site: Site,
        _root: Site,
        _chain: &[ValueSymbol],
        label: TypeSymbol,
    ) -> TypeAt {
        self.path(site, None, label)
    }
}

/// Why a load-time type that read no lexical variable is concrete: nothing it was elaborated under
/// declares a `FOR ALL` group or a head parameter it could name, and every binder door binds its
/// own group's variables, so none escapes free.
const FREE: &str = "a load-time type that read no lexical variable holds no variable";

/// Type the component `component` of type binders of `shape` in `Declaring` mode: each member's
/// declaration, closed or unknown. A member the binding binds is closed at its binding. A refused
/// declaration refuses where it is written.
pub fn declarations<'graph, 'o, C, X>(
    context: &C,
    tools: Tools<'_, '_, '_, '_, '_, '_, '_, X>,
    shape: &'graph BodyShape<'graph>,
    component: &Component<'graph>,
    each: &mut dyn FnMut(Slot, StaticType<'o>),
) -> Result<(), ShapeError<'o>>
where
    C: TypeContext<'graph, 'o> + ?Sized,
    X: Knotted,
{
    let reader = Reader::new(context, tools, shape, Mode::Declaring);
    let typed = type_declarations(component, &reader, tools.types, tools.scratch);
    let bound = |slot: Slot| {
        let unknown = Unknown {
            segment: shape.segment(),
            at: Address::Slot(slot),
        };
        match context.binding(unknown) {
            Some(Bound::Type(bound)) => Some(bound),
            _ => None,
        }
    };
    match typed {
        Ok(handles) => {
            for (slot, handle) in component.members.iter().zip(handles) {
                each(*slot, Static::Closed(bound(*slot).unwrap_or(*handle)));
            }
        }
        Err(Elaboration::Unknown { .. }) => {
            for slot in component.members {
                each(*slot, bound(*slot).map_or(Static::Unknown, Static::Closed));
            }
        }
        Err(error) => {
            let mut nodes = (component.members.iter()).filter_map(|slot| shape.declarations(*slot));
            let first = nodes.clone().next().expect("a type binder has its node");
            let at = nodes
                .find_map(|node| source_of(node, error.site()))
                .unwrap_or(first.source);
            return Err(ShapeError::Type { error, at });
        }
    }
    Ok(())
}

/// Type the recorded type expression `expression` of `shape` in `Typing` mode: `Unknown` where the
/// load cannot know it. A refusal is located at the part it names.
pub fn expression<'graph, 'o, C, X>(
    context: &C,
    tools: Tools<'_, '_, '_, '_, '_, '_, '_, X>,
    lay: impl Lays<'o>,
    shape: &'graph BodyShape<'graph>,
    expression: &TypeExpression<'graph>,
) -> Result<StaticType<'o>, ShapeError<'o>>
where
    C: TypeContext<'graph, 'o> + ?Sized,
    X: Knotted,
{
    let (types, scratch) = (tools.types, tools.scratch);
    let reader = Reader::new(context, tools, shape, Mode::Typing);
    let typed = match expression.guard {
        Some(_) => {
            let body = quoted_body(expression.part).expect("a guard is a quote");
            let top = Groups {
                names: &[],
                bounds: &[],
                outer: None,
            };
            let elaborator = Elaborator {
                reader: &reader,
                types,
                scratch,
                fellows: &[],
                locals: &[],
                binder: Cell::new(false),
            };
            elaborator.node(expression.site, body, &top)
        }
        None => type_expression(expression.part, &reader, types, scratch),
    };
    match typed {
        Ok(handle) => Ok(reader.fixed(lay, handle, |handle| types.concrete(handle).expect(FREE))),
        Err(Elaboration::Unknown { .. }) => Ok(Static::Unknown),
        Err(error) => {
            let statement = &shape.body()[expression.statement as usize];
            Err(ShapeError::Type {
                error,
                at: located(statement, error, expression.site),
            })
        }
    }
}

/// Refuse the first `MATCH … WITH` guard among `shape`'s `expressions`, in program order, that
/// `typed` gives the handle an earlier guard of its arm set has.
pub fn guards<'graph, 'o>(
    shape: &'graph BodyShape<'graph>,
    expressions: &[&TypeExpression<'graph>],
    typed: impl Fn(&TypeExpression<'graph>) -> StaticType<'o>,
) -> Result<(), ShapeError<'o>> {
    let handle = |typed: StaticType<'_>| match typed {
        Static::Closed(handle) => Some(Parametric::from(handle)),
        Static::Rigid { value: handle, .. } => Some(handle),
        Static::Unknown => None,
    };
    let guards =
        || (expressions.iter()).filter_map(|expression| Some((*expression, expression.guard?)));
    let repeat = guards()
        .filter_map(|(expression, (arms, index))| {
            let guard = handle(typed(expression))?;
            let (first, _) = guards().find(|(earlier, (held, written))| {
                *held == arms && *written < index && handle(typed(earlier)) == Some(guard)
            })?;
            Some((expression, first, guard))
        })
        .min_by_key(|(expression, ..)| expression.order);
    let Some((expression, first, guard)) = repeat else {
        return Ok(());
    };
    let body = shape.body();
    let source = |expression: &TypeExpression<'_>| {
        let statement = &body[expression.statement as usize];
        source_of(statement, expression.site).unwrap_or(statement.source)
    };
    Err(ShapeError::RepeatedGuard {
        guard,
        first: source(first),
        at: source(expression),
    })
}

/// Each `SIG` a block's surfaced heads name, by its declaring segment and slot, beside its handle
/// and its heads' shapes — `None` where it does not type — so a unit elaborates each once.
pub type Signatures<'p> = BumpVec<'p, ((Segment, Slot), Option<(KType, BumpVec<'p, HeadShape>)>)>;

/// The bucket entry of `registration`, one of `shape`'s: a definition's from its callable's type,
/// a surfaced head's from its signature or its module body. `Unknown` where the load cannot know
/// it.
pub fn registration<'graph, 'o, 'p, C, X>(
    context: &C,
    tools: Tools<'_, 'p, '_, '_, '_, '_, '_, X>,
    lay: impl Lays<'o>,
    shape: &'graph BodyShape<'graph>,
    registration: &Registration<'graph>,
    signatures: &mut Signatures<'p>,
) -> Result<StaticRegistered<'o>, ShapeError<'o>>
where
    C: TypeContext<'graph, 'o> + ?Sized,
    X: Knotted,
{
    let types = tools.types;
    if let Some(heads) = registration.surfaced {
        return Ok(
            surfaced(context, tools, lay, shape, registration, heads, signatures)
                .unwrap_or(Static::Unknown),
        );
    }
    let Some(form) = shape.births(registration.slot).and_then(BodyShape::form) else {
        return Ok(Static::Unknown);
    };
    let reader = Reader::new(context, tools, shape, Mode::Typing);
    match callable_type(form, &reader, types, tools.scratch, Some(registration)) {
        Ok(callable) => {
            let registered = callable
                .registered
                .expect("a callable born for a registration carries its bucket entry");
            let registered = laid_registered(lay, registered);
            Ok(reader.fixed(lay, registered, |registered| {
                close_registered(types, registered)
            }))
        }
        Err(Elaboration::Unknown { .. }) => Ok(Static::Unknown),
        Err(error) => Err(ShapeError::Type {
            error,
            at: source_of(form, error.site()).unwrap_or(form.source),
        }),
    }
}

/// The bucket entry of `registration`, a key of the block `block` its operand declares the heads
/// `heads` at: where it declares one, that head's shape, and otherwise `None`, which the load
/// leaves unknown.
fn surfaced<'graph, 'o, 'p, C, X>(
    context: &C,
    tools: Tools<'_, 'p, '_, '_, '_, '_, '_, X>,
    lay: impl Lays<'o>,
    block: &'graph BodyShape<'graph>,
    registration: &Registration<'graph>,
    heads: &[SurfacedHead<'graph>],
    signatures: &mut Signatures<'p>,
) -> Option<StaticRegistered<'o>>
where
    C: TypeContext<'graph, 'o> + ?Sized,
    X: Knotted,
{
    let shapes = context.shapes();
    match heads {
        [
            SurfacedHead::Signature {
                head,
                signature,
                ascription,
            },
        ] => signature_head(
            context,
            tools,
            lay,
            block,
            registration,
            (head, *signature, *ascription),
            signatures,
        ),
        // A body's definition is typed where the body is: its closed shape is the entry.
        [
            SurfacedHead::Body {
                module: (hops, slot),
                ..
            },
        ] => {
            let holder = shapes.up(block.segment(), *hops)?;
            let body = shapes.shape_at(holder).births(*slot)?;
            let mut at_key =
                (body.registrations().iter()).filter(|held| held.key == registration.key);
            let (Some(held), None) = (at_key.next(), at_key.next()) else {
                return None;
            };
            match context.registered(Binder {
                body: body.segment(),
                slot: held.slot,
            }) {
                Static::Closed(registered) => Some(Static::Closed(registered)),
                _ => None,
            }
        }
        _ => None,
    }
}

/// The bucket entry of `registration`, the surfaced signature head `head` of `block`, beside its
/// `SIG`'s binder and the ascription naming it, each where it lies: the shape the head's `SIG`
/// declares under the registration's key and ranking, each head parameter read at the
/// ascription's pin or, unpinned, at the block's own type parameter of that name. `None` where the
/// ascription is no closed signature or application of one, or the signature does not type where
/// it is declared. Reading the ascription gathers what it waits on, closed or not.
fn signature_head<'graph, 'o, 'p, C, X>(
    context: &C,
    tools: Tools<'_, 'p, '_, '_, '_, '_, '_, X>,
    lay: impl Lays<'o>,
    block: &'graph BodyShape<'graph>,
    registration: &Registration<'graph>,
    (head, (declared_hops, slot), (hops, site)): (&KExpression<'graph>, (u32, Slot), (u32, Site)),
    signatures: &mut Signatures<'p>,
) -> Option<StaticRegistered<'o>>
where
    C: TypeContext<'graph, 'o> + ?Sized,
    X: Knotted,
{
    let (shapes, types, scratch) = (context.shapes(), tools.types, tools.scratch);
    let ascribing = shapes.up(block.segment(), hops)?;
    let Static::Closed(ascribed) = context.expression(ascribing, site) else {
        return None;
    };
    let mut pins = BumpVec::new_in(scratch);
    let signature = match types.node(ascribed) {
        TypeNode::Signature { .. } => ascribed,
        TypeNode::SignatureApply {
            signature,
            pins: pinned,
        } => {
            pins.extend(pinned.iter());
            signature
        }
        _ => return None,
    };
    let declaring = (shapes.up(block.segment(), declared_hops)?, slot);
    // The `SIG` declaration's heads read what its binder reads.
    let _ = context.declared(Binder {
        body: declaring.0,
        slot,
    });
    let index = match signatures.iter().position(|(held, _)| *held == declaring) {
        Some(index) => index,
        None => {
            let declarer = shapes.shape_at(declaring.0);
            let node = declarer.declarations(slot)?;
            let reader = Reader::new(context, tools, declarer, Mode::Declaring);
            let heads = signature_heads(node, &reader, types, scratch).ok();
            signatures.push((declaring, heads));
            signatures.len() - 1
        }
    };
    let (declared, heads) = signatures[index].1.as_ref()?;
    debug_assert_eq!(
        *declared, signature,
        "an ascription names the signature its SIG declares"
    );
    let written = Site::of(&head.parts[0].value);
    let (_, _, shape) = heads
        .iter()
        .find(|(site, which, _)| *site == written && *which == registration.which)
        .expect("a signature's heads are keyed as its surfaced registrations are");
    let TypeNode::Signature { schema, .. } = types.node(signature) else {
        unreachable!("an application applies a signature");
    };
    let reader = Reader::new(context, tools, block, Mode::Typing);
    head_entry(&reader, lay, registration, *shape, &schema, &pins)
}

/// The bucket entry of `registration`, a surfaced signature head whose `SIG` declares `shape`
/// over `schema`'s head parameters, read through `reader` at the block: each head parameter at
/// its pin among `pins` or, unpinned, at the block's own type parameter of that name.
fn head_entry<'graph, 'o, C, X>(
    reader: &Reader<'_, '_, 'graph, 'o, '_, '_, '_, '_, '_, C, X>,
    lay: impl Lays<'o>,
    registration: &Registration<'graph>,
    shape: DeclaredType<Parametric>,
    schema: &SigSchema,
    pins: &[(BinderSymbol, KType)],
) -> Option<StaticRegistered<'o>>
where
    C: TypeContext<'graph, 'o> + ?Sized,
    X: Knotted,
{
    let (types, scratch) = (reader.tools.types, reader.tools.scratch);
    let block = reader.shape;
    let mut bindings = BumpVec::with_capacity_in(schema.parameters.len(), scratch);
    for (name, _) in schema.parameters.iter() {
        let pinned = pins
            .iter()
            .find(|(pin, _)| *pin == BinderSymbol::Type(*name));
        let bound: Parametric = match pinned {
            Some((_, pin)) => (*pin).into(),
            None => {
                let (slot, _) = block.slot(BinderSymbol::Type(*name))?;
                let local = Coordinate::Activation {
                    hops: 0,
                    target: Target::Local(slot),
                };
                match reader.type_at(local) {
                    TypeAt::Type(handle) => handle.into(),
                    TypeAt::Rigid(handle) => handle,
                    TypeAt::NotAType | TypeAt::Unknown => return None,
                }
            }
        };
        bindings.push((*name, bound));
    }
    let read = substitute_parameters(types, scratch, shape, Members::from_table(bindings));
    let registered = Registered {
        shape: ranked(types, scratch, read, registration.classes),
        quantifier_map: ShapeGroupMap::default(),
        parameters: ParameterBinding::Named(&[]),
    };
    Some(reader.fixed(lay, registered, |registered| {
        close_registered(types, registered)
    }))
}

/// The expression shape `shape` ranked by `classes`.
fn ranked(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    shape: DeclaredType<Parametric>,
    classes: &[u16],
) -> DeclaredType<Parametric> {
    let view = shape.view(types);
    let TypeNode::ExpressionShape { elements, ret, .. } = view.body else {
        unreachable!("a head declares an expression shape");
    };
    let mut written = BumpVec::with_capacity_in(elements.len(), scratch);
    written.extend(elements.iter());
    types
        .shape_scheme(
            scratch,
            view.group.names,
            view.group.bounds,
            &written,
            classes,
            ret,
        )
        .handle
}

/// The type of the callable `body`, read through its encloser's names: `Unknown` where the load
/// cannot know it.
pub fn callable<'graph, 'o, C, X>(
    context: &C,
    tools: Tools<'_, '_, '_, '_, '_, '_, '_, X>,
    lay: impl Lays<'o>,
    encloser: &'graph BodyShape<'graph>,
    body: &'graph BodyShape<'graph>,
) -> Result<StaticCallable<'o>, ShapeError<'o>>
where
    C: TypeContext<'graph, 'o> + ?Sized,
    X: Knotted,
{
    let types = tools.types;
    let form = body.form().expect("a callable body sits in its form");
    let reader = Reader::new(context, tools, encloser, Mode::Typing);
    match callable_type(form, &reader, types, tools.scratch, None) {
        Ok(callable) => {
            let callable = laid_callable(lay, callable);
            Ok(reader.fixed(lay, callable, |callable| close_callable(types, callable)))
        }
        Err(Elaboration::Unknown { .. }) => Ok(Static::Unknown),
        Err(error) => Err(ShapeError::Type {
            error,
            at: source_of(form, error.site()).unwrap_or(form.source),
        }),
    }
}

/// The callable `body`'s own group as its body reads it, read off its type `typed`, by group
/// order: the group's variable `i` is the lexical variable of the name it is, at that name's
/// shape level — or, where the binding binds it, the type it binds. Empty where the callable
/// writes no `FOR ALL` group or the load did not type it.
pub fn own_levels<'graph, 'o, C, X>(
    context: &C,
    tools: Tools<'_, '_, '_, '_, '_, '_, '_, X>,
    lay: impl Lays<'o>,
    body: &'graph BodyShape<'graph>,
    typed: StaticCallable<'o>,
) -> &'o [Parametric]
where
    C: TypeContext<'graph, 'o> + ?Sized,
    X: Knotted,
{
    let types = tools.types;
    let (map, ktype): (FunctionGroupMap<'_>, DeclaredType<Parametric>) = match typed {
        Static::Closed(callable) => (callable.quantifier_map, callable.ktype.into()),
        Static::Rigid { value, .. } => (value.quantifier_map, value.ktype),
        Static::Unknown => return &[],
    };
    let form = body.form().expect("a callable body sits in its form");
    if !writes_for_all(form) {
        return &[];
    }
    let bounds = match ktype {
        DeclaredType::Scheme(scheme) => scheme.group(types).bounds,
        // An empty group binds nothing.
        DeclaredType::Type(_) => &[],
    };
    let mut levels = BumpVec::with_capacity_in(bounds.len(), tools.scratch);
    levels.extend(bounds.iter().enumerate().map(|(index, bound)| {
        let (name, _) = map
            .iter()
            .find(|(_, at)| *at == index)
            .expect("each variable of the group is a name the declaration wrote");
        let (slot, _) = body
            .slot(BinderSymbol::Type(name))
            .expect("a `FOR ALL` name has its slot");
        let address = Address::Slot(slot);
        let unknown = Unknown {
            segment: body.segment(),
            at: address,
        };
        match context.binding(unknown) {
            Some(Bound::Type(closed)) => closed.into(),
            _ => {
                let level = (context.shapes().level(body.segment(), address))
                    .expect("a `FOR ALL` name has its level");
                types.lexical(level, name, *bound)
            }
        }
    }));
    collect(lay, levels.iter().copied())
}

/// A callable's type with its slices laid through `lay`.
fn laid_callable<'o, T: Copy>(lay: impl Lays<'o>, callable: Callable<'_, T>) -> Callable<'o, T> {
    Callable {
        ktype: callable.ktype,
        quantifier_map: FunctionGroupMap(collect(lay, callable.quantifier_map.iter())),
        registered: (callable.registered).map(|registered| laid_registered(lay, registered)),
    }
}

/// A bucket entry with its slices laid through `lay`.
fn laid_registered<'o, T: Copy>(
    lay: impl Lays<'o>,
    registered: Registered<'_, T>,
) -> Registered<'o, T> {
    Registered {
        shape: registered.shape,
        quantifier_map: ShapeGroupMap(collect(lay, registered.quantifier_map.iter())),
        parameters: match registered.parameters {
            ParameterBinding::Named(names) => {
                ParameterBinding::Named(collect(lay, names.iter().copied()))
            }
            ParameterBinding::Operands => ParameterBinding::Operands,
        },
    }
}

/// A callable and its bucket entry that read no lexical variable, each declared type narrowed to
/// concrete and a scheme kept as it is.
fn close_callable<'o>(
    types: &Chain<'_, '_, '_, '_>,
    callable: Callable<'o>,
) -> Callable<'o, KType> {
    Callable {
        ktype: types.concrete_declared(callable.ktype).expect(FREE),
        quantifier_map: callable.quantifier_map,
        registered: callable
            .registered
            .map(|registered| close_registered(types, registered)),
    }
}

/// A bucket entry that read no lexical variable, its shape narrowed as [`close_callable`] narrows
/// a callable's type.
fn close_registered<'o>(
    types: &Chain<'_, '_, '_, '_>,
    registered: Registered<'o>,
) -> Registered<'o, KType> {
    Registered {
        shape: types.concrete_declared(registered.shape).expect(FREE),
        quantifier_map: registered.quantifier_map,
        parameters: registered.parameters,
    }
}

/// Where a refusal about the type expression at `site`, written in `statement`, is located: at the
/// part the refusal names, else at the expression.
fn located(statement: &KExpression<'_>, error: Elaboration, site: Site) -> SourceRef {
    source_of(statement, error.site())
        .or_else(|| source_of(statement, site))
        .unwrap_or(statement.source)
}

/// Whether a callable's declaration writes a `FOR ALL` group — whose names its body reads as
/// lexical variables of its own.
fn writes_for_all(declaration: &KExpression<'_>) -> bool {
    declaration
        .cache()
        .builtin_shape()
        .is_some_and(|shape| shape.roles().any(|role| role == Role::Quantifiers))
}
