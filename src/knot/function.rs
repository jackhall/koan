//! A function as a knot node: its memoized type, the body shape it runs, the closure bindings a
//! call reads its captures through, the shape a registration puts in its bucket, and the weight of
//! the whole knot it sits in.
//!
//! Beside it, the staging a [tie](super::tie()) does for a function node. Everything a node needs
//! is read into scratch with no writer in reach — its body shape, its type elaborated from the form
//! that body sits in, and its captures, every mention of a fellow member minted as an edge into the
//! knot about to be tied — so a refusal writes nothing. The same staging and lay-down serve the
//! lambda door, [`lambda`], which births a callable no binder names as a one-node knot.
//!
//! A quantified function is made concrete where the load solved its group at the type it is wanted
//! at: born so, where its body shape holds the solution ([`BodyShape::born_instance`]), or read so
//! through [`instance`], a one-node knot over the same body and captures. Either way each lexical
//! variable the solution names is read where the site runs, the typing record carries the solution
//! so bound, and a frame binds the body's `FOR ALL` names from it.

use crate::elaborate::callable_type;
use crate::memory::{BumpAllocator, BumpVec, Edge, KnotPlan, Writer, resident};
use crate::scope::{BodyShape, ClosureBindings, Registration, ShapeKind, Site};
use crate::scope::{Callable, FunctionGroupMap, ParameterBinding, Registered, ShapeGroupMap};
use crate::scope::{Static, StaticSolution, solutions};
use crate::symbols::{BinderSymbol, TypeSymbol};
use crate::type_lattice::{
    DeclaredType, KType, Parametric, TypeRegistry, instantiate_quantified, substitute_levels,
};
use crate::values::{Knotted as _, Link, Value, Weight};

use super::{KActivationView, Knotted, Node, Untieable};

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
    shape: &'graph BodyShape<'graph>,
    closure: &'cell ClosureBindings<'cell, X>,
    /// What rebuilding the whole knot this node sits in writes, the same on every node.
    knot_weight: Weight,
}

impl<X> Clone for Function<'_, '_, X> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<X> Copy for Function<'_, '_, X> {}

impl<'graph, 'cell, X> Function<'graph, 'cell, X> {
    /// The function `ktype` runs through `shape` over `closure`, inside a knot weighing
    /// `knot_weight`. The private-field constructor the tie uses.
    pub(super) fn new(
        ktype: DeclaredType<KType>,
        typing: Option<&'cell Typing<'cell>>,
        shape: &'graph BodyShape<'graph>,
        closure: &'cell ClosureBindings<'cell, X>,
        knot_weight: Weight,
    ) -> Self {
        Function {
            ktype,
            typing,
            shape,
            closure,
            knot_weight,
        }
    }

    /// The function's type, elaborated from its signature where it was born.
    pub fn ktype(&self) -> DeclaredType<KType> {
        self.ktype
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

    /// The body shape a call activates.
    pub fn shape(&self) -> &'graph BodyShape<'graph> {
        self.shape
    }

    /// The closure bindings a call's activation reads its captures through.
    pub fn closure(&self) -> &'cell ClosureBindings<'cell, X> {
        self.closure
    }

    pub fn knot_weight(&self) -> Weight {
        self.knot_weight
    }

    /// This function over `closure` rebuilt at another region lifetime — the copy's arm. The type,
    /// the body shape and the knot weight ride over: a copy re-ties the same knot. The typing
    /// record is re-homed through `writer`, since it is a run in the source region.
    pub(super) fn rebuilt<'to, Y>(
        &self,
        writer: Writer<'to>,
        closure: &'to ClosureBindings<'to, Y>,
    ) -> Function<'graph, 'to, Y> {
        Function {
            ktype: self.ktype,
            typing: Typing::laid_down(
                writer,
                self.quantifier_map(),
                self.registered(),
                self.instance().unwrap_or(&[]),
            ),
            shape: self.shape,
            closure,
            knot_weight: self.knot_weight,
        }
    }
}

/// What a call and a selection read beside a function's type: its quantifier map, each `FOR ALL`
/// name the declaration wrote paired with its index in the type's group, what its registration
/// puts in its bucket, and the solution an instance was made at.
///
/// A call binds each type-parameter slot by the **name** its map pairs with a solution: a frame
/// walks its callee's slots symbol-sorted, so a positional read would hand one variable another's
/// solution. The node points at this record rather than holding it: a slice or a second type
/// handle inline would widen every node in the program from 64 to 80 bytes.
#[derive(Clone, Copy)]
pub struct Typing<'cell> {
    quantifier_map: FunctionGroupMap<'cell>,
    registered: Option<Registered<'cell, KType>>,
    /// The solution in group order; empty for anything that is no instance.
    instance: &'cell [KType],
}

impl<'cell> Typing<'cell> {
    /// `map`, `registered` and `instance` written into the region `writer` fills, or `None` where
    /// the function has none of them.
    pub(super) fn laid_down(
        writer: Writer<'cell>,
        map: FunctionGroupMap<'_>,
        registered: Option<Registered<'_, KType>>,
        instance: &[KType],
    ) -> Option<&'cell Typing<'cell>> {
        (!map.is_empty() || registered.is_some() || !instance.is_empty()).then(|| {
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
                },
            )
        })
    }

    /// What laying the record down costs a rebuild: the record and its runs.
    pub(super) fn weight(
        len: usize,
        registered: Option<&Registered<'_, KType>>,
        instance: usize,
    ) -> Weight {
        if len == 0 && registered.is_none() && instance == 0 {
            return Weight::ZERO;
        }
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
    }
}

/// A function node, read and not yet written.
pub(super) struct Staged<'graph, 'cell, 'x> {
    pub shape: &'graph BodyShape<'graph>,
    pub ktype: DeclaredType<KType>,
    /// The name-keyed quantifier map the elaborator handed back, scratch-lived until the tie lays
    /// it into the region.
    pub quantifier_map: FunctionGroupMap<'x>,
    /// What the elaborator built for the registration the function is born for.
    pub registered: Option<Registered<'x, KType>>,
    /// The solution a quantified function is born instantiated at, bound where it is born; empty
    /// where it is born as it is.
    pub instance: &'x [KType],
    pub captures: BumpVec<'x, Link<'cell, Knotted<'graph, 'cell>>>,
}

impl<'graph, 'cell> Staged<'graph, 'cell, '_> {
    /// This node's closure run and typing record laid down in `writer`'s region, beside what the two
    /// add to the knot's weight. A plain unquantified `FN` has no typing record and writes none.
    pub(super) fn laid_down(
        &self,
        writer: Writer<'cell>,
    ) -> (
        &'cell ClosureBindings<'cell, Knotted<'graph, 'cell>>,
        Option<&'cell Typing<'cell>>,
        Weight,
    ) {
        let closure = ClosureBindings::of(writer, &self.captures);
        let typing = Typing::laid_down(writer, self.quantifier_map, self.registered, self.instance);
        let weight = closure.weight().plus(Typing::weight(
            self.quantifier_map.0.len(),
            self.registered.as_ref(),
            self.instance.len(),
        ));
        (closure, typing, weight)
    }
}

/// Read the callable of `body` into `scratch`: its type as the load fixed it, or — where the load
/// left it unknown — elaborated from the declaration it sits in, born for `registration` or for
/// none, and its captures read through `activation`, each `Member` source minted by `edge`.
pub(super) fn staged<'graph, 'cell, 'x>(
    body: &'graph BodyShape<'graph>,
    registration: Option<&Registration<'graph>>,
    activation: &KActivationView<'graph, 'cell>,
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'x>,
    edge: impl FnMut(u32) -> Edge,
) -> Result<Staged<'graph, 'cell, 'x>, Untieable<'x>> {
    let form = body.form().expect("a callable body sits in its form");
    let elaborated = || {
        callable_type(form, activation, types, scratch, registration)
            .map(|callable| born(types, callable))
    };
    let callable = match loaded(body, registration, activation, types, scratch) {
        Some(callable) => {
            debug_assert_eq!(
                elaborated().ok(),
                Some(callable),
                "the load-time type agrees with elaborating where the callable is born"
            );
            callable
        }
        None => elaborated().map_err(Untieable::Type)?,
    };
    // A body whose group the load solved where it is wanted is born as that instance.
    let (ktype, instance) = match (body.born_instance(), callable.ktype) {
        (Some(solution), DeclaredType::Scheme(scheme)) => {
            debug_assert!(registration.is_none(), "a registration binds its scheme");
            let solution = solved(solution, activation, types, scratch);
            let instance = instantiate_quantified(types, scratch, scheme, solution);
            let instance = types.concrete(instance).expect(INSTANCE);
            (DeclaredType::Type(instance), solution)
        }
        (None, ktype) => (ktype, &[][..]),
        (Some(_), DeclaredType::Type(_)) => unreachable!("only a quantified body is instantiated"),
    };
    let captures = ClosureBindings::read_captures(body, activation, scratch, edge);
    Ok(Staged {
        shape: body,
        ktype,
        quantifier_map: callable.quantifier_map,
        registered: callable.registered,
        instance,
        captures,
    })
}

/// Why an instance's type is concrete: the scheme it instantiates is born with its levels bound,
/// and its solution is bound where it runs.
const INSTANCE: &str = "a born scheme instantiated at a solution solved where it runs is concrete";

/// Why an instance's variables are bound where it is read or born: the wanted type names each of
/// them, and the run reads that type at or before the site.
const BOUND: &str = "an instance's lexical variables are bound where it is read or born";

/// `solution`, an instance's solution in group order as the load recorded it, where the site runs:
/// as it is when closed, and with each lexical variable it names replaced by the type `activation`
/// reads at its coordinate when rigid.
fn solved<'x>(
    solution: StaticSolution<'_>,
    activation: &KActivationView<'_, '_>,
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'x>,
) -> &'x [KType] {
    match solution {
        Static::Closed(solution) => scratch.alloc_slice_copy(solution),
        Static::Rigid { value, variables } => {
            let bindings = solutions(variables, activation, scratch).expect(BOUND);
            let mut solved = BumpVec::with_capacity_in(value.len(), scratch);
            solved.extend(value.iter().map(|each| {
                let each = substitute_levels(types, scratch, *each, &bindings);
                types.concrete(each).expect(BOUND)
            }));
            solved.leak()
        }
        Static::Unknown => unreachable!("the load fixes every instance's solution"),
    }
}

/// Why a callable's type is concrete outside its own group where it is born: every lexical variable
/// its declaration read is bound there.
const BORN: &str = "a callable's type holds no variable once its levels are bound";

/// `declared` where every level is bound: a scheme as it is, and a type narrowed to concrete.
fn concrete_declared(
    types: &TypeRegistry<'_>,
    declared: DeclaredType<Parametric>,
) -> DeclaredType<KType> {
    match declared {
        DeclaredType::Type(kt) => DeclaredType::Type(types.concrete(kt).expect(BORN)),
        DeclaredType::Scheme(scheme) => DeclaredType::Scheme(scheme),
    }
}

/// `callable` as a function is born with it, every level its declaration read bound.
fn born<'x>(types: &TypeRegistry<'_>, callable: Callable<'x, Parametric>) -> Callable<'x, KType> {
    Callable {
        ktype: concrete_declared(types, callable.ktype),
        quantifier_map: callable.quantifier_map,
        registered: callable.registered.map(|registered| Registered {
            shape: concrete_declared(types, registered.shape),
            quantifier_map: registered.quantifier_map,
            parameters: registered.parameters,
        }),
    }
}

/// The type the load fixed for the callable of `body`, born for `registration` or for none, read
/// through `activation`: as it is when closed, and with its variables substituted when rigid, its
/// runs copied into `scratch` as an elaborated type's are. `None` where the load left the callable
/// or its registration unknown.
fn loaded<'graph, 'x>(
    body: &'graph BodyShape<'graph>,
    registration: Option<&Registration<'graph>>,
    activation: &KActivationView<'graph, '_>,
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'x>,
) -> Option<Callable<'x, KType>> {
    let substitute = |value: DeclaredType<Parametric>, bindings: &[KType]| {
        concrete_declared(types, substitute_levels(types, scratch, value, bindings))
    };
    let callable = body
        .callable_type()
        .solved(activation, scratch, |callable, bindings| {
            Some(Callable {
                ktype: substitute(callable.ktype, bindings),
                quantifier_map: callable.quantifier_map,
                registered: None,
            })
        })?;
    let registered = match registration {
        Some(registration) => {
            let registered = activation
                .shape()
                .registered_type(registration.slot)
                .solved(activation, scratch, |registered, bindings| {
                    Some(Registered {
                        shape: substitute(registered.shape, bindings),
                        quantifier_map: registered.quantifier_map,
                        parameters: registered.parameters,
                    })
                })?;
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
    Some(Callable {
        ktype: callable.ktype,
        quantifier_map: FunctionGroupMap(scratch.alloc_slice_copy(callable.quantifier_map.0)),
        registered,
    })
}

/// A callable body a knot node runs, beside the registration it is born for.
pub(super) type Born<'graph> = (
    &'graph BodyShape<'graph>,
    Option<&'graph Registration<'graph>>,
);

/// Read every function node into `scratch`, by node index — `bodies[i]` is node `i`'s callable
/// body beside the registration it is born for, `None` for a data node — each capture of a fellow
/// member minted as an edge through `plan`.
pub(super) fn stage<'graph, 'cell, 'x>(
    plan: &KnotPlan,
    activation: &KActivationView<'graph, 'cell>,
    bodies: &[Option<Born<'graph>>],
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'x>,
) -> Result<BumpVec<'x, Option<Staged<'graph, 'cell, 'x>>>, Untieable<'x>> {
    let mut staged = BumpVec::with_capacity_in(bodies.len(), scratch);
    for body in bodies {
        staged.push(match body {
            Some((body, registration)) => Some(self::staged(
                body,
                *registration,
                activation,
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
/// never asks for it, so no capture here is an edge.
pub fn lambda<'graph, 'cell, 'x>(
    writer: Writer<'cell>,
    activation: &KActivationView<'graph, 'cell>,
    site: Site,
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'x>,
) -> Result<Knotted<'graph, 'cell>, Untieable<'x>> {
    let body = activation
        .shape()
        .nested(site)
        .filter(|body| body.kind() == ShapeKind::Callable)
        .expect("a lambda is born at the site of a callable body its shape holds");
    let staged = staged(body, None, activation, types, scratch, |_| {
        unreachable!("only the tie births a callable that captures a fellow member")
    })?;
    let (closure, typing, weight) = staged.laid_down(writer);
    let knot_weight = Weight::flat::<usize>()
        .plus(weight)
        .plus(Weight::flat::<Node<'graph, 'cell>>());
    let knot = KnotPlan::new(1).tie(writer, |_| {
        Node::Function(Function::new(
            staged.ktype,
            typing,
            staged.shape,
            closure,
            knot_weight,
        ))
    });
    Ok(Knotted::of(knot, 0))
}

/// `member`, a quantified function, read where the load solved its group to `solution`: a one-node
/// knot in `writer`'s region running the same body over the same captures, typed by its instance.
/// Each lexical variable `solution` names is read through `activation`, the activation the site
/// runs in.
///
/// An edge in `member`'s closure is relative to its own knot, so each is rehomed as the sibling
/// value it names.
pub fn instance<'graph, 'cell>(
    writer: Writer<'cell>,
    member: Knotted<'graph, 'cell>,
    solution: StaticSolution<'graph>,
    activation: &KActivationView<'graph, '_>,
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
) -> Knotted<'graph, 'cell> {
    let solution = solved(solution, activation, types, scratch);
    let function = member
        .function()
        .expect("an instance is read of a function member");
    let DeclaredType::Scheme(scheme) = function.ktype() else {
        unreachable!("an instance is read of a quantified function")
    };
    let instance = instantiate_quantified(types, scratch, scheme, solution);
    let ktype = DeclaredType::Type(types.concrete(instance).expect(INSTANCE));
    let mut links = BumpVec::with_capacity_in(function.closure().len(), scratch);
    links.extend(function.closure().links().iter().map(|link| match link {
        Link::Edge(edge) => Link::Value(Value::Knotted(member.sibling(*edge))),
        Link::Value(value) => Link::Value(*value),
    }));
    let closure = ClosureBindings::of(writer, &links);
    let map = function.quantifier_map();
    let typing = Typing::laid_down(writer, map, None, solution);
    let knot_weight = Weight::flat::<usize>()
        .plus(closure.weight())
        .plus(Typing::weight(map.0.len(), None, solution.len()))
        .plus(Weight::flat::<Node<'graph, 'cell>>());
    let knot = KnotPlan::new(1).tie(writer, |_| {
        Node::Function(Function::new(
            ktype,
            typing,
            function.shape(),
            closure,
            knot_weight,
        ))
    });
    Knotted::of(knot, 0)
}
