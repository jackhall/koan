//! A function as a knot node: its memoized type, the body shape it runs, the closure bindings a
//! call reads its captures through, and the weight of the whole knot it sits in.
//!
//! Beside it, the staging a [tie](super::tie()) does for a function member. Everything a member
//! needs is read into scratch with no writer in reach — the body shape its binder births, its type
//! elaborated from the form that body sits in, and its captures, every mention of a fellow member
//! minted as an edge into the knot about to be tied — so a refusal writes nothing.

use crate::elaborate::callable_type;
use crate::memory::{BumpAllocator, BumpVec, KnotPlan};
use crate::scope::{BodyShape, ClosureBindings, Component};
use crate::type_lattice::{KType, TypeRegistry};
use crate::values::{Link, Weight};

use super::{KActivationView, Knotted, Untieable};

/// A function: what one knot node holds.
pub struct Function<'graph, 'cell, X> {
    ktype: KType,
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
        shape: &'graph BodyShape<'graph>,
        closure: &'cell ClosureBindings<'graph, 'cell, X>,
        knot_weight: Weight,
    ) -> Self {
        Function {
            ktype,
            shape,
            closure,
            knot_weight,
        }
    }

    /// The function's type, elaborated from its signature where it was born.
    pub fn ktype(&self) -> KType {
        self.ktype
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
    /// the body shape and the knot weight ride over: a copy re-ties the same knot.
    pub(super) fn rebuilt<'to, Y>(
        &self,
        closure: &'to ClosureBindings<'graph, 'to, Y>,
    ) -> Function<'graph, 'to, Y> {
        Function {
            ktype: self.ktype,
            shape: self.shape,
            closure,
            knot_weight: self.knot_weight,
        }
    }
}

/// A function member, read and not yet written.
pub(super) struct Staged<'graph, 'cell, 'x> {
    pub shape: &'graph BodyShape<'graph>,
    pub ktype: KType,
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
        let ktype = callable_type(form, activation, types, scratch).map_err(Untieable::Type)?;
        let captures = ClosureBindings::read_captures(body, activation, scratch, |index| {
            plan.edge(index)
                .expect("a member index is below the knot's count")
        });
        staged.push(Some(Staged {
            shape: body,
            ktype,
            captures,
        }));
    }
    Ok(staged)
}
