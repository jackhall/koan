//! A function as a knot node: its memoized type, the body shape it runs, the closure bindings a
//! call reads its captures through, the shape a registration puts in its bucket, and the weight of
//! the whole knot it sits in.
//!
//! Beside it, the staging a [tie](super::tie()) does for a function member. Everything a member
//! needs is read into scratch with no writer in reach — the body shape its binder births, its type
//! elaborated from the form that body sits in, and its captures, every mention of a fellow member
//! minted as an edge into the knot about to be tied — so a refusal writes nothing.

use crate::elaborate::{Canonical, callable_type};
use crate::memory::{BumpAllocator, BumpVec, KnotPlan, Writer, resident};
use crate::scope::{BodyShape, ClosureBindings, Component};
use crate::symbols::TypeSymbol;
use crate::type_lattice::{KType, TypeRegistry};
use crate::values::{Link, Weight};

use super::{KActivationView, Knotted, Untieable};

/// A function: what one knot node holds.
pub struct Function<'graph, 'cell, X> {
    ktype: KType,
    /// The quantifier map and registered shape, or `None` where the function has neither — an
    /// unquantified `FN`, almost every function, which costs nothing. Homed out of line for the
    /// reason [`Node::Coerced`] is.
    ///
    /// [`Node::Coerced`]: super::Node::Coerced
    typing: Option<&'cell Typing<'cell>>,
    shape: &'graph BodyShape<'graph>,
    closure: &'cell ClosureBindings<'graph, 'cell, X>,
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
        ktype: KType,
        typing: Option<&'cell Typing<'cell>>,
        shape: &'graph BodyShape<'graph>,
        closure: &'cell ClosureBindings<'graph, 'cell, X>,
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
    pub fn ktype(&self) -> KType {
        self.ktype
    }

    /// Where each `FOR ALL` name the declaration wrote landed in the canonical group, empty for
    /// an unquantified function.
    pub fn quantifier_map(&self) -> &'cell [(TypeSymbol, Canonical)] {
        self.typing.map_or(&[], |typing| typing.quantifier_map)
    }

    /// The expression shape this function's registration puts in its bucket, built from its type
    /// over the head where it was born; `None` for a `FN`, which no bucket holds.
    pub fn registered_shape(&self) -> Option<KType> {
        self.typing.and_then(|typing| typing.registered)
    }

    /// Where the type parameter named `name` landed in the canonical group — its index, or its
    /// bound where canonical form dropped it — and `None` where the function binds no such name.
    ///
    /// Keyed by name because a frame walks its callee's slots **symbol-sorted**, not in the order
    /// the `FOR ALL` group was written.
    pub fn canonical_quantifier(&self, name: TypeSymbol) -> Option<Canonical> {
        self.quantifier_map()
            .iter()
            .find(|(declared, _)| *declared == name)
            .map(|(_, canonical)| *canonical)
    }

    /// The body shape a call activates.
    pub fn shape(&self) -> &'graph BodyShape<'graph> {
        self.shape
    }

    /// The closure bindings a call's activation reads its captures through.
    pub fn closure(&self) -> &'cell ClosureBindings<'graph, 'cell, X> {
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
        closure: &'to ClosureBindings<'graph, 'to, Y>,
    ) -> Function<'graph, 'to, Y> {
        Function {
            ktype: self.ktype,
            typing: Typing::laid_down(writer, self.quantifier_map(), self.registered_shape()),
            shape: self.shape,
            closure,
            knot_weight: self.knot_weight,
        }
    }
}

/// What a call and a selection read beside a function's type: its quantifier map, each `FOR ALL`
/// name the declaration wrote paired with its index in the canonical group or with its bound where
/// canonical form dropped it, and the expression shape its registration puts in its bucket.
///
/// A call binds each type-parameter slot by the **name** its map pairs with a solution: a frame
/// walks its callee's slots symbol-sorted, so a positional read would hand one variable another's
/// solution. The node points at this record rather than holding it: a slice or a second type
/// handle inline would widen every node in the program from 64 to 80 bytes.
#[derive(Clone, Copy)]
pub struct Typing<'cell> {
    quantifier_map: &'cell [(TypeSymbol, Canonical)],
    registered: Option<KType>,
}

impl<'cell> Typing<'cell> {
    /// `map` and `registered` written into the region `writer` fills, or `None` where the function
    /// has neither.
    pub(super) fn laid_down(
        writer: Writer<'cell>,
        map: &[(TypeSymbol, Canonical)],
        registered: Option<KType>,
    ) -> Option<&'cell Typing<'cell>> {
        (!map.is_empty() || registered.is_some()).then(|| {
            let quantifier_map = writer.fill(map.len(), |at| map[at]);
            resident(
                writer,
                Typing {
                    quantifier_map,
                    registered,
                },
            )
        })
    }

    /// What laying the record down costs a rebuild: the map's run, plus the record.
    pub(super) fn weight(len: usize, registered: bool) -> Weight {
        if len == 0 && !registered {
            return Weight::ZERO;
        }
        Weight::run::<(TypeSymbol, Canonical)>(len).plus(Weight::flat::<Typing<'_>>())
    }
}

/// A function member, read and not yet written.
pub(super) struct Staged<'graph, 'cell, 'x> {
    pub shape: &'graph BodyShape<'graph>,
    pub ktype: KType,
    /// The name-keyed quantifier map the elaborator handed back, scratch-lived until the tie lays
    /// it into the region.
    pub quantifier_map: &'x [(TypeSymbol, Canonical)],
    /// The shape the elaborator built for a registration.
    pub registered: Option<KType>,
    pub captures: BumpVec<'x, Link<'graph, 'cell, Knotted<'graph, 'cell>>>,
}

/// Read every function member of `component` into `scratch`, by member index; `None` for a data
/// member.
pub(super) fn stage<'graph, 'cell, 'x>(
    plan: &KnotPlan,
    activation: &KActivationView<'graph, 'cell>,
    component: &Component<'graph>,
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'x>,
) -> Result<BumpVec<'x, Option<Staged<'graph, 'cell, 'x>>>, Untieable<'x>> {
    let shape = activation.shape();
    let mut staged = BumpVec::with_capacity_in(component.members.len(), scratch);
    for slot in component.members {
        let Some(body) = shape.births(*slot) else {
            staged.push(None);
            continue;
        };
        let form = body.form().expect("a callable body sits in its form");
        let callable = callable_type(form, activation, types, scratch).map_err(Untieable::Type)?;
        let captures = ClosureBindings::read_captures(body, activation, scratch, |index| {
            plan.edge(index)
                .expect("a member index is below the knot's count")
        });
        staged.push(Some(Staged {
            shape: body,
            ktype: callable.ktype,
            quantifier_map: callable.quantifier_map,
            registered: callable.registered,
            captures,
        }));
    }
    Ok(staged)
}
