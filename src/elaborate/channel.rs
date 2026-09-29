//! The type channel's load pass: every type binder, type expression, callable and registration of
//! a loaded program typed where the program loads, a quote's code included, each result written
//! into the write-once cell the shape builder laid beside it.
//!
//! The pass walks the shape tree from the root, keeping the chain of shapes enclosing the one it
//! types. Its reader answers a coordinate from the builtin table and from the cells the pass has
//! already filled — a type binder is typed before anything reads it, since a shape's binders are
//! typed in unit order before its expressions and nested shapes. A name a run binds is a rigid
//! variable in `Typing` mode, numbered per **region** — the program, a quantified callable's body,
//! a quote's code — and *unknown* in `Declaring` mode, since a type binder is closed or unknown,
//! never rigid.
//!
//! See [README.md § The type channel at load](README.md#the-type-channel-at-load).

use std::cell::Cell;

use crate::memory::{BumpAllocator, BumpVec, Writer, collect, resident};
use crate::parse::KExpression;
use crate::parse::builtin_shapes::binder::quoted_body;
use crate::parse::builtin_shapes::role::Role;
use crate::scope::{
    BodyShape, Builtins, Callable, Canonical, CaptureSlot, CaptureSource, Coordinate, Elaboration,
    ParameterBinding, Registered, ShapeError, ShapeKind, Site, Slot, Static, Target, UnitWork,
    Variable, source_of,
};
use crate::source::SourceRef;
use crate::symbols::{BinderSymbol, TypeSymbol};
use crate::type_lattice::{KType, TypeNode, TypeRegistry};
use crate::values::{Knotted, Value};

use super::declaration::type_declarations;
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
        regions: Cell::new(BumpVec::new_in(scratch)),
    };
    let region = pass.open_region(0, 0);
    pass.chain.push(Level {
        shape: root,
        region,
        own: None,
    });
    pass.visit(0)
}

/// A run-bound slot, by the chain level whose shape holds it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Key {
    Slot(usize, Slot),
    Capture(usize, CaptureSlot),
}

/// A quantified callable's own `FOR ALL` names, read off its load-time type: where each landed in
/// its canonical group, and each canonical variable's bound.
#[derive(Clone, Copy)]
struct Own<'graph> {
    map: &'graph [(TypeSymbol, Canonical)],
    bounds: &'graph [KType],
}

/// One shape on the chain, innermost last.
struct Level<'graph> {
    shape: &'graph BodyShape<'graph>,
    /// The region its run-bound names are numbered in, by index into [`Pass::regions`].
    region: usize,
    /// Set for a quantified callable body whose load-time type is known.
    own: Option<Own<'graph>>,
}

/// A run of shapes numbering their run-bound names together.
struct Region<'x> {
    /// The chain level of the region's root shape.
    level: usize,
    /// The next free index.
    next: usize,
    assigned: BumpVec<'x, (Key, usize)>,
}

/// What a coordinate holds, as far as the load can tell.
enum Class {
    Type(KType),
    NotAType,
    /// A run binds it: a slot keyed `key`, bounded by `bound`.
    RunBound {
        key: Key,
        bound: KType,
    },
    /// The canonical variable `index` of the region's own quantified callable.
    Canonical {
        index: usize,
        bound: KType,
    },
}

/// Whether a reader is typing a type binder, which is closed or unknown, or anything else.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Declaring,
    Typing,
}

/// The walk's state: the chain of enclosing shapes and the regions numbering their names.
struct Pass<'p, 'graph, 'cell, X> {
    builtins: &'p Builtins<'cell, X>,
    types: &'p TypeRegistry<'graph>,
    writer: Writer<'graph>,
    scratch: BumpAllocator<'p>,
    chain: BumpVec<'p, Level<'graph>>,
    regions: Cell<BumpVec<'p, Region<'p>>>,
}

/// The load-time reader: reads the names mentioned in the shape at chain level `from`.
struct Reader<'e, 'p, 'graph, 'cell, X> {
    pass: &'e Pass<'p, 'graph, 'cell, X>,
    from: usize,
    mode: Mode,
    rigid: Cell<u32>,
    /// Each rigid variable read, once per index.
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
            Class::Canonical { index, bound } => self.rigid(index, bound, at),
            Class::RunBound { key, bound } => {
                let index = self.pass.index(self.from, key);
                self.rigid(index, bound, at)
            }
        }
    }

    fn rigid_reads(&self) -> u32 {
        self.rigid.get()
    }
}

impl<X> Reader<'_, '_, '_, '_, X> {
    /// Rigid variable `index`, bounded by `bound`, read at `at`.
    fn rigid(&self, index: usize, bound: KType, at: Coordinate) -> TypeAt {
        self.rigid.set(self.rigid.get() + 1);
        edit(&self.variables, self.pass.scratch, |variables| {
            if !variables.iter().any(|variable| variable.index == index) {
                variables.push(Variable { index, at });
            }
        });
        TypeAt::Rigid(self.pass.types.quantified(index, bound))
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

    /// A new region rooted at chain level `level`, its free indices starting at `next`.
    fn open_region(&self, level: usize, next: usize) -> usize {
        edit(&self.regions, self.scratch, |regions| {
            regions.push(Region {
                level,
                next,
                assigned: BumpVec::new_in(self.scratch),
            });
            regions.len() - 1
        })
    }

    /// The index `key` holds in the region of the shape at level `from`, assigned on first sight.
    fn index(&self, from: usize, key: Key) -> usize {
        edit(&self.regions, self.scratch, |regions| {
            let region = &mut regions[self.chain[from].region];
            if let Some((_, index)) = region.assigned.iter().find(|(held, _)| *held == key) {
                return *index;
            }
            let index = region.next;
            region.next += 1;
            region.assigned.push((key, index));
            index
        })
    }

    /// The chain level of the root of the region the shape at level `from` sits in.
    fn region_root(&self, from: usize) -> usize {
        edit(&self.regions, self.scratch, |regions| {
            regions[self.chain[from].region].level
        })
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
            Target::Local(slot) => self.slot_at(from, level, slot),
            Target::Capture(capture) => self.capture_at(level, capture),
        }
    }

    /// What the slot `slot` of the shape at `level` holds, read from level `from`.
    fn slot_at(&self, from: usize, level: usize, slot: Slot) -> Class {
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
            && let Some((_, canonical)) = own.map.iter().find(|(held, _)| *held == name)
        {
            return match *canonical {
                Canonical::Dropped { bound } => Class::Type(bound),
                Canonical::At(index) => {
                    let bound = own.bounds[index];
                    if self.region_root(from) == level {
                        Class::Canonical { index, bound }
                    } else {
                        Class::RunBound { key, bound }
                    }
                }
            };
        }
        Class::RunBound {
            key,
            bound: KType::ANY,
        }
    }

    /// What the capture `capture` of the shape at `level` holds: what its source reads where the
    /// shape is born when that is a type, and otherwise a run-bound name of this shape's own.
    fn capture_at(&self, level: usize, capture: CaptureSlot) -> Class {
        let key = Key::Capture(level, capture);
        match self.chain[level].shape.captures()[capture.index()].source {
            CaptureSource::Read(inner) if level > 0 => match self.classify(level - 1, inner) {
                Class::Type(handle) => Class::Type(handle),
                Class::NotAType => Class::NotAType,
                Class::RunBound { bound, .. } | Class::Canonical { bound, .. } => {
                    Class::RunBound { key, bound }
                }
            },
            // A type name is never a knot member.
            CaptureSource::Member { .. } => Class::NotAType,
            _ => Class::RunBound {
                key,
                bound: KType::ANY,
            },
        }
    }

    /// `value`, typed through `reader`: closed when it read no rigid variable, else rigid over the
    /// variables it read, laid down in program storage.
    fn fixed<T>(&self, reader: &Reader<'_, 'p, 'graph, 'cell, X>, value: T) -> Static<'graph, T> {
        if reader.rigid.get() == 0 {
            return Static::Closed(value);
        }
        let variables = edit(&reader.variables, self.scratch, |variables| {
            collect(self.writer, variables.iter().copied())
        });
        Static::Rigid { value, variables }
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
                    };
                    elaborator.node(expression.site, body, &top)
                }
                None => type_expression(expression.part, &reader, types, scratch),
            };
            match typed {
                Ok(handle) => expression.fix(self.fixed(&reader, handle)),
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

        for registration in shape.registrations() {
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
                    shape.fix_registered(registration.slot, self.fixed(&reader, registered));
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
            let (region, own) = match nested.kind() {
                ShapeKind::Callable => {
                    let form = nested.form().expect("a callable body sits in its form");
                    let reader = self.reader(level, Mode::Typing);
                    let typed = match callable_type(form, &reader, types, scratch, None) {
                        Ok(callable) => self.fixed(&reader, laid_callable(self.writer, callable)),
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
                    if quantified(form) {
                        let own = match typed {
                            Static::Closed(callable) => Some(Own {
                                map: callable.quantifier_map,
                                bounds: match types.node(callable.ktype) {
                                    TypeNode::KFunction { bounds, .. } => bounds,
                                    _ => &[],
                                },
                            }),
                            _ => None,
                        };
                        let next = own.map_or(0, |own| own.bounds.len());
                        (self.open_region(level + 1, next), own)
                    } else {
                        (self.chain[level].region, None)
                    }
                }
                // A quote whose code could not be built has nothing to type.
                ShapeKind::Code if nested.refusal().is_some() => continue,
                ShapeKind::Code => (self.open_region(level + 1, 0), None),
                _ => (self.chain[level].region, None),
            };
            self.chain.push(Level {
                shape: nested,
                region,
                own,
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

/// Refuse the first `MATCH … WITH` guard of `shape` that types to the handle an earlier guard of
/// its arm set does.
fn repeated_guards<'graph>(shape: &'graph BodyShape<'graph>) -> Result<(), ShapeError<'graph>> {
    let handle = |typed| match typed {
        Static::Closed(handle) | Static::Rigid { value: handle, .. } => Some(handle),
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

/// Whether a callable's form writes a `FOR ALL` group.
fn quantified(form: &KExpression<'_>) -> bool {
    form.cache()
        .builtin_shape()
        .is_some_and(|shape| shape.roles().any(|role| role == Role::Quantifiers))
}

/// `callable`'s runs laid down in program storage.
fn laid_callable<'graph>(writer: Writer<'graph>, callable: Callable<'_>) -> Callable<'graph> {
    Callable {
        ktype: callable.ktype,
        quantifier_map: collect(writer, callable.quantifier_map.iter().copied()),
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
        quantifier_map: collect(writer, registered.quantifier_map.iter().copied()),
        parameters: match registered.parameters {
            ParameterBinding::Named(names) => {
                ParameterBinding::Named(collect(writer, names.iter().copied()))
            }
            ParameterBinding::Operands => ParameterBinding::Operands,
        },
    }
}
