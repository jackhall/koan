//! The type half of the one walk: the type channel's rules, run unit by unit over a
//! [`TypeContext`] the walk implements ([the one walk](../build/README.md#the-one-walk)).
//!
//! Each entry point types one fact — a component of type binders, a recorded type expression, a
//! registration, a callable's type, a callable body's own group — through the one load-time
//! reader, [`Reader`], and hands it back; every unknown it read is left in the context's
//! gathering ([`TypeContext::note`]). The elaborator's doors do the typing: nothing here mints a
//! level or a path. A run-bound name's level is the shape's ([`Shapes::level`]), and a path is read
//! through the resolution the walk fixed ([`TypeContext::resolution`]); one it has not fixed yet
//! stops the entry point with a request ([`Stop::Unresolved`]), which the walk resolves before
//! typing again.
//!
//! The reader answers a coordinate with a builtin's value; else with the closed type the binding
//! gives it; else, for a type binder, with its declaration: closed or rigid as it is, each variable
//! a rigid declaration reads read again where the binder is, and otherwise the binder is an unknown
//! read as a lexical variable; else, for a type name a `USING … SCOPE` surfaces, with the type its
//! module's static signature declares it at exactly ([`TypeContext::surfaced`]); else, for any
//! other run-bound type name, with its lexical variable at its shape level. A path reads its resolution: closed as it is, and a class as its class's
//! path variable. A type binder reads alike, so it is closed or rigid wherever it reads a name a
//! run binds. The walk's namer is total, so it leaves a type unknown only past its chain, where no
//! binding is written in ([`Unwalked`](crate::scope::Unwalked)), or at a `FOR ALL` bound over a
//! run-bound name; both hold.
//!
//! A fact's values are laid through the walk's `lay`, at the records' lifetime; every other value
//! rests in the scratch the rules are handed.

use std::cell::Cell;

use crate::memory::{BumpAllocator, BumpVec, Lays, collect};
use crate::parse::quoted_body;
use crate::parse::{KExpression, Role};
use crate::scope::{
    Address, AsWritten, BodyShape, Builtins, Callable, CaptureSlot, CaptureSource, Component,
    Coordinate, Elaboration, FunctionGroupMap, ParameterBinding, PathName, Question, Registered,
    Registration, Repeated, Resolved, Segment, ShapeError, ShapeGroupMap, ShapeKind, Shapes, Site,
    Slot, Static, StaticCallable, StaticRegistered, StaticType, SurfacedHead, Target,
    TypeExpression, Unknown, Unresolved, Variable, source_of,
};
use crate::source::SourceRef;
use crate::symbols::{BinderSymbol, TypeSymbol, ValueSymbol};
use crate::type_lattice::{
    DeclaredType, KType, Members, Parametric, SigSchema, TypeNode, substitute_parameters,
};
use crate::values::{Chain, Knotted, Value};

use super::declaration::{HeadShape, signature_heads, type_declarations};
use super::expression::{Elaborator, Fellows, Groups, type_expression};
use super::judge::{Derives, Marks};
use super::reads::{Reads, TypeAt};
use super::signature::callable_type;
use std::convert::Infallible;

/// What the type half reads of the walk: the shape table, the binding, and every fact the walk has
/// fixed so far. Each read of a fact gathers what the fact waits on.
pub trait TypeContext<'graph, 'o> {
    fn shapes(&self) -> &'graph Shapes<'graph>;
    /// The declaration of the type binder at `slot` of `body` as far as the walk has it.
    fn declared(&self, body: Segment, slot: Slot) -> StaticType<'o>;
    /// The entry of the registration at `slot` of `body` as far as the walk has it.
    fn registered(&self, body: Segment, slot: Slot) -> StaticRegistered<'o>;
    /// A callable body's type as far as the walk has it.
    fn callable(&self, body: Segment) -> StaticCallable<'o>;
    /// The type a `USING … SCOPE` body `body`'s surfaced type name at `slot` is, where its module's
    /// static type declares the member exactly: a manifest member, or a head parameter it pins,
    /// closed.
    fn surfaced(&self, body: Segment, slot: Slot) -> Option<KType>;
    /// A callable body's own `FOR ALL` group as its body reads it, by group order: empty where the
    /// walk has not typed it.
    fn group_levels(&self, body: Segment) -> &'o [Parametric];
    /// A recorded type expression's type, for a surfaced head reading its ascription.
    fn expression(&self, segment: Segment, site: Site) -> StaticType<'o>;
    /// What the path at `level` of the chain `root` roots resolves to, where the walk resolved it:
    /// `None` where it has not yet, which the reader asks it to.
    fn resolution(&self, root: Segment, level: usize) -> Option<Resolved>;
    /// Gather `unknowns`: each the binding leaves unbound, of a shape on the reader's chain.
    fn note(&self, unknowns: &[Unknown]);
    /// Where a judgement door marks, and what names its derived names: a walk, which names every
    /// term.
    fn marks(&self) -> &dyn Derives<Unwalked = Infallible>;
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

/// What a coordinate holds, as far as the load can tell.
enum Class<'o> {
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
    /// A type binder declared rigid: its declaration, over `variables` as its own segment reads
    /// them, `hops` shapes out from the reader.
    Rigid {
        value: Parametric,
        variables: &'o [Variable],
        hops: usize,
    },
}

/// The load-time reader: reads the names mentioned in `shape` through the walk's context.
pub struct Reader<'e, 'p, 'graph, 'o, 'cell, 'tier, 'pair, 'reg, 'door, C: ?Sized, X> {
    context: &'e C,
    tools: Tools<'e, 'p, 'cell, 'tier, 'pair, 'reg, 'door, X>,
    shape: &'graph BodyShape<'graph>,
    rigid: Cell<u32>,
    /// Each rigid variable read, once per level.
    variables: Cell<BumpVec<'p, Variable>>,
    /// Each unknown it gathered: open where what it types is left unknown.
    noted: Cell<BumpVec<'p, Unknown>>,
    records: std::marker::PhantomData<&'o ()>,
}

impl<'e, 'p, 'graph, 'o, 'cell, 'tier, 'pair, 'reg, 'door, C, X>
    Reader<'e, 'p, 'graph, 'o, 'cell, 'tier, 'pair, 'reg, 'door, C, X>
where
    C: TypeContext<'graph, 'o> + ?Sized,
    X: Knotted,
{
    /// A reader of the names `shape` mentions.
    pub fn new(
        context: &'e C,
        tools: Tools<'e, 'p, 'cell, 'tier, 'pair, 'reg, 'door, X>,
        shape: &'graph BodyShape<'graph>,
    ) -> Self {
        Reader {
            context,
            tools,
            shape,
            rigid: Cell::new(0),
            variables: Cell::new(BumpVec::new_in(tools.scratch)),
            noted: Cell::new(BumpVec::new_in(tools.scratch)),
            records: std::marker::PhantomData,
        }
    }

    fn shapes(&self) -> &'graph Shapes<'graph> {
        self.context.shapes()
    }

    /// What `at`, read in `segment`, holds, each unknown the read reaches gathered.
    fn classify(&self, segment: Segment, at: Coordinate) -> Class<'o> {
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
    fn slot_at(&self, segment: Segment, slot: Slot) -> Class<'o> {
        let shape = self.shapes().shape_at(segment);
        let address = Address::Slot(slot);
        let unknown = Unknown {
            segment,
            at: address,
        };
        if shape.declarations(slot).is_some() {
            return match self.context.declared(segment, slot) {
                Static::Closed(handle) => Class::Type(handle),
                // A rigid binder is the names its declaration reads, so its reader waits on each,
                // and nothing binds the binder itself.
                Static::Rigid { value, variables } => {
                    for variable in variables {
                        if let Variable::Type { level, .. } = variable
                            && let Some(unknown) = self.shapes().unknown_of_level(segment, *level)
                        {
                            self.note(unknown);
                        }
                    }
                    Class::Rigid {
                        value,
                        variables,
                        hops: self.hops_to(segment),
                    }
                }
                Static::Unknown => {
                    self.note(unknown);
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
        // A surfaced name its module's signature declares exactly is that type at every binding.
        if let Some(exactly) = self.context.surfaced(segment, slot) {
            return Class::Type(exactly);
        }
        self.note(unknown);
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
    /// born — the very type name, and so its level — and otherwise, a quote's code's capture of a
    /// value name or one its `EVAL` or its entry binds, a run-bound name of the code's own.
    fn capture_at(&self, segment: Segment, capture: CaptureSlot) -> Class<'o> {
        let shape = self.shapes().shape_at(segment);
        let address = Address::Capture(capture);
        let typed = matches!(
            shape.captures()[capture.index()].name,
            BinderSymbol::Type(_)
        );
        let root = shape.kind() == ShapeKind::Code
            && (!typed || self.shapes().level(segment, address).is_some());
        if root {
            // A name the code binds itself is one of the code's own.
            self.note(Unknown {
                segment,
                at: address,
            });
        }
        let parent = self.shapes().parent(segment).map(|(parent, _)| parent);
        match (shape.captures()[capture.index()].source, parent) {
            (CaptureSource::Read(inner), Some(parent)) => match self.classify(parent, inner) {
                read if !root => read,
                Class::Type(handle) => Class::Type(handle),
                Class::NotAType => Class::NotAType,
                Class::Rigid { .. } => Class::RunBound {
                    segment,
                    address,
                    bound: KType::ANY,
                },
                Class::RunBound { bound, .. } => Class::RunBound {
                    segment,
                    address,
                    bound,
                },
                Class::Variable(variable) => Class::RunBound {
                    segment,
                    address,
                    bound: (super::judge::own_variable(self.tools.types, variable).rigid_bound())
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

    /// How many shapes out from the reader's `segment` lies: an enclosing shape of its chain.
    fn hops_to(&self, segment: Segment) -> usize {
        let mut at = self.shape.segment();
        let mut hops = 0;
        while at != segment {
            at = (self.shapes().parent(at))
                .expect("a binder the reader reaches encloses it")
                .0;
            hops += 1;
        }
        hops
    }

    /// The lexical variable a run-bound name of `segment` held at `address` is, bounded by
    /// `bound`, at its shape level.
    fn variable(&self, segment: Segment, address: Address, bound: KType) -> Parametric {
        let shape = self.shapes().shape_at(segment);
        let name = match address {
            Address::Slot(slot) => shape.slot_name(slot),
            Address::Capture(capture) => shape.captures()[capture.index()].name,
            Address::Held(_) => unreachable!("a run-bound type name has a binder"),
        };
        let BinderSymbol::Type(name) = name else {
            unreachable!("a run-bound name is a type name");
        };
        let level =
            (self.shapes().level(segment, address)).expect("a run-bound type name has its level");
        self.tools.types.lexical(level, name, bound)
    }

    /// The lexical variable `variable` of a run-bound name, read at `at`.
    fn rigid(&self, variable: Parametric, at: Coordinate) -> TypeAt {
        let TypeNode::Lexical { level, .. } =
            super::judge::own_variable(self.tools.types, variable)
        else {
            unreachable!("a run-bound name reads as a lexical variable");
        };
        self.read(Variable::Type { level, at });
        TypeAt::Rigid(variable)
    }

    /// Gather `unknown`, named until what this reader types is left unknown over it.
    fn note(&self, unknown: Unknown) {
        self.context.note(&[unknown]);
        let mut noted = self.noted.replace(BumpVec::new_in(self.tools.scratch));
        noted.push(unknown);
        self.noted.set(noted);
    }

    /// Record a rigid read of `variable`, once per variable.
    fn read(&self, variable: Variable) {
        self.rigid.set(self.rigid.get() + 1);
        let mut variables = self.variables.replace(BumpVec::new_in(self.tools.scratch));
        if !variables.iter().any(|held| held.names_alike(&variable)) {
            variables.push(variable);
        }
        self.variables.set(variables);
    }

    /// The path `self.shape` spells at `site`, read through its resolution: `root` is where the
    /// shape reads its root, `None` for one written in place. Unknown past the chain; a request
    /// where the walk has not resolved it yet.
    fn path(
        &self,
        site: Site,
        at: Option<Coordinate>,
        label: TypeSymbol,
    ) -> Result<TypeAt, Unresolved> {
        let shapes = self.shapes();
        let segment = self.shape.segment();
        let index =
            (self.shape.spelled_at(site)).expect("the builder records every path a type spells");
        let level = match shapes.path_level(segment, index) {
            Ok(level) => level,
            Err(unwalked) => return Ok(TypeAt::Unknown(unwalked)),
        };
        let root = shapes.chain_root(segment);
        let unresolved = Unresolved {
            segment,
            index: index as u32,
        };
        Ok(
            match self.context.resolution(root, level).ok_or(unresolved)? {
                Resolved::Closed(closed) => TypeAt::Type(closed),
                // A pin over run-bound names: rigid over them, which a binding closes by level.
                Resolved::Over(over) => {
                    self.rigid.set(self.rigid.get() + 1);
                    TypeAt::Rigid(over)
                }
                Resolved::Class(class) => {
                    self.read(Variable::Path {
                        class,
                        at,
                        root,
                        entry: PathName::Spelled(level),
                    });
                    TypeAt::Rigid(self.tools.types.path_variable(class, label, KType::ANY))
                }
            },
        )
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
    type Unwalked = Infallible;

    fn shape(&self) -> &'graph BodyShape<'graph> {
        self.shape
    }

    fn written_bound(
        &self,
        variable: Parametric,
        types: &Chain<'_, '_, '_, '_>,
    ) -> Option<AsWritten> {
        let level = super::judge::lexical_level(types, variable)?;
        self.shapes().written_bound(self.shape.segment(), level)
    }

    fn type_at(&self, at: Coordinate) -> TypeAt {
        match self.classify(self.shape.segment(), at) {
            Class::Type(handle) => TypeAt::Type(handle),
            Class::NotAType => TypeAt::NotAType,
            Class::Variable(variable) => self.rigid(variable, at),
            Class::Rigid {
                value,
                variables,
                hops,
            } => {
                let out = |at: Coordinate| (0..hops).fold(at, |at, _| at.through_block());
                for variable in variables {
                    self.read(match *variable {
                        Variable::Type { level, at } => Variable::Type { level, at: out(at) },
                        Variable::Path {
                            class,
                            at,
                            root,
                            entry,
                        } => Variable::Path {
                            class,
                            at: at.map(out),
                            root,
                            entry,
                        },
                    });
                }
                TypeAt::Rigid(value)
            }
            Class::RunBound {
                segment,
                address,
                bound,
            } => self.rigid(self.variable(segment, address, bound), at),
        }
    }

    fn rigid_reads(&self) -> u32 {
        self.rigid.get()
    }

    fn marks(&self) -> &dyn Derives<Unwalked = Infallible> {
        self.context.marks()
    }

    /// A path through the binder `at` lands at, read through its resolution.
    fn member_type(
        &self,
        at: Coordinate,
        site: Site,
        _chain: &[ValueSymbol],
        label: TypeSymbol,
        _types: &Chain<'_, '_, '_, '_>,
    ) -> Result<Option<(TypeAt, KType)>, Unresolved> {
        Ok(Some((
            self.path(site, Some(at), label)?,
            KType::EMPTY_SIGNATURE,
        )))
    }

    /// A path through the expression at `root`, written in place, read through its resolution.
    fn inline_member(
        &self,
        site: Site,
        _root: Site,
        _chain: &[ValueSymbol],
        label: TypeSymbol,
    ) -> Result<TypeAt, Unresolved> {
        self.path(site, None, label)
    }
}

/// Why a load-time type that read no lexical variable is concrete: nothing it was elaborated under
/// declares a `FOR ALL` group or a head parameter it could name, and every binder door binds its
/// own group's variables, so none escapes free.
const FREE: &str = "a load-time type that read no lexical variable holds no variable";

/// Where typing one of a unit's types stops short: a refusal, or a path the walk had not resolved
/// when a reader met it, which the walk resolves before typing again.
pub enum Stop<'o> {
    Refused(ShapeError<'o>),
    Unresolved(Unresolved),
}

/// `error`, a refusal located at `at`, or the path it asks the walk to resolve.
fn refused<'o>(error: Elaboration, at: SourceRef) -> Stop<'o> {
    match error {
        Elaboration::Unresolved { path, .. } => Stop::Unresolved(path),
        error => Stop::Refused(ShapeError::Type { error, at }),
    }
}

/// Type the component `component` of type binders of `shape`: each member's declaration, closed,
/// rigid over the run-bound names it reads, laid through `lay`, or unknown. A refused declaration
/// refuses where it is written.
pub fn declarations<'graph, 'o, C, X>(
    context: &C,
    tools: Tools<'_, '_, '_, '_, '_, '_, '_, X>,
    lay: impl Lays<'o>,
    shape: &'graph BodyShape<'graph>,
    component: &Component<'graph>,
    each: &mut dyn FnMut(Slot, StaticType<'o>),
) -> Result<(), Stop<'o>>
where
    C: TypeContext<'graph, 'o> + ?Sized,
    X: Knotted,
{
    let reader = Reader::new(context, tools, shape);
    let typed = type_declarations(component, &reader, tools.types, tools.scratch);
    match typed {
        Ok(handles) => {
            let types = tools.types;
            for (slot, handle) in component.members.iter().zip(handles) {
                each(
                    *slot,
                    reader.fixed(lay, *handle, |handle| {
                        super::judge::static_of(types, handle).expect(FREE)
                    }),
                );
            }
        }
        // A `FOR ALL` bound inside a type over a run-bound name is refused below: each binding
        // would make the type a different one, and no type written over names holds such a scheme.
        Err(Elaboration::Unknown { .. }) => {
            for slot in component.members {
                each(*slot, Static::Unknown);
            }
        }
        Err(error) => {
            let mut nodes = (component.members.iter()).filter_map(|slot| shape.declarations(*slot));
            let first = nodes.clone().next().expect("a type binder has its node");
            let at = nodes
                .find_map(|node| source_of(shape, node, error.site()))
                .unwrap_or(first.source);
            return Err(refused(error, at));
        }
    }
    Ok(())
}

/// Type the recorded type expression `expression` of `shape`: `Unknown` where the
/// load cannot know it. A refusal is located at the part it names.
pub fn expression<'graph, 'o, C, X>(
    context: &C,
    tools: Tools<'_, '_, '_, '_, '_, '_, '_, X>,
    lay: impl Lays<'o>,
    shape: &'graph BodyShape<'graph>,
    expression: &TypeExpression<'graph>,
) -> Result<StaticType<'o>, Stop<'o>>
where
    C: TypeContext<'graph, 'o> + ?Sized,
    X: Knotted,
{
    let (types, scratch) = (tools.types, tools.scratch);
    let reader = Reader::new(context, tools, shape);
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
                fellows: Fellows::NONE,
                locals: &[],
                binder: Cell::new(false),
            };
            elaborator.node(expression.site, body, &top)
        }
        None => type_expression(expression.part, &reader, types, scratch),
    };
    match typed {
        Ok(handle) => Ok(reader.fixed(lay, handle, |handle| {
            super::judge::static_of(types, handle).expect(FREE)
        })),
        Err(Elaboration::Unknown { .. }) => Ok(Static::Unknown),
        Err(error) => {
            let statement = &shape.body()[expression.statement as usize];
            Err(refused(
                error,
                located(shape, statement, error, expression.site),
            ))
        }
    }
}

/// Refuse the first `MATCH … WITH` guard among `shape`'s `expressions`, in program order, that
/// `typed` gives the handle an earlier guard of its arm set has: each guard typed once. Else each
/// pair a binding may make alike, in the order that refusal reads them: whether the two are one
/// type, beside the refusal naming the later.
pub fn guards<'graph, 'o, 'x>(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    shape: &'graph BodyShape<'graph>,
    expressions: &[&TypeExpression<'graph>],
    scratch: BumpAllocator<'x>,
    typed: impl Fn(&TypeExpression<'graph>) -> StaticType<'o>,
) -> Result<Repeats<'x, 'o>, ShapeError<'o>> {
    let handle = |typed: StaticType<'_>| match typed {
        Static::Closed(handle) => Some(Parametric::from(handle)),
        Static::Rigid { value: handle, .. } => Some(handle),
        Static::Unknown => None,
    };
    // Each guard the load types, sorted so the guards of one arm set and handle run in written
    // order. A guard the load cannot type never matched an earlier one.
    let mut known = BumpVec::new_in(scratch);
    for expression in expressions.iter() {
        if let Some((arms, written)) = expression.guard
            && let Some(guard) = handle(typed(expression))
        {
            known.push((arms, guard, written, *expression));
        }
    }
    known.sort_unstable_by_key(|(arms, guard, written, _)| (*arms, *guard, *written));
    let body = shape.body();
    let source = |expression: &TypeExpression<'_>| {
        let statement = &body[expression.statement as usize];
        source_of(shape, statement, expression.site).unwrap_or(statement.source)
    };
    // A binding may make two guards of one arm set alike, or tell two apart: each pair is one
    // where `same` says so, and the guard written later repeats the one written first. The
    // repeat written first refuses, beside the first guard it repeats.
    let mut repeats = BumpVec::new_in(scratch);
    for set in known.chunk_by(|(held, ..), (arms, ..)| held == arms) {
        for (at, (_, one, written, expression)) in set.iter().enumerate() {
            for (_, other, again, repeated) in set[at + 1..].iter() {
                let same = super::judge::same_decided(marks, types, scratch, *one, *other);
                if same == Some(false) {
                    continue;
                }
                let (expression, first, guard, first_written) = match written <= again {
                    true => (*repeated, *expression, *other, *written),
                    false => (*expression, *repeated, *one, *again),
                };
                let refusal = ShapeError::RepeatedGuard {
                    guard: Repeated::Type(guard),
                    first: source(first),
                    at: source(expression),
                };
                let asked = Question::Same {
                    a: (*one).into(),
                    b: (*other).into(),
                };
                let key = (expression.site, first_written);
                repeats.push((same == Some(true), key, (asked, refusal)));
            }
        }
    }
    repeats.sort_unstable_by_key(|(_, key, _)| *key);
    if let Some((_, _, (_, refusal))) = repeats.iter().find(|(same, ..)| *same) {
        return Err(*refusal);
    }
    let mut asked = BumpVec::with_capacity_in(repeats.len(), scratch);
    asked.extend(repeats.iter().map(|(_, _, asked)| *asked));
    Ok(asked)
}

/// What a binding may refuse among a unit's `MATCH … WITH` guards ([`guards`]): each pair's
/// question whether the two are one type, beside the refusal naming the later, first refusing
/// first.
pub type Repeats<'x, 'o> = BumpVec<'x, (Question<'x>, ShapeError<'o>)>;

/// Each `SIG` a block's surfaced heads name, by its declaring segment and slot, beside its handle
/// and its heads' shapes — `None` where it does not type — so a unit elaborates each once.
pub type Signatures<'p> = BumpVec<
    'p,
    (
        (Segment, Slot),
        Option<(Parametric, BumpVec<'p, HeadShape>)>,
    ),
>;

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
) -> Result<StaticRegistered<'o>, Stop<'o>>
where
    C: TypeContext<'graph, 'o> + ?Sized,
    X: Knotted,
{
    let types = tools.types;
    if let Some(heads) = registration.surfaced {
        return Ok(
            surfaced(context, tools, lay, shape, registration, heads, signatures)
                .map_err(Stop::Unresolved)?
                .unwrap_or(Static::Unknown),
        );
    }
    let Some(form) = shape.births(registration.slot).and_then(BodyShape::form) else {
        return Ok(Static::Unknown);
    };
    let reader = Reader::new(context, tools, shape);
    match callable_type(form, &reader, types, tools.scratch, Some(registration)) {
        // A shape beside bounds over an encloser's names reads them, so it is rigid over them.
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
        Err(error) => Err(refused(
            error,
            source_of(shape, form, error.site()).unwrap_or(form.source),
        )),
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
) -> Result<Option<StaticRegistered<'o>>, Unresolved>
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
            let Some(holder) = shapes.up(block.segment(), *hops) else {
                return Ok(None);
            };
            let Some(body) = shapes.shape_at(holder).births(*slot) else {
                return Ok(None);
            };
            let mut at_key =
                (body.registrations().iter()).filter(|held| held.key == registration.key);
            let (Some(held), None) = (at_key.next(), at_key.next()) else {
                return Ok(None);
            };
            // The definition's entry, closed or written over names, which a binding closes alike.
            Ok(match context.registered(body.segment(), held.slot) {
                Static::Unknown => None,
                entry => Some(entry),
            })
        }
        _ => Ok(None),
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
) -> Result<Option<StaticRegistered<'o>>, Unresolved>
where
    C: TypeContext<'graph, 'o> + ?Sized,
    X: Knotted,
{
    let (shapes, types, scratch) = (context.shapes(), tools.types, tools.scratch);
    let Some(ascribing) = shapes.up(block.segment(), hops) else {
        return Ok(None);
    };
    // An ascription over run-bound names is an application whose pins are over them: the entry
    // is rigid over what the ascription reads, which a binding closes alike.
    let (ascribed, read): (Parametric, &[Variable]) = match context.expression(ascribing, site) {
        Static::Closed(ascribed) => (ascribed.into(), &[]),
        Static::Rigid { value, variables } => (value, variables),
        Static::Unknown => return Ok(None),
    };
    let mut pins = BumpVec::new_in(scratch);
    let signature = match super::judge::node_unless_variable(types, ascribed) {
        Some(TypeNode::Signature { .. }) => match super::judge::static_of(types, ascribed) {
            Some(signature) => signature,
            None => return Ok(None),
        },
        Some(TypeNode::SignatureApply {
            signature,
            pins: pinned,
        }) => {
            pins.extend(pinned.iter());
            signature
        }
        _ => return Ok(None),
    };
    let Some(declaring) = shapes.up(block.segment(), declared_hops) else {
        return Ok(None);
    };
    let declaring = (declaring, slot);
    // The `SIG` declaration's heads read what its binder reads.
    let _ = context.declared(declaring.0, slot);
    let index = match signatures.iter().position(|(held, _)| *held == declaring) {
        Some(index) => index,
        None => {
            let declarer = shapes.shape_at(declaring.0);
            let Some(node) = declarer.declarations(slot) else {
                return Ok(None);
            };
            let reader = Reader::new(context, tools, declarer);
            // A path the walk has not resolved is asked for, never cached as no heads.
            let heads = match signature_heads(node, &reader, types, scratch) {
                Ok(heads) => Some(heads),
                Err(Elaboration::Unresolved { path, .. }) => return Err(path),
                Err(_) => None,
            };
            signatures.push((declaring, heads));
            signatures.len() - 1
        }
    };
    let Some((declared, heads)) = signatures[index].1.as_ref() else {
        return Ok(None);
    };
    debug_assert_eq!(
        *declared,
        Parametric::from(signature),
        "an ascription names the signature its SIG declares"
    );
    // A head's site is its declarer's, where the `SIG` writes it.
    let written = shapes.shape_at(declaring.0).site(&head.parts[0].value);
    let (_, _, shape) = heads
        .iter()
        .find(|(site, which, _)| *site == written && *which == registration.which)
        .expect("a signature's heads are keyed as its surfaced registrations are");
    let TypeNode::Signature { schema, .. } = super::judge::closed_type_node(types, signature)
    else {
        unreachable!("an application applies a signature");
    };
    let reader = Reader::new(context, tools, block);
    // The ascription's reads, located where it is written, read from the block below it.
    let out = |at: Coordinate| (0..hops).fold(at, |at, _| at.through_block());
    for variable in read {
        reader.read(match *variable {
            Variable::Type { level, at } => Variable::Type { level, at: out(at) },
            Variable::Path {
                class,
                at,
                root,
                entry,
            } => Variable::Path {
                class,
                at: at.map(out),
                root,
                entry,
            },
        });
    }
    Ok(head_entry(
        &reader,
        lay,
        registration,
        *shape,
        &schema,
        &pins,
    ))
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
    pins: &[(BinderSymbol, Parametric)],
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
            Some((_, pin)) => *pin,
            None => {
                let (slot, _) = block.slot(BinderSymbol::Type(*name))?;
                let local = Coordinate::Activation {
                    hops: 0,
                    target: Target::Local(slot),
                };
                match reader.type_at(local) {
                    TypeAt::Type(handle) => handle.into(),
                    TypeAt::Rigid(handle) => handle,
                    TypeAt::NotAType | TypeAt::Unknown(_) => return None,
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
        rebounds: crate::scope::Rebounds::NONE,
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
    registration: Option<&Registration<'graph>>,
) -> Result<StaticCallable<'o>, Stop<'o>>
where
    C: TypeContext<'graph, 'o> + ?Sized,
    X: Knotted,
{
    let types = tools.types;
    let form = body.form().expect("a callable body sits in its form");
    let reader = Reader::new(context, tools, encloser);
    match callable_type(form, &reader, types, tools.scratch, registration) {
        // Its registration's entry is the registration's own fact. A scheme beside bounds over an
        // encloser's names reads them, so it is rigid over them.
        Ok(callable) => {
            let callable = laid_callable(
                lay,
                Callable {
                    registered: None,
                    ..callable
                },
            );
            Ok(reader.fixed(lay, callable, |callable| close_callable(types, callable)))
        }
        Err(Elaboration::Unknown { .. }) => Ok(Static::Unknown),
        Err(error) => Err(refused(
            error,
            source_of(encloser, form, error.site()).unwrap_or(form.source),
        )),
    }
}

/// The callable `body`'s own group as its body reads it, read off its type `typed`, by group
/// order: the group's variable `i` is the lexical variable of the name it is, at that name's
/// shape level; each bound over an encloser's names at its upper end. Empty where the callable writes no `FOR ALL` group or the load did not
/// type it.
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
    // Each group variable's name, by group index.
    let mut names = BumpVec::with_capacity_in(bounds.len(), tools.scratch);
    names.resize(bounds.len(), None);
    for (name, at) in map.iter() {
        if let Some(each) = names.get_mut(at) {
            *each = Some(name);
        }
    }
    let mut levels = BumpVec::with_capacity_in(bounds.len(), tools.scratch);
    levels.extend(bounds.iter().zip(names.iter()).map(|(bound, name)| {
        let name = name.expect("each variable of the group is a name the declaration wrote");
        let (slot, _) = body
            .slot(BinderSymbol::Type(name))
            .expect("a `FOR ALL` name has its slot");
        let address = Address::Slot(slot);
        let level = (context.shapes().level(body.segment(), address))
            .expect("a `FOR ALL` name has its level");
        types.lexical(level, name, *bound)
    }));
    collect(lay, levels.iter().copied())
}

/// A callable's type with its slices laid through `lay`.
fn laid_callable<'o, T: Copy>(lay: impl Lays<'o>, callable: Callable<'_, T>) -> Callable<'o, T> {
    Callable {
        ktype: callable.ktype,
        quantifier_map: FunctionGroupMap(collect(lay, callable.quantifier_map.iter())),
        registered: (callable.registered).map(|registered| laid_registered(lay, registered)),
        rebounds: callable.rebounds.laid(lay),
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
        rebounds: registered.rebounds.laid(lay),
    }
}

/// A callable and its bucket entry that read no lexical variable, each declared type narrowed to
/// concrete and a scheme kept as it is.
fn close_callable<'o>(
    types: &Chain<'_, '_, '_, '_>,
    callable: Callable<'o>,
) -> Callable<'o, KType> {
    Callable {
        ktype: super::judge::static_declared(types, callable.ktype).expect(FREE),
        quantifier_map: callable.quantifier_map,
        registered: callable
            .registered
            .map(|registered| close_registered(types, registered)),
        rebounds: crate::scope::Rebounds::NONE,
    }
}

/// A bucket entry that read no lexical variable, its shape narrowed as [`close_callable`] narrows
/// a callable's type.
fn close_registered<'o>(
    types: &Chain<'_, '_, '_, '_>,
    registered: Registered<'o>,
) -> Registered<'o, KType> {
    Registered {
        shape: super::judge::static_declared(types, registered.shape).expect(FREE),
        quantifier_map: registered.quantifier_map,
        parameters: registered.parameters,
        rebounds: crate::scope::Rebounds::NONE,
    }
}

/// Where a refusal about the type expression at `site`, written in `statement`, is located: at the
/// part the refusal names, else at the expression.
fn located(
    shape: &BodyShape<'_>,
    statement: &KExpression<'_>,
    error: Elaboration,
    site: Site,
) -> SourceRef {
    source_of(shape, statement, error.site())
        .or_else(|| source_of(shape, statement, site))
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
