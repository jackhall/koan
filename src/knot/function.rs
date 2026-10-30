//! A function as a knot node: its memoized type, the record of the body it runs, the closure bindings a
//! call reads its captures through, the shape a registration puts in its bucket, and the weight of
//! the whole knot it sits in. Its content, which its knot's digest covers, is its body's code
//! digest, its instance's solution, and the digest of each capture the code digest does not name
//! ([`Function::content`]). A function of written code points at the load's record of its body;
//! one born of code composed at run time keeps a copy of its composition's record in its typing
//! record, the root of that code on it, which finds or rebuilds the composition — so the knot node
//! stays 64 bytes and nothing it holds points into an arena node.
//!
//! A function whose body is untraced code — named code the load could not follow to its quote —
//! is born over the code its body part evaluated to: its closure is that code, laid flat, and the
//! record of what its declaring scope offers it, and its typing record holds what finds the code's
//! composition, which binds its parameters ([`Composing`]). A call builds or finds that
//! composition and runs its root ([`Function::frame_shape`]).
//!
//! Beside it, the staging a [tie](super::tie()) does for a function node. Everything a node needs
//! is read into scratch with no writer in reach — its body shape, its type as the facts fix it, and
//! its captures, every mention of a fellow member minted as an edge into the
//! knot about to be tied — so a refusal writes nothing. The same staging and lay-down serve the
//! lambda door, [`lambda`], which births a callable no binder names as a one-node knot.
//!
//! A quantified function is made concrete where the load solved its group at the type it is wanted
//! at: born so, where its body shape holds the solution ([`BodyShape::born_instance`]), or read so
//! through [`instance`], a one-node knot over the same body and captures. Either way the solution is
//! closed where the site runs — the load, or a layer the site reads, fixed it — the typing record
//! carries it, and a frame binds the body's `FOR ALL` names from it.

use std::cell::Cell;

use crate::memory::{Bump, BumpAllocator, BumpVec, Edge, KnotPlan, Writer, resident};
use crate::scope::{Activated, AtRun, Elaboration, StaticInstance};
use crate::scope::{
    BodyAt, BodyRef, BodyShape, Candidate, ClosureBindings, ComposedRoot, Offer, Position,
    Registration, ShapeKind, Site, Slot, Stamp, Written,
};
use crate::scope::{Callable, FunctionGroupMap, ParameterBinding, Registered, ShapeGroupMap};
use crate::symbols::{BinderSymbol, TypeSymbol};
use crate::type_lattice::{DeclaredType, KType, Scheme, instantiate_quantified};
use crate::values::digest::{composition_digest, function_content};
use crate::values::{
    ArenaRef, Chain, ContentDigest, Knotted as _, Link, List, Record, Retyped, Seen, Value, Weight,
};

use super::{Coerced, KReading, KValue, Knotted, Node, Untieable, composed, flattened};

/// A function: what one knot node holds.
pub struct Function<'graph, 'cell, X> {
    /// The function's type: a scheme where a `FOR ALL` group quantifies it, and otherwise
    /// concrete, every lexical variable its declaration read bound where it was born.
    ktype: DeclaredType<KType>,
    /// The quantifier map, registered shape and instance solution, or `None` where the function has
    /// none of them — an unquantified `FN`, almost every function, which costs nothing. Homed out
    /// of line for the reason [`Node::Coerced`] is.
    ///
    /// [`Node::Coerced`]: super::Node::Coerced
    typing: Option<&'cell Typing<'cell>>,
    /// The body it runs, beside which of its type captures its content composes: the load's, or
    /// `None` for a body of composed code, whose record its typing record holds.
    body: Option<&'graph BodyRef<'graph, 'graph>>,
    closure: &'cell ClosureBindings<'cell, X>,
    /// What rebuilding the whole knot this node sits in writes, the same on every node; the root
    /// node's also carries the knot's memo bits.
    knot_weight: Cell<Weight>,
}

impl<'graph, 'cell, X> Function<'graph, 'cell, X> {
    /// The function `ktype` runs through `body` over `closure`, inside a knot weighing
    /// `knot_weight`. The private-field constructor the tie uses.
    pub(super) fn new(
        ktype: DeclaredType<KType>,
        typing: Option<&'cell Typing<'cell>>,
        body: Option<&'graph BodyRef<'graph, 'graph>>,
        closure: &'cell ClosureBindings<'cell, X>,
        knot_weight: Weight,
    ) -> Self {
        Function {
            ktype,
            typing,
            body,
            closure,
            knot_weight: Cell::new(knot_weight),
        }
    }

    /// The function's type: elaborated from its signature where it was born, or the annotation's
    /// retype where it was born at one — what a caller sees.
    pub fn ktype(&self) -> DeclaredType<KType> {
        self.ktype
    }

    /// The signature the body was checked against: the one its parameters, return and group are
    /// read from. Its own type, unless it was born at an annotation that retypes it.
    pub fn declared(&self) -> DeclaredType<KType> {
        self.typing
            .and_then(|typing| typing.declared)
            .unwrap_or(self.ktype)
    }

    /// Each `FOR ALL` name the declaration wrote, with its index in the type's group; empty for an
    /// unquantified function.
    pub fn quantifier_map(&self) -> FunctionGroupMap<'cell> {
        self.typing
            .map_or_else(FunctionGroupMap::default, |typing| typing.quantifier_map)
    }

    /// What the registration this function was born for puts in its bucket, built from its type
    /// over the registration's key where it was born; `None` for a function no registration binds.
    pub fn registered(&self) -> Option<Registered<'cell, KType>> {
        self.typing.and_then(|typing| typing.registered)
    }

    /// The solution this function's group was instantiated at, in group order, where it is an
    /// instance of a quantified function: its type is then that instance's, and a call solves
    /// nothing.
    pub fn instance(&self) -> Option<&'cell [KType]> {
        self.typing
            .map(|typing| typing.instance)
            .filter(|instance| !instance.is_empty())
    }

    /// The expression shape this function's registration puts in its bucket.
    pub fn registered_shape(&self) -> Option<DeclaredType<KType>> {
        self.registered().map(|registered| registered.shape)
    }

    /// The index in the type's group of the type parameter named `name`, and `None` where the
    /// function binds no such name.
    ///
    /// Keyed by name because a frame walks its callee's slots **symbol-sorted**, not in the order
    /// the `FOR ALL` group was written.
    pub fn quantifier_index(&self, name: TypeSymbol) -> Option<usize> {
        self.quantifier_map().get(name)
    }

    /// The body as the function holds it: the segment of the shape a call activates, the closure
    /// slots its content composes, and the root of the composed code it lies in, if any.
    pub fn body(&self) -> &'cell BodyRef<'cell, 'cell>
    where
        'graph: 'cell,
    {
        match self.body {
            Some(body) => body,
            None => (self.typing.and_then(|typing| typing.composed))
                .expect("a function holds its body's record, or its typing does"),
        }
    }

    /// The closure bindings a call's activation reads its captures through.
    pub fn closure(&self) -> &'cell ClosureBindings<'cell, X> {
        self.closure
    }

    /// What a call finds the composition of this function's untraced body by; `None` for any other
    /// body.
    pub fn composing(&self) -> Option<Composing<'cell>> {
        self.typing.and_then(|typing| typing.composing)
    }

    pub fn knot_weight(&self) -> Weight {
        self.knot_weight.get()
    }

    /// The knot weight's cell, where a knot's root node keeps its memo bits.
    pub(super) fn knot_weight_cell(&self) -> &Cell<Weight> {
        &self.knot_weight
    }

    /// This function over `closure` rebuilt at another region lifetime — the copy's arm. The type,
    /// the body shape and the knot weight ride over: a copy re-ties the same knot. The typing
    /// record is re-homed through `writer`, since it is a run in the source region.
    pub(super) fn rebuilt<'to, Y>(
        &self,
        writer: Writer<'to, '_>,
        closure: &'to ClosureBindings<'to, Y>,
    ) -> Function<'graph, 'to, Y> {
        Function {
            ktype: self.ktype,
            typing: Typing::laid_down(
                writer,
                self.quantifier_map(),
                self.registered(),
                self.instance().unwrap_or(&[]),
                self.typing.and_then(|typing| typing.declared),
                // A copy holds its own copy of the root's syntax.
                self.body.is_none().then(|| {
                    let body = self.body();
                    (body, body.root.map(|root| root.copied(writer)))
                }),
                self.composing(),
            ),
            body: self.body,
            closure,
            knot_weight: Cell::new(self.knot_weight.get().without_bits()),
        }
    }
}

impl<'graph, 'cell> Function<'graph, 'cell, Knotted<'graph, 'cell>> {
    /// The root of this function's untraced body's code — the code is its first closure binding —
    /// beside what finds its composition. `None` for any other body.
    pub fn untraced_root(&self) -> Option<ComposedRoot<'cell>> {
        let composing = self.composing()?;
        let Some(Link::Value(code)) = self.closure.links().first() else {
            unreachable!("an untraced body's first closure binding is its code")
        };
        let syntax = code.as_code()?.code()?.body()?;
        Some(ComposedRoot {
            syntax,
            digest: composing.digest,
            parameters: composing.parameters,
        })
    }

    /// The root of the composed code a call of this function runs under: its untraced body's
    /// code, or the composed code its body lies in. `None` for a body of written code.
    pub fn root(&self) -> Option<ComposedRoot<'cell>> {
        self.untraced_root().or(self.body().root)
    }

    /// The shape a call of this function activates, read through `door`: an untraced body's
    /// composition's root, which `door` must rest on, or the shape at its body's segment.
    pub fn frame_shape<'step>(&self, door: &crate::scope::Code<'step>) -> &'step BodyShape<'step> {
        match self.composing() {
            Some(_) => door.shapes().root(),
            None => door.shape(self.body().segment),
        }
    }
    /// The values the node's content covers, in the order [`content`](Self::content) asks for
    /// their digests: each capture the code digest does not name that holds a value, not an edge.
    pub(super) fn content_parts<'a>(&self, out: &mut dyn FnMut(Seen<'a, Knotted<'graph, 'cell>>))
    where
        'cell: 'a,
    {
        for (_, link) in composed(self.body(), self.closure.links()) {
            if let Link::Value(value) = link {
                out(Seen::of(*value));
            }
        }
    }

    /// The node's content: its body's code digest, the solution it is an instance at, and the
    /// digest of each capture the code digest does not name — every one but a read of the
    /// program's top level, and every type capture — a value's answered by `parts`.
    pub(super) fn content(&self, parts: &mut dyn FnMut() -> ContentDigest) -> ContentDigest {
        let captures = composed(self.body(), self.closure.links())
            .map(|(index, link)| (index, link.digest_from(parts)));
        function_content(self.body().code, self.instance().unwrap_or(&[]), captures)
    }
}

/// What a call and a selection read beside a function's type: its quantifier map, each `FOR ALL`
/// name the declaration wrote paired with its index in the type's group, what its registration
/// puts in its bucket, and the solution an instance was made at.
///
/// A call binds each type-parameter slot by the **name** its map pairs with a solution: a frame
/// walks its callee's slots symbol-sorted, so a positional read would hand one variable another's
/// solution. The knot node points at this record rather than holding it: a slice or a second type
/// handle inline would widen every knot node in the program from 64 to 80 bytes.
#[derive(Clone, Copy)]
pub struct Typing<'cell> {
    quantifier_map: FunctionGroupMap<'cell>,
    registered: Option<Registered<'cell, KType>>,
    /// The solution in group order; empty for anything that is no instance.
    instance: &'cell [KType],
    /// The signature the body was checked against, where the function was born at an annotation
    /// its own type differs from.
    declared: Option<DeclaredType<KType>>,
    /// The body's record of a function born of code composed at run time, its root on it, which a
    /// copy lays again.
    composed: Option<&'cell BodyRef<'cell, 'cell>>,
    /// What a call of a function of an untraced body finds its composition by.
    composing: Option<Composing<'cell>>,
}

/// What a call of a function whose body is untraced code finds the code's composition by: the
/// digest of the code's syntax and the parameters it binds, and those parameters, the function's
/// own in its shape's channel order. The code itself is the function's first closure binding, the
/// record of what its declaring scope offers it the second.
#[derive(Clone, Copy)]
pub struct Composing<'cell> {
    pub digest: ContentDigest,
    pub parameters: &'cell [BinderSymbol],
}

impl<'cell> Typing<'cell> {
    /// `map`, `registered`, `instance`, `declared`, a body of composed code's record beside the
    /// root it is handed, and what an untraced body's call composes by, written into the region
    /// `writer` fills, or `None` where the function has none of them.
    pub(super) fn laid_down(
        writer: Writer<'cell, '_>,
        map: FunctionGroupMap<'_>,
        registered: Option<Registered<'_, KType>>,
        instance: &[KType],
        declared: Option<DeclaredType<KType>>,
        composed: Option<(&BodyRef<'_, '_>, Option<ComposedRoot<'cell>>)>,
        composing: Option<Composing<'_>>,
    ) -> Option<&'cell Typing<'cell>> {
        let empty = map.is_empty() && registered.is_none() && instance.is_empty();
        (!empty || declared.is_some() || composed.is_some() || composing.is_some()).then(|| {
            let quantifier_map = FunctionGroupMap(writer.fill(map.0.len(), |at| map.0[at]));
            let registered = registered.map(|registered| Registered {
                shape: registered.shape,
                quantifier_map: ShapeGroupMap(
                    writer.fill(registered.quantifier_map.0.len(), |at| {
                        registered.quantifier_map.0[at]
                    }),
                ),
                parameters: match registered.parameters {
                    ParameterBinding::Named(names) => {
                        ParameterBinding::Named(writer.fill(names.len(), |at| names[at]))
                    }
                    ParameterBinding::Operands => ParameterBinding::Operands,
                },
            });
            resident(
                writer,
                Typing {
                    quantifier_map,
                    registered,
                    instance: writer.fill(instance.len(), |at| instance[at]),
                    declared,
                    composed: composed.map(|(body, root)| body.laid(writer, root)),
                    composing: composing.map(|composing| Composing {
                        parameters: writer
                            .fill(composing.parameters.len(), |at| composing.parameters[at]),
                        ..composing
                    }),
                },
            )
        })
    }

    /// What laying the record down costs a rebuild: the record and its runs.
    pub(super) fn weight(
        len: usize,
        registered: Option<&Registered<'_, KType>>,
        instance: usize,
        declared: bool,
        composed: Option<&BodyRef<'_, '_>>,
        composing: Option<&Composing<'_>>,
    ) -> Weight {
        if len == 0
            && registered.is_none()
            && instance == 0
            && !declared
            && composed.is_none()
            && composing.is_none()
        {
            return Weight::ZERO;
        }
        let composing = composing.map_or(Weight::ZERO, |composing| {
            Weight::run::<BinderSymbol>(composing.parameters.len())
        });
        let composed = composed.map_or(Weight::ZERO, BodyRef::weight);
        let (map, names) = registered.map_or((0, 0), |registered| {
            let names = match registered.parameters {
                ParameterBinding::Named(names) => names.len(),
                ParameterBinding::Operands => 0,
            };
            (registered.quantifier_map.0.len(), names)
        });
        Weight::run::<(TypeSymbol, usize)>(len)
            .plus(Weight::run::<(TypeSymbol, usize)>(map))
            .plus(Weight::run::<BinderSymbol>(names))
            .plus(Weight::run::<KType>(instance))
            .plus(Weight::flat::<Typing<'_>>())
            .plus(composed)
            .plus(composing)
    }
}

/// A function node, read and not yet written.
pub(super) struct Staged<'graph, 'cell, 'x> {
    /// Its body, a composed record's copy in scratch until the tie lays the node, with no root.
    pub body: BodyAt<'graph, 'static, 'x>,
    /// The root of the composed code the body lies in, at the region's lifetime, which the scratch
    /// record does not hold.
    pub root: Option<ComposedRoot<'cell>>,
    pub ktype: DeclaredType<KType>,
    /// The name-keyed quantifier map the elaborator handed back, scratch-lived until the tie lays
    /// it into the region.
    pub quantifier_map: FunctionGroupMap<'x>,
    /// What the elaborator built for the registration the function is born for.
    pub registered: Option<Registered<'x, KType>>,
    /// The solution a quantified function is born instantiated at, bound where it is born; empty
    /// where it is born as it is.
    pub instance: &'x [KType],
    /// The signature the body was checked against, where the node is born at an annotation that
    /// retypes it: `ktype` is then the annotation's retype.
    pub declared: Option<DeclaredType<KType>>,
    pub captures: BumpVec<'x, Link<'cell, Knotted<'graph, 'cell>>>,
    /// What the activation the function is born in had decided, which its closure keeps.
    pub decided: Option<&'cell Activated<'cell>>,
    /// An untraced body's code, beside its body part's site in the shape it is born in and the
    /// parameters its composition binds.
    pub untraced: Option<(Knotted<'graph, 'cell>, Site, &'x [BinderSymbol])>,
}

impl<'graph, 'cell> Staged<'graph, 'cell, '_> {
    /// The load's record of its body, which the function holds itself: `None` for a body of
    /// composed code, whose record [`laid_down`](Self::laid_down) copies into its closure.
    pub(super) fn written_body(&self) -> Option<&'graph BodyRef<'graph, 'graph>> {
        match self.body {
            BodyAt::Written(body) => Some(body),
            BodyAt::Composed(_) => None,
        }
    }

    /// This node's closure run, born under the arena node `build`, and typing record laid down in
    /// `writer`'s region, beside what the two add to the knot's weight. A plain unquantified `FN` has
    /// no typing record and writes none. An untraced body's closure is its code, laid flat, then
    /// the record of what `activation`, the activation it is born in, offers it.
    pub(super) fn laid_down(
        &self,
        writer: Writer<'cell, '_>,
        born: (ArenaRef, Option<Stamp<'cell, Knotted<'graph, 'cell>>>),
        activation: &KReading<'_, 'graph, 'cell>,
        types: &Chain<'_, 'cell, '_, '_>,
    ) -> (
        &'cell ClosureBindings<'cell, Knotted<'graph, 'cell>>,
        Option<&'cell Typing<'cell>>,
        Weight,
    ) {
        let composed = match self.body {
            BodyAt::Written(_) => None,
            BodyAt::Composed(body) => Some(body),
        };
        let (closure, composing) = match self.untraced {
            None => (
                ClosureBindings::of(writer, &self.captures, born, self.decided),
                None,
            ),
            Some((code, site, parameters)) => {
                let scratch = Bump::new();
                let code = flattened(writer, code, types);
                let syntax =
                    (code.code().and_then(|node| node.body())).expect("flat code has a body");
                let shape = activation.shape();
                let links = [
                    Link::Value(Value::Knotted(code)),
                    Link::Value(offered(
                        writer,
                        activation,
                        shape.offers(site),
                        types,
                        &scratch,
                    )),
                ];
                let composing = Composing {
                    digest: composition_digest(syntax, parameters),
                    parameters,
                };
                (
                    ClosureBindings::of(writer, &links, born, self.decided),
                    Some(composing),
                )
            }
        };
        let typing = Typing::laid_down(
            writer,
            self.quantifier_map,
            self.registered,
            self.instance,
            self.declared,
            composed.map(|body| (body, self.root)),
            composing,
        );
        let weight = closure.weight().plus(Typing::weight(
            self.quantifier_map.0.len(),
            self.registered.as_ref(),
            self.instance.len(),
            self.declared.is_some(),
            composed,
            composing.as_ref(),
        ));
        (closure, typing, weight)
    }
}

/// Read the callable of `body` into `scratch`: its type as the load, or a layer `activation`
/// reads, fixed it, born for `registration` or for none, and its captures read through
/// `activation`, each `Member` source minted by `edge`. The function keeps its body's record as
/// `written` holds it. An untraced body is born over `code`, what its body part evaluated to, which
/// is `NotCode` where it is no code.
#[allow(clippy::too_many_arguments)]
pub(super) fn staged<'graph, 'cell, 'x>(
    body: &BodyShape<'_>,
    registration: Option<&Registration<'_>>,
    activation: &KReading<'_, 'graph, 'cell>,
    written: Written<'graph, 'cell, '_>,
    code: Option<KValue<'graph, 'cell>>,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'x>,
    edge: impl FnMut(u32) -> Edge,
) -> Result<Staged<'graph, 'cell, 'x>, Untieable<'x>>
where
    'graph: 'x,
{
    let untraced = match body.deferred() {
        false => None,
        true => {
            let code = code.expect("an untraced body is born over its code");
            let Some(member) = code.as_code() else {
                return Err(Untieable::NotCode {
                    value: code.concrete_ktype(),
                });
            };
            let shape = activation.shape();
            let form = body.form().expect("a callable's body records its form");
            let site = shape
                .site_of_body(form)
                .expect("a callable's form has a body");
            let mut parameters = BumpVec::new_in(scratch);
            parameters.extend(
                (0..body.slots() as u32)
                    .map(Slot)
                    .filter(|slot| body.position_of(*slot) == Position::PARAMETER)
                    .map(|slot| body.slot_name(slot)),
            );
            Some((member, site, &*parameters.leak()))
        }
    };
    let callable =
        loaded(body, registration, activation, types, scratch).map_err(Untieable::Type)?;
    // A body whose group the load solved where it is wanted is born as that instance.
    let facts = activation.facts();
    let (ktype, instance) = match (facts.born_instance(types, body), callable.ktype) {
        (Some(instance), DeclaredType::Scheme(scheme)) => {
            debug_assert!(registration.is_none(), "a registration binds its scheme");
            let (solution, instance) = solved(instance, scratch);
            debug_assert!(instantiates(types, scratch, scheme, solution, instance));
            (DeclaredType::Type(instance), solution)
        }
        (None, ktype) => (ktype, &[][..]),
        (Some(_), DeclaredType::Type(_)) => unreachable!("only a quantified body is instantiated"),
    };
    let captures = ClosureBindings::read_captures(body, activation, scratch, edge);
    Ok(Staged {
        body: written.unrooted(scratch, body.segment()),
        root: written.root(),
        ktype,
        quantifier_map: callable.quantifier_map,
        registered: callable.registered,
        instance,
        declared: None,
        captures,
        decided: activation.view().decided(),
        untraced,
    })
}

#[cfg(test)]
thread_local! {
    /// How many types the run instantiated from a scheme: a barrier's declared instance alone.
    pub(crate) static INSTANTIATED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Whether `instance`, the type the load recorded for an instance of `scheme` at `solution`, is the
/// scheme instantiated there. Not gated on `debug_assertions`: its callers are `debug_assert!`s.
fn instantiates(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    scheme: Scheme,
    solution: &[KType],
    instance: KType,
) -> bool {
    types.concrete(instantiate_quantified(types, scratch, scheme, solution)) == Some(instance)
}

/// `instance`, as the load or a layer the site reads recorded it: its solution in group order and
/// its type. A rigid one never reaches the run: the layer binding its variables closes it.
fn solved<'x>(instance: StaticInstance<'_>, scratch: BumpAllocator<'x>) -> (&'x [KType], KType) {
    match instance {
        StaticInstance::Closed { solution, ktype } => (scratch.alloc_slice_copy(solution), ktype),
        StaticInstance::Rigid { .. } => unreachable!("an instance is closed where it is born"),
    }
}

/// A callable's fact where it is born: fixed, or refused at every run.
fn closed<T>(fact: AtRun<T>) -> Result<T, Elaboration> {
    match fact {
        AtRun::Closed(closed) => Ok(closed),
        AtRun::Refused(refused) => Err(refused),
        AtRun::Open => unreachable!("a callable's type is fixed where it is born"),
    }
}

/// The type the load, or a layer `activation` reads, fixed for the callable of `body`, born for
/// `registration` or for none, resolving in `types`, its runs copied into `scratch`; or why every
/// run refuses it.
fn loaded<'x>(
    body: &BodyShape<'_>,
    registration: Option<&Registration<'_>>,
    activation: &KReading<'_, '_, '_>,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'x>,
) -> Result<Callable<'x, KType>, Elaboration> {
    let (facts, shape) = (activation.facts(), activation.shape());
    let form = body.form().expect("a callable body sits in its form");
    let site = shape.site_of_node(form);
    let callable = closed(facts.at_run(types, shape, site, facts.callable_type(types, body)))?;
    let registered = match registration {
        Some(registration) => {
            let fact = facts.registered_type(types, shape, registration.slot);
            let registered = closed(facts.at_run(types, shape, site, fact))?;
            Some(Registered {
                shape: registered.shape,
                quantifier_map: ShapeGroupMap(
                    scratch.alloc_slice_copy(registered.quantifier_map.0),
                ),
                parameters: match registered.parameters {
                    ParameterBinding::Named(names) => {
                        ParameterBinding::Named(scratch.alloc_slice_copy(names))
                    }
                    ParameterBinding::Operands => ParameterBinding::Operands,
                },
            })
        }
        None => None,
    };
    Ok(Callable {
        ktype: callable.ktype,
        quantifier_map: FunctionGroupMap(scratch.alloc_slice_copy(callable.quantifier_map.0)),
        registered,
    })
}

/// A callable body a knot node runs, beside the registration it is born for.
pub(super) type Born<'step> = (&'step BodyShape<'step>, Option<&'step Registration<'step>>);

/// Read every function node into `scratch`, by node index — `bodies[i]` is node `i`'s callable
/// body beside the registration it is born for, `None` for a data node — each capture of a fellow
/// member minted as an edge through `plan`. An untraced body is born over `codes[i]`.
pub(super) fn stage<'graph, 'cell, 'x>(
    plan: &KnotPlan,
    activation: &KReading<'_, 'graph, 'cell>,
    written: Written<'graph, 'cell, '_>,
    bodies: &[Option<Born<'_>>],
    codes: &[Option<KValue<'graph, 'cell>>],
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'x>,
) -> Result<BumpVec<'x, Option<Staged<'graph, 'cell, 'x>>>, Untieable<'x>>
where
    'graph: 'x,
{
    let mut staged = BumpVec::with_capacity_in(bodies.len(), scratch);
    for (body, code) in bodies.iter().zip(codes) {
        staged.push(match body {
            Some((body, registration)) => Some(self::staged(
                body,
                *registration,
                activation,
                written,
                *code,
                types,
                scratch,
                |index| {
                    plan.edge(index)
                        .expect("a member index is below the knot's count")
                },
            )?),
            None => None,
        });
    }
    Ok(staged)
}

/// The lambda door: birth the callable whose body `activation`'s shape holds at `site` — a `FN` no
/// binder names, found by [`Site::of_body`] — as a one-node knot in `writer`'s region. Its type is
/// read as the tie's is, and its captures are read through `activation`, which finds every one
/// bound because the body runner performs a statement after every unit it reads. A signature the
/// load left unknown that does not elaborate refuses `Type`, and writes nothing.
///
/// A callable that captures a fellow member is born inside its binder's knot by the tie, which
/// never asks for it, so no capture here is an edge. It is born under the arena node `build`, and
/// keeps its body's record as `written` holds it. An untraced body is born over `code`, what the
/// caller evaluated its body part to.
#[allow(clippy::too_many_arguments)]
pub fn lambda<'graph, 'cell, 'x>(
    writer: Writer<'cell, '_>,
    activation: &KReading<'_, 'graph, 'cell>,
    written: Written<'graph, 'cell, '_>,
    site: Site,
    code: Option<KValue<'graph, 'cell>>,
    born: (ArenaRef, Option<Stamp<'cell, Knotted<'graph, 'cell>>>),
    types: &Chain<'_, 'cell, '_, '_>,
    scratch: BumpAllocator<'x>,
) -> Result<Knotted<'graph, 'cell>, Untieable<'x>>
where
    'graph: 'x,
{
    let body = activation
        .shape()
        .nested(site)
        .filter(|body| body.kind() == ShapeKind::Callable)
        .expect("a lambda is born at the site of a callable body its shape holds");
    let staged = staged(
        body,
        None,
        activation,
        written,
        code,
        types,
        scratch,
        |_| unreachable!("only the tie births a callable that captures a fellow member"),
    )?;
    let (closure, typing, weight) = staged.laid_down(writer, born, activation, types);
    let knot_weight = Weight::flat::<usize>()
        .plus(weight)
        .plus(Weight::flat::<Node<'graph, 'cell>>());
    let knot = KnotPlan::new(1).tie(writer, |_| {
        Node::Function(Function::new(
            staged.ktype,
            typing,
            staged.written_body(),
            closure,
            knot_weight,
        ))
    });
    Ok(Knotted::of(super::stamped(knot, types), 0))
}

/// `member`, a quantified function behind an opaque view's barrier, read where the load solved its
/// group to `solution`: a barrier of its own at the instance — what a caller sees, and the types its
/// arguments cross at — before the same function, which solves its own group where it runs.
pub fn barrier_instance<'graph, 'cell>(
    writer: Writer<'cell, '_>,
    member: Knotted<'graph, 'cell>,
    instance: StaticInstance<'_>,
    types: &Chain<'_, 'cell, '_, '_>,
    scratch: BumpAllocator<'_>,
) -> Knotted<'graph, 'cell> {
    let (solution, instance) = solved(instance, scratch);
    let barrier = member
        .coerced()
        .expect("a barrier instance is read of a member behind a barrier");
    let (DeclaredType::Scheme(scheme), DeclaredType::Scheme(declared)) =
        (barrier.ktype(), barrier.declared())
    else {
        unreachable!("a barrier instance is read of a quantified member")
    };
    debug_assert!(instantiates(types, scratch, scheme, solution, instance));
    let ktype = DeclaredType::Type(instance);
    // The declared instance is in the view's terms, which no record holds: instantiated here.
    #[cfg(test)]
    INSTANTIATED.with(|count| count.set(count.get() + 1));
    let declared = DeclaredType::Type(instantiate_quantified(types, scratch, declared, solution));
    let knot = Coerced::tie(
        writer,
        barrier.underlying(),
        ktype,
        declared,
        barrier.from(),
        barrier.to(),
        types,
    );
    Knotted::of(knot, 0)
}

/// `header`, a function seen at a scheme other than its own — a member read through a signature
/// declaring it so — read where the load solved that scheme's group to `solution`: a header over
/// the same bare member at the scheme's instance. No knot is made: a call through it solves the
/// member's own group, as a call by name does.
pub fn header_instance<'graph, 'cell>(
    writer: Writer<'cell, '_>,
    header: &Retyped<Knotted<'graph, 'cell>>,
    instance: StaticInstance<'_>,
    types: &Chain<'_, 'cell, '_, '_>,
    scratch: BumpAllocator<'_>,
) -> KValue<'graph, 'cell> {
    let (solution, instance) = solved(instance, scratch);
    let DeclaredType::Scheme(scheme) = header.ktype() else {
        unreachable!("a header instance is read of a header at a scheme")
    };
    debug_assert!(instantiates(types, scratch, scheme, solution, instance));
    let ktype = DeclaredType::Type(instance);
    Value::Retyped(Retyped::of(writer, header.member(), ktype))
}

/// `member`, a quantified function, read where the load solved its group to `solution`: a one-node
/// knot in `writer`'s region running the same body over the same captures, typed by its instance.
///
/// An edge in `member`'s closure is relative to its own knot, so each is rehomed as the sibling
/// value it names.
pub fn instance<'graph, 'cell>(
    writer: Writer<'cell, '_>,
    member: Knotted<'graph, 'cell>,
    instance: StaticInstance<'_>,
    types: &Chain<'_, 'cell, '_, '_>,
    scratch: BumpAllocator<'_>,
) -> Knotted<'graph, 'cell> {
    let (solution, instance) = solved(instance, scratch);
    let function = member
        .function()
        .expect("an instance is read of a function member");
    let DeclaredType::Scheme(scheme) = function.ktype() else {
        unreachable!("an instance is read of a quantified function")
    };
    debug_assert!(instantiates(types, scratch, scheme, solution, instance));
    let ktype = DeclaredType::Type(instance);
    let mut links = BumpVec::with_capacity_in(function.closure().len(), scratch);
    links.extend(function.closure().links().iter().map(|link| match link {
        Link::Edge(edge) => Link::Value(Value::Knotted(member.sibling(*edge))),
        Link::Value(value) => Link::Value(*value),
    }));
    // The same body over the same captures, so the same arena node.
    let closure = ClosureBindings::of(
        writer,
        &links,
        (
            function.closure().build(),
            function.closure().stamp().copied(),
        ),
        function.closure().decided(),
    );
    let map = function.quantifier_map();
    let typing = Typing::laid_down(
        writer,
        map,
        None,
        solution,
        None,
        function
            .body
            .is_none()
            .then(|| (function.body(), function.body().root)),
        function.composing(),
    );
    let knot_weight = Weight::flat::<usize>()
        .plus(closure.weight())
        .plus(Typing::weight(
            map.0.len(),
            None,
            solution.len(),
            false,
            function.body.is_none().then(|| function.body()),
            function.composing().as_ref(),
        ))
        .plus(Weight::flat::<Node<'graph, 'cell>>());
    let knot = KnotPlan::new(1).tie(writer, |_| {
        Node::Function(Function::new(
            ktype,
            typing,
            function.body,
            closure,
            knot_weight,
        ))
    });
    Knotted::of(super::stamped(knot, types), 0)
}

/// The record of the names and keys `offers` — an `EVAL`'s, or an untraced body's — offers the code
/// it runs, read through `activation` and built in `writer`'s region: a name's value, and a key as
/// the list of its functions, as a use at the key written there resolves it.
pub fn offered<'graph, 'cell>(
    writer: Writer<'cell, '_>,
    activation: &KReading<'_, 'graph, 'cell>,
    offers: &[(BinderSymbol, Offer<'_>)],
    types: &Chain<'_, 'cell, '_, '_>,
    scratch: &Bump,
) -> KValue<'graph, 'cell> {
    let mut fields = BumpVec::with_capacity_in(offers.len(), scratch);
    for (name, offer) in offers {
        let offered = match offer {
            Offer::Name(coordinate) => activation.read(*coordinate),
            Offer::Key(list) => {
                let mut functions = BumpVec::new_in(scratch);
                for candidate in list.candidates {
                    match candidate {
                        Candidate::One(coordinate) => functions.push(activation.read(*coordinate)),
                        Candidate::Spread(coordinate) => {
                            let spread = activation.read(*coordinate);
                            if spread.as_list().is_some()
                                && let Some(list) = spread.surface(types, scratch)
                            {
                                functions.extend(
                                    (0..list.len())
                                        .map(|at| list.child(at, types, scratch).value()),
                                );
                            }
                        }
                    }
                }
                Value::List(List::of_candidates(
                    writer,
                    functions.iter().copied(),
                    types,
                ))
            }
        };
        fields.push((*name, offered));
    }
    Value::Record(Record::new(writer, &fields, types, scratch))
}
