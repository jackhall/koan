//! The type channel's load pass: every type binder, type expression, callable and registration of
//! a loaded program typed where the program loads, a quote's code included, each result written
//! into the write-once cell the shape builder laid beside it.
//!
//! The pass walks the shape tree from the root, keeping the chain of shapes enclosing the one it
//! types. Its reader answers a coordinate from the builtin table and from the cells the pass has
//! already filled — a type binder is typed before anything reads it, since a shape's binders are
//! typed in unit order before its expressions and nested shapes. A name a run binds is a lexical
//! variable in `Typing` mode and *unknown* in `Declaring` mode, since a type binder is closed or
//! unknown, never rigid.
//!
//! Lexical variables are numbered by **level** along a **chain**: the shapes from a chain root —
//! the program, or a quote's code — down to the one read. A shape's own names take the levels after
//! its parent's, a callable's own `FOR ALL` group first in group order, so a name keeps its
//! level wherever a nested shape of the chain reads it, and sibling shapes reuse levels. A `$` name
//! crossing into code is a name of the code's own, bounded as the name it reads.
//!
//! A `USING … SCOPE` block's surfaced head is typed from its signature: the head's shape as the
//! `SIG` declares it, each head parameter read at the ascription's pin or, unpinned, at the block's
//! own type parameter of that name.
//!
//! See [README.md § The type channel at load](README.md#the-type-channel-at-load).

use std::cell::Cell;

use crate::memory::{BumpAllocator, BumpVec, Writer, collect, resident};
use crate::parse::KExpression;
use crate::parse::Role;
use crate::parse::quoted_body;
use crate::scope::{
    BodyShape, Builtins, Callable, CaptureSlot, CaptureSource, Coordinate, Elaboration,
    FunctionGroupMap, ParameterBinding, Registered, Registration, ShapeError, ShapeGroupMap,
    ShapeKind, Site, Slot, Static, StaticRegistered, StaticType, SurfacedHead, Target, UnitWork,
    Variable, source_of,
};
use crate::source::SourceRef;
use crate::symbols::BinderSymbol;
use crate::type_lattice::{
    DeclaredType, KType, Members, Parametric, TypeNode, TypeRegistry, substitute_parameters,
};
use crate::values::{Knotted, Value};

use super::declaration::{HeadShape, signature_heads, type_declarations};
use super::expression::{Elaborator, Groups, type_expression};
use super::reads::{Reads, TypeAt};
use super::signature::callable_type;

/// Type `root`'s type channel and every shape nested in it, writing each result into its cell and
/// laying the records it keeps down through `writer`. A closed type that does not elaborate, or a
/// `MATCH … WITH` arm set guarding one type twice, refuses the load; inside a quote's code the
/// refusal is kept on the code shape instead.
pub fn type_channel<'graph, X: Knotted>(
    root: &'graph BodyShape<'graph>,
    builtins: &Builtins<'_, X>,
    types: &TypeRegistry<'graph>,
    writer: Writer<'graph>,
    scratch: BumpAllocator<'_>,
) -> Result<(), ShapeError<'graph>> {
    let mut pass = Pass {
        builtins,
        types,
        writer,
        scratch,
        chain: BumpVec::new_in(scratch),
    };
    pass.chain.push(Level {
        shape: root,
        root: true,
        base: 0,
        count: 0,
        own: None,
        names: BumpVec::new_in(scratch),
    });
    pass.visit(0)
}

/// A run-bound name, by the chain level of the shape that declares it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Key {
    Slot(usize, Slot),
    Capture(usize, CaptureSlot),
}

/// A quantified callable's own `FOR ALL` names, read off its load-time type: each one's index in
/// its group, and the lexical variable each variable of the group is in its body.
#[derive(Clone, Copy)]
struct Own<'graph> {
    map: FunctionGroupMap<'graph>,
    levels: &'graph [Parametric],
}

/// One shape on the chain, innermost last.
struct Level<'p, 'graph> {
    shape: &'graph BodyShape<'graph>,
    /// Whether the shape roots a chain: the program, or a quote's code.
    root: bool,
    /// The first level the shape's own names take.
    base: usize,
    /// How many levels the shape's own names take, its own group included.
    count: usize,
    /// Set for a quantified callable body whose load-time type is known.
    own: Option<Own<'graph>>,
    /// Every other run-bound name the shape declares, beside its level.
    names: BumpVec<'p, (Key, usize)>,
}

/// What a coordinate holds, as far as the load can tell.
enum Class {
    Type(KType),
    NotAType,
    /// A run binds it: a name keyed `key`, bounded by `bound`, whose level its declaring shape
    /// holds.
    RunBound {
        key: Key,
        bound: KType,
    },
    /// One of a callable's own `FOR ALL` names: this lexical variable.
    Variable(Parametric),
}

/// Whether a reader is typing a type binder, which is closed or unknown, or anything else.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Declaring,
    Typing,
}

/// The walk's state: the chain of enclosing shapes, each numbering its names.
struct Pass<'p, 'graph, 'cell, X> {
    builtins: &'p Builtins<'cell, X>,
    types: &'p TypeRegistry<'graph>,
    writer: Writer<'graph>,
    scratch: BumpAllocator<'p>,
    chain: BumpVec<'p, Level<'p, 'graph>>,
}

/// The load-time reader: reads the names mentioned in the shape at chain level `from`.
struct Reader<'e, 'p, 'graph, 'cell, X> {
    pass: &'e Pass<'p, 'graph, 'cell, X>,
    from: usize,
    mode: Mode,
    rigid: Cell<u32>,
    /// Each rigid variable read, once per level.
    variables: Cell<BumpVec<'p, Variable>>,
}

impl<'graph, X: Knotted> Reads<'graph> for Reader<'_, '_, 'graph, '_, X> {
    fn shape(&self) -> &'graph BodyShape<'graph> {
        self.pass.chain[self.from].shape
    }

    fn type_at(&self, at: Coordinate) -> TypeAt {
        match self.pass.classify(self.from, at) {
            Class::Type(handle) => TypeAt::Type(handle),
            Class::NotAType => TypeAt::NotAType,
            _ if self.mode == Mode::Declaring => TypeAt::Unknown,
            Class::Variable(variable) => self.rigid(variable, at),
            Class::RunBound { key, bound } => self.rigid(self.pass.variable(key, bound), at),
        }
    }

    fn rigid_reads(&self) -> u32 {
        self.rigid.get()
    }
}

impl<X> Reader<'_, '_, '_, '_, X> {
    /// The lexical variable `variable`, read at `at`.
    fn rigid(&self, variable: Parametric, at: Coordinate) -> TypeAt {
        let TypeNode::Lexical { level, .. } = self.pass.types.node(variable) else {
            unreachable!("a run-bound name reads as a lexical variable");
        };
        self.rigid.set(self.rigid.get() + 1);
        edit(&self.variables, self.pass.scratch, |variables| {
            if !variables.iter().any(|held| held.level == level) {
                variables.push(Variable { level, at });
            }
        });
        TypeAt::Rigid(variable)
    }
}

/// `f` over the run `cell` holds, put back after.
fn edit<'x, T, R>(
    cell: &Cell<BumpVec<'x, T>>,
    scratch: BumpAllocator<'x>,
    f: impl FnOnce(&mut BumpVec<'x, T>) -> R,
) -> R {
    let mut run = cell.replace(BumpVec::new_in(scratch));
    let out = f(&mut run);
    cell.set(run);
    out
}

impl<'p, 'graph, 'cell, X: Knotted> Pass<'p, 'graph, 'cell, X> {
    fn reader(&self, from: usize, mode: Mode) -> Reader<'_, 'p, 'graph, 'cell, X> {
        Reader {
            pass: self,
            from,
            mode,
            rigid: Cell::new(0),
            variables: Cell::new(BumpVec::new_in(self.scratch)),
        }
    }

    /// The lexical variable the run-bound name `key` is, bounded by `bound`: its name, at the level
    /// its declaring shape numbered it.
    fn variable(&self, key: Key, bound: KType) -> Parametric {
        let (level, name) = match key {
            Key::Slot(level, slot) => (level, self.chain[level].shape.slot_name(slot)),
            Key::Capture(level, capture) => (
                level,
                self.chain[level].shape.captures()[capture.index()].name,
            ),
        };
        let BinderSymbol::Type(name) = name else {
            unreachable!("a run-bound name is a type name");
        };
        let (_, numbered) = self.chain[level]
            .names
            .iter()
            .find(|(held, _)| *held == key)
            .expect("a run-bound name has its level");
        self.types.lexical(*numbered, name, bound)
    }

    /// What `at`, read in the shape at level `from`, holds.
    fn classify(&self, from: usize, at: Coordinate) -> Class {
        let (hops, target) = match at {
            Coordinate::Builtin(index) => {
                return match self.builtins.get(index) {
                    Value::Type(value) => Class::Type(value.handle()),
                    _ => Class::NotAType,
                };
            }
            Coordinate::Activation { hops, target } => (hops, target),
        };
        let level = from
            .checked_sub(hops as usize)
            .expect("a coordinate steps out only through shapes on the chain");
        match target {
            Target::Local(slot) => self.slot_at(level, slot),
            Target::Capture(capture) => self.capture_at(level, capture),
        }
    }

    /// What the slot `slot` of the shape at `level` holds.
    fn slot_at(&self, level: usize, slot: Slot) -> Class {
        let shape = self.chain[level].shape;
        let key = Key::Slot(level, slot);
        if shape.declarations(slot).is_some() {
            return match shape.declared_type(slot) {
                Static::Closed(handle) => Class::Type(handle),
                _ => Class::RunBound {
                    key,
                    bound: KType::ANY,
                },
            };
        }
        let BinderSymbol::Type(name) = shape.slot_name(slot) else {
            return Class::NotAType;
        };
        if let Some(own) = self.chain[level].own
            && let Some(index) = own.map.get(name)
        {
            return Class::Variable(own.levels[index]);
        }
        Class::RunBound {
            key,
            bound: KType::ANY,
        }
    }

    /// What the capture `capture` of the shape at `level` holds: what its source reads where the
    /// shape is born — the very name, and so its level, when the shape shares its parent's chain —
    /// and otherwise, crossing into code, a run-bound name of the code's own, bounded as the name
    /// it reads.
    fn capture_at(&self, level: usize, capture: CaptureSlot) -> Class {
        let key = Key::Capture(level, capture);
        match self.chain[level].shape.captures()[capture.index()].source {
            CaptureSource::Read(inner) if level > 0 => match self.classify(level - 1, inner) {
                read if !self.chain[level].root => read,
                Class::Type(handle) => Class::Type(handle),
                Class::NotAType => Class::NotAType,
                Class::RunBound { bound, .. } => Class::RunBound { key, bound },
                Class::Variable(variable) => Class::RunBound {
                    key,
                    bound: self
                        .types
                        .node(variable)
                        .rigid_bound()
                        .expect("a lexical variable has a bound"),
                },
            },
            // A type name is never a knot member.
            CaptureSource::Member { .. } => Class::NotAType,
            _ => Class::RunBound {
                key,
                bound: KType::ANY,
            },
        }
    }

    /// `value`, typed through `reader`: closed when it read no lexical variable — then concrete,
    /// through `close` — else rigid over the variables it read, laid down in program storage.
    fn fixed<C, R>(
        &self,
        reader: &Reader<'_, 'p, 'graph, 'cell, X>,
        value: R,
        close: impl FnOnce(R) -> C,
    ) -> Static<'graph, C, R> {
        if reader.rigid.get() == 0 {
            return Static::Closed(close(value));
        }
        let variables = edit(&reader.variables, self.scratch, |variables| {
            collect(self.writer, variables.iter().copied())
        });
        Static::Rigid { value, variables }
    }

    /// [`fixed`](Self::fixed) for a type.
    fn fixed_type(
        &self,
        reader: &Reader<'_, 'p, 'graph, 'cell, X>,
        value: Parametric,
    ) -> StaticType<'graph> {
        self.fixed(reader, value, |value| {
            self.types.concrete(value).expect(FREE)
        })
    }

    /// The bucket entry of `registration`, a key of the block at chain level `level` its operand
    /// declares the heads `heads` at: where it declares one, that head's shape, and otherwise
    /// `None`, which the load leaves unknown. The function the key's list holds where the block
    /// runs answers each call, so the entry carries no quantifier map and binds no parameter.
    fn surfaced(
        &self,
        level: usize,
        registration: &Registration<'graph>,
        heads: &[SurfacedHead<'graph>],
        signatures: &mut Signatures<'p>,
    ) -> Option<StaticRegistered<'graph>> {
        match heads {
            [
                SurfacedHead::Signature {
                    head,
                    signature,
                    ascription,
                },
            ] => self.signature_head(
                level,
                registration,
                head,
                *signature,
                *ascription,
                signatures,
            ),
            // A body's definition is typed where the body is: its closed shape is the entry.
            [
                SurfacedHead::Body {
                    module: (hops, slot),
                    ..
                },
            ] => {
                let body = self.chain[level - *hops as usize].shape.births(*slot)?;
                let mut at_key = body
                    .registrations()
                    .iter()
                    .filter(|held| held.key == registration.key);
                let (Some(held), None) = (at_key.next(), at_key.next()) else {
                    return None;
                };
                match body.registered_type(held.slot) {
                    Static::Closed(registered) => Some(Static::Closed(registered)),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// The bucket entry of `registration`, the surfaced signature head `head` of the block at chain
    /// level `level`: the shape the head's `SIG` declares under the registration's key and ranking,
    /// each head parameter read at the ascription's pin or, unpinned, at the block's own type
    /// parameter of that name. `None` where the ascription is no closed signature or application
    /// of one, or the signature does not type where it is declared.
    fn signature_head(
        &self,
        level: usize,
        registration: &Registration<'graph>,
        head: &KExpression<'graph>,
        (declared_hops, slot): (u32, Slot),
        (hops, site): (u32, Site),
        signatures: &mut Signatures<'p>,
    ) -> Option<StaticRegistered<'graph>> {
        let (types, scratch) = (self.types, self.scratch);
        let ascribing = self.chain[level - hops as usize].shape;
        let Static::Closed(ascribed) = ascribing.typed_expression(site) else {
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
        let declaring = (level - declared_hops as usize, slot);
        let index = match signatures.iter().position(|(held, _)| *held == declaring) {
            Some(index) => index,
            None => {
                let node = self.chain[declaring.0].shape.declarations(slot)?;
                let reader = self.reader(declaring.0, Mode::Declaring);
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
        let reader = self.reader(level, Mode::Typing);
        let block = self.chain[level].shape;
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
        let read = substitute_parameters(types, scratch, *shape, Members::from_table(bindings));
        let registered = Registered {
            shape: ranked(types, scratch, read, registration.classes),
            quantifier_map: ShapeGroupMap::default(),
            parameters: ParameterBinding::Named(&[]),
        };
        Some(self.fixed(&reader, registered, |registered| {
            close_registered(types, registered)
        }))
    }

    /// Type the shape at chain level `level`, then every shape nested in it.
    fn visit(&mut self, level: usize) -> Result<(), ShapeError<'graph>> {
        let shape = self.chain[level].shape;
        let (types, scratch) = (self.types, self.scratch);

        for unit in shape.units() {
            let UnitWork::Component(index) = unit.work else {
                continue;
            };
            let component = &shape.components()[index.index()];
            if shape.declarations(component.members[0]).is_none() {
                continue;
            }
            let reader = self.reader(level, Mode::Declaring);
            match type_declarations(component, &reader, types, scratch) {
                Ok(handles) => {
                    for (slot, handle) in component.members.iter().zip(handles) {
                        shape.fix_declared(*slot, Static::Closed(*handle));
                    }
                }
                Err(Elaboration::Unknown { .. }) => {}
                Err(error) => {
                    let mut nodes = component
                        .members
                        .iter()
                        .filter_map(|slot| shape.declarations(*slot));
                    let first = nodes.clone().next().expect("a type binder has its node");
                    let at = nodes
                        .find_map(|node| source_of(node, error.site()))
                        .unwrap_or(first.source);
                    return Err(ShapeError::Type { error, at });
                }
            }
        }

        self.number(level);

        let top = Groups {
            names: &[],
            bounds: &[],
            outer: None,
        };
        for expression in shape.type_expressions() {
            let reader = self.reader(level, Mode::Typing);
            let typed = match expression.guard {
                Some(_) => {
                    let body = quoted_body(expression.part).expect("a guard is a quote");
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
                Ok(handle) => expression.fix(self.fixed_type(&reader, handle)),
                Err(Elaboration::Unknown { .. }) => {}
                Err(error) => {
                    let statement = &shape.body()[expression.statement as usize];
                    return Err(ShapeError::Type {
                        error,
                        at: located(statement, error, expression.site),
                    });
                }
            }
        }
        repeated_guards(shape)?;

        let mut signatures: Signatures<'p> = BumpVec::new_in(scratch);
        for registration in shape.registrations() {
            if let Some(head) = registration.surfaced {
                if let Some(typed) = self.surfaced(level, registration, head, &mut signatures) {
                    shape.fix_registered(registration.slot, typed);
                }
                continue;
            }
            let Some(form) = shape.births(registration.slot).and_then(BodyShape::form) else {
                continue;
            };
            let reader = self.reader(level, Mode::Typing);
            match callable_type(form, &reader, types, scratch, Some(registration)) {
                Ok(callable) => {
                    let registered = callable
                        .registered
                        .expect("a callable born for a registration carries its bucket entry");
                    let registered = laid_registered(self.writer, registered);
                    let typed = self.fixed(&reader, registered, |registered| {
                        close_registered(types, registered)
                    });
                    shape.fix_registered(registration.slot, typed);
                }
                Err(Elaboration::Unknown { .. }) => {}
                Err(error) => {
                    return Err(ShapeError::Type {
                        error,
                        at: source_of(form, error.site()).unwrap_or(form.source),
                    });
                }
            }
        }

        for (_, nested) in shape.nested_shapes() {
            let root = nested.kind() == ShapeKind::Code;
            // A quote whose code could not be built has nothing to type.
            if root && nested.refusal().is_some() {
                continue;
            }
            let base = match root {
                true => 0,
                false => self.chain[level].base + self.chain[level].count,
            };
            let own = match nested.kind() {
                ShapeKind::Callable => {
                    let form = nested.form().expect("a callable body sits in its form");
                    let reader = self.reader(level, Mode::Typing);
                    let typed = match callable_type(form, &reader, types, scratch, None) {
                        Ok(callable) => {
                            self.fixed(&reader, laid_callable(self.writer, callable), |callable| {
                                close_callable(types, callable)
                            })
                        }
                        Err(Elaboration::Unknown { .. }) => Static::Unknown,
                        Err(error) => {
                            return Err(ShapeError::Type {
                                error,
                                at: source_of(form, error.site()).unwrap_or(form.source),
                            });
                        }
                    };
                    drop(reader);
                    nested.fix_callable(typed);
                    let group = match typed {
                        Static::Closed(callable) => {
                            Some((callable.quantifier_map, callable.ktype.into()))
                        }
                        Static::Rigid {
                            value: callable, ..
                        } => Some((callable.quantifier_map, callable.ktype)),
                        Static::Unknown => None,
                    };
                    match group {
                        Some((map, ktype)) if writes_for_all(form) => {
                            let own = self.own(map, ktype, base);
                            nested.fix_group_levels(own.levels);
                            Some(own)
                        }
                        _ => None,
                    }
                }
                _ => None,
            };
            self.chain.push(Level {
                shape: nested,
                root,
                base,
                count: 0,
                own,
                names: BumpVec::new_in(scratch),
            });
            let visited = self.visit(level + 1);
            self.chain.pop();
            match visited {
                Err(error) if nested.kind() == ShapeKind::Code => {
                    nested.refuse_typing(resident(self.writer, error));
                }
                visited => visited?,
            }
        }
        Ok(())
    }
}

/// The expression shape `shape` ranked by `classes`.
fn ranked(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    shape: DeclaredType<Parametric>,
    classes: &[u8],
) -> DeclaredType<Parametric> {
    let node = match shape {
        DeclaredType::Type(shape) => types.node(shape),
        DeclaredType::Scheme(scheme) => types.scheme_node(scheme),
    };
    let TypeNode::ExpressionShape {
        quantifiers,
        bounds,
        elements,
        ret,
        ..
    } = node
    else {
        unreachable!("a head declares an expression shape");
    };
    let mut written = BumpVec::with_capacity_in(elements.len(), scratch);
    written.extend(elements.iter());
    types
        .shape_scheme(scratch, quantifiers, bounds, &written, classes, ret)
        .handle
}

/// Each `SIG` a block's surfaced heads name, by its declaring level and slot, beside its handle
/// and its heads' shapes — `None` where it does not type — so the block elaborates each once.
type Signatures<'p> = BumpVec<'p, ((usize, Slot), Option<(KType, BumpVec<'p, HeadShape>)>)>;

/// Why a load-time type that read no lexical variable is concrete: nothing it was elaborated under
/// declares a `FOR ALL` group or a head parameter it could name, and every binder door binds its
/// own group's variables, so none escapes free.
const FREE: &str = "a load-time type that read no lexical variable holds no variable";

/// `declared` where it read no lexical variable: a scheme as it is, a type narrowed to concrete.
fn close_declared(
    types: &TypeRegistry<'_>,
    declared: DeclaredType<Parametric>,
) -> DeclaredType<KType> {
    match declared {
        DeclaredType::Type(kt) => DeclaredType::Type(types.concrete(kt).expect(FREE)),
        DeclaredType::Scheme(scheme) => DeclaredType::Scheme(scheme),
    }
}

/// [`close_declared`] over a callable and its bucket entry.
fn close_callable<'graph>(
    types: &TypeRegistry<'_>,
    callable: Callable<'graph>,
) -> Callable<'graph, KType> {
    Callable {
        ktype: close_declared(types, callable.ktype),
        quantifier_map: callable.quantifier_map,
        registered: callable
            .registered
            .map(|registered| close_registered(types, registered)),
    }
}

/// [`close_declared`] over a bucket entry.
fn close_registered<'graph>(
    types: &TypeRegistry<'_>,
    registered: Registered<'graph>,
) -> Registered<'graph, KType> {
    Registered {
        shape: close_declared(types, registered.shape),
        quantifier_map: registered.quantifier_map,
        parameters: registered.parameters,
    }
}

impl<'p, 'graph, X> Pass<'p, 'graph, '_, X> {
    /// Number the run-bound names the shape at chain level `level` declares, after its own group:
    /// each type name in slot order but a closed type binder's, then, at a chain root, each
    /// capture naming a type. The shape records each one's level beside where its activation
    /// holds it.
    fn number(&mut self, level: usize) {
        let at = &self.chain[level];
        let shape = at.shape;
        let mut next = at.base + at.own.map_or(0, |own| own.levels.len());
        let mut names = BumpVec::new_in(self.scratch);
        for index in 0..shape.slots() {
            let slot = Slot(index as u32);
            let BinderSymbol::Type(name) = shape.slot_name(slot) else {
                continue;
            };
            let own = at.own.is_some_and(|own| own.map.get(name).is_some());
            let closed = shape.declarations(slot).is_some()
                && matches!(shape.declared_type(slot), Static::Closed(_));
            if !own && !closed {
                names.push((Key::Slot(level, slot), next));
                next += 1;
            }
        }
        for (index, capture) in shape.captures().iter().enumerate() {
            debug_assert!(
                at.root || !matches!(capture.source, CaptureSource::Hole | CaptureSource::Offered),
                "only a code shape holds a hole or an offered name"
            );
            if at.root && matches!(capture.name, BinderSymbol::Type(_)) {
                names.push((Key::Capture(level, CaptureSlot(index as u32)), next));
                next += 1;
            }
        }
        let mut homes = BumpVec::new_in(self.scratch);
        if let Some(own) = at.own {
            homes.extend(own.map.iter().map(|(name, index)| {
                let (slot, _) = shape
                    .slot(BinderSymbol::Type(name))
                    .expect("a `FOR ALL` name has its slot");
                (at.base + index, Target::Local(slot))
            }));
        }
        homes.extend(names.iter().map(|(key, numbered)| match key {
            Key::Slot(_, slot) => (*numbered, Target::Local(*slot)),
            Key::Capture(_, capture) => (*numbered, Target::Capture(*capture)),
        }));
        if !homes.is_empty() {
            shape.fix_declared_variables(collect(self.writer, homes.iter().copied()));
        }
        let at = &mut self.chain[level];
        at.count = next - at.base;
        at.names = names;
    }

    /// A quantified callable's own group, read off its load-time type `ktype` and its group map
    /// `map`: the group's variable `i` is the lexical variable at level `base + i`, named as the
    /// declaration wrote it, laid down in program storage.
    fn own(
        &self,
        map: FunctionGroupMap<'graph>,
        ktype: DeclaredType<Parametric>,
        base: usize,
    ) -> Own<'graph> {
        let bounds = match ktype {
            DeclaredType::Scheme(scheme) => match self.types.scheme_node(scheme) {
                TypeNode::KFunction { bounds, .. } => bounds,
                _ => unreachable!("a callable's type is a function type"),
            },
            // An empty group binds nothing.
            DeclaredType::Type(_) => &[],
        };
        let mut levels = BumpVec::with_capacity_in(bounds.len(), self.scratch);
        levels.extend(bounds.iter().enumerate().map(|(index, bound)| {
            let (name, _) = map
                .iter()
                .find(|(_, at)| *at == index)
                .expect("each variable of the group is a name the declaration wrote");
            self.types.lexical(base + index, name, *bound)
        }));
        Own {
            map,
            levels: collect(self.writer, levels.iter().copied()),
        }
    }
}

/// Refuse the first `MATCH … WITH` guard of `shape` that types to the handle an earlier guard of
/// its arm set does.
fn repeated_guards<'graph>(shape: &'graph BodyShape<'graph>) -> Result<(), ShapeError<'graph>> {
    let handle = |typed: StaticType<'_>| match typed {
        Static::Closed(handle) => Some(Parametric::from(handle)),
        Static::Rigid { value: handle, .. } => Some(handle),
        Static::Unknown => None,
    };
    let guards = || {
        shape
            .type_expressions()
            .iter()
            .filter_map(|expression| Some((expression, expression.guard?)))
    };
    for (expression, (arms, index)) in guards() {
        let Some(guard) = handle(expression.typed()) else {
            continue;
        };
        let first = guards().find(|(earlier, (held, written))| {
            *held == arms && *written < index && handle(earlier.typed()) == Some(guard)
        });
        if let Some((first, _)) = first {
            let body = shape.body();
            let source = |expression: &crate::scope::TypeExpression<'_>| {
                let statement = &body[expression.statement as usize];
                source_of(statement, expression.site).unwrap_or(statement.source)
            };
            return Err(ShapeError::RepeatedGuard {
                guard,
                first: source(first),
                at: source(expression),
            });
        }
    }
    Ok(())
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
pub fn writes_for_all(form: &KExpression<'_>) -> bool {
    form.cache()
        .builtin_shape()
        .is_some_and(|shape| shape.roles().any(|role| role == Role::Quantifiers))
}

/// `callable`'s runs laid down in program storage.
fn laid_callable<'graph>(writer: Writer<'graph>, callable: Callable<'_>) -> Callable<'graph> {
    Callable {
        ktype: callable.ktype,
        quantifier_map: FunctionGroupMap(collect(writer, callable.quantifier_map.iter())),
        registered: callable
            .registered
            .map(|registered| laid_registered(writer, registered)),
    }
}

/// `registered`'s runs laid down in program storage.
fn laid_registered<'graph>(
    writer: Writer<'graph>,
    registered: Registered<'_>,
) -> Registered<'graph> {
    Registered {
        shape: registered.shape,
        quantifier_map: ShapeGroupMap(collect(writer, registered.quantifier_map.iter())),
        parameters: match registered.parameters {
            ParameterBinding::Named(names) => {
                ParameterBinding::Named(collect(writer, names.iter().copied()))
            }
            ParameterBinding::Operands => ParameterBinding::Operands,
        },
    }
}
