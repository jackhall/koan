//! The readers of a module value: a named member, the registration run, the functions at a key.
//! Each reads the run by the layout order [`elaborate`'s members](crate::elaborate) spell — the
//! signature places every named member, and the registrations are the tail past them. A reader
//! finds a key's functions by their registered shapes ([`functions_at`]), never by position.

use crate::elaborate::{member_count, schema_member, schema_of};
use crate::knot::{KValue, Knotted};
use crate::memory::{BumpAllocator, BumpVec};
use crate::symbols::{BinderSymbol, KeySymbol};
use crate::type_lattice::{
    DeclaredType, DispatchTokenElement, KType, Parametric, TypeNode, TypeRegistry,
};

/// The registration members of `module` — the tail of its run past the named members, each a
/// function a keyworded use at its registered shape's key may select: one per registration of a
/// body-born module, and one per overload a view carries for a keyworded member of its signature.
/// Empty for anything that is no module.
pub fn registrations<'graph, 'cell>(
    module: Knotted<'graph, 'cell>,
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
) -> &'cell [KValue<'graph, 'cell>] {
    let Some((node, schema)) = module
        .module()
        .and_then(|node| Some((node, schema_of(node.ktype(), types)?)))
    else {
        return &[];
    };
    node.members()
        .get(member_count(&schema, scratch)..)
        .unwrap_or(&[])
}

/// The member of `module` named `name` — what `m.f` reads. `None` if `module` is no module, or
/// its signature does not name `name`.
pub fn member<'graph, 'cell>(
    module: Knotted<'graph, 'cell>,
    name: BinderSymbol,
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
) -> Option<KValue<'graph, 'cell>> {
    let node = module.module()?;
    let schema = schema_of(node.ktype(), types)?;
    let (index, _) = schema_member(&schema, scratch, name)?;
    node.members().get(index).copied()
}

/// The functions `module` offers at `key`: each registration member whose
/// registered shape ([`registered_shape`]) has that key.
pub fn functions_at<'graph, 'cell, 'x>(
    module: Knotted<'graph, 'cell>,
    key: KeySymbol,
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'x>,
) -> BumpVec<'x, KValue<'graph, 'cell>> {
    let mut found = BumpVec::new_in(scratch);
    for member in registrations(module, types, scratch) {
        let shape = registered_shape(*member).expect("a registration member is a function");
        if key_of(shape, types) == key {
            found.push(*member);
        }
    }
    found
}

/// The expression shape a registration member registers at: a function's own, or the shape a
/// barrier shows its caller. `None` for any other value.
pub fn registered_shape(member: KValue<'_, '_>) -> Option<DeclaredType<KType>> {
    let callable = member.as_callable()?;
    match (callable.function(), callable.coerced()) {
        (Some(function), _) => function.registered_shape(),
        (None, Some(barrier)) => Some(barrier.ktype()),
        (None, None) => None,
    }
}

/// The bucket key of the expression shape `shape`: its keywords in place, each slot erased. A
/// scheme's node spells its keys as a type's does.
pub fn key_of(shape: DeclaredType<impl Into<Parametric>>, types: &TypeRegistry<'_>) -> KeySymbol {
    let node = shape.node(types);
    let TypeNode::ExpressionShape { elements, .. } = node else {
        unreachable!("a registered shape is an expression shape")
    };
    KeySymbol::of(elements.iter().map(|element| match element {
        DispatchTokenElement::Keyword(keyword) => Some(keyword),
        DispatchTokenElement::Slot(_) => None,
    }))
}
