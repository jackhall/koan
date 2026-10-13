//! The readers of a module value: a named member, the registration run, the functions at a key.
//! Each reads the run by the layout order [`elaborate`'s members](crate::elaborate) spell — the
//! signature places every named member, and the registrations are the tail past them. A reader
//! finds a key's functions by their registered shapes ([`functions_at`]), never by position.
//!
//! Through a header, a reader sees what the carried signature declares: a named member retyped to
//! its declared type, and at a key, per keyworded member, the most specific overloads covering it
//! ([`covering`]), each retyped to its shape. A view's build makes the same pick.

use crate::elaborate::{member_count, schema_member, schema_of, signature_member, type_members};
use crate::knot::{KValue, Knotted};
use crate::memory::{BumpAllocator, BumpVec, Writer};
use crate::symbols::{BinderSymbol, KeySymbol};
use crate::type_lattice::{
    DeclaredType, DispatchTokenElement, KType, Members, Parametric, TypeNode, satisfied_by,
    select_by_class, substitute_parameters,
};
use crate::values::{Seen, TypeValue, Types, Value, applications};

/// The registration members of `module` — the tail of its run past the named members, each a
/// function a keyworded use at its registered shape's key may select: one per registration of a
/// body-born module, and one per overload a view carries for a keyworded member of its signature.
/// Empty for anything that is no module.
pub fn registrations<'graph, 'cell>(
    module: KValue<'graph, 'cell>,
    types: &Types<'_, '_>,
    scratch: BumpAllocator<'_>,
) -> &'cell [KValue<'graph, 'cell>] {
    let Some((node, schema)) = (module.knot_member())
        .and_then(Knotted::module)
        .and_then(|node| Some((node, schema_of(node.ktype(), types)?)))
    else {
        return &[];
    };
    node.members()
        .get(member_count(&schema, scratch)..)
        .unwrap_or(&[])
}

/// The member of `module` named `name` — what `m.f` reads — at the type the signature `module`
/// carries declares it: a header's application, or a bare module's own signature. A value member
/// is retyped to it in `writer`'s region, a scheme included; a type member is a type value of it,
/// which the node itself need not bind. `None` if `module` is no module, or its type does not
/// name `name`.
pub fn member<'graph, 'cell>(
    writer: Writer<'cell>,
    module: KValue<'graph, 'cell>,
    name: BinderSymbol,
    types: &Types<'_, 'cell>,
    scratch: BumpAllocator<'_>,
) -> Option<KValue<'graph, 'cell>> {
    let node = module.knot_member()?.module()?;
    let read = signature_member(types, scratch, module.ktype().as_type()?.into(), name)?;
    if let BinderSymbol::Type(_) = name {
        let declared = types.concrete(read.declared.as_type()?)?;
        return Some(Value::Type(TypeValue::new(writer, declared, types)));
    }
    // A module carries a signature pinning every head parameter, so what it declares is closed.
    let declared = match read.declared {
        DeclaredType::Type(declared) => DeclaredType::Type(types.concrete(declared)?),
        DeclaredType::Scheme(scheme) => DeclaredType::Scheme(scheme),
    };
    let own = schema_of(node.ktype(), types)?;
    let (index, _) = schema_member(&own, scratch, name)?;
    let held = *node.members().get(index)?;
    Some(
        Seen::of(held)
            .seen_at(declared, types, scratch)
            .restamped(writer),
    )
}

/// How many named members a module carrying `carried` shows: each name an application `carried`
/// holds declares, once.
pub fn named_count(carried: KType, types: &Types<'_, '_>, scratch: BumpAllocator<'_>) -> usize {
    let mut names = BumpVec::new_in(scratch);
    for application in applications(types, scratch, carried) {
        let signature = match types.node(application) {
            TypeNode::SignatureApply { signature, .. } => signature,
            _ => application,
        };
        let Some(schema) = schema_of(signature, types) else {
            continue;
        };
        names.extend(
            schema
                .value_slots
                .iter()
                .map(|(name, _)| BinderSymbol::Value(*name)),
        );
        names.extend(
            (type_members(&schema, scratch).iter()).map(|(name, _)| BinderSymbol::Type(*name)),
        );
    }
    names.sort_unstable();
    names.dedup();
    names.len()
}

/// The functions `module` offers at `key`. A bare module offers each registration member whose
/// registered shape ([`registered_shape`]) has that key. Through a header, each keyworded member
/// the carried signature declares at `key` offers the module's overloads [`covering`] it, each
/// retyped in `writer`'s region to the member's declared shape.
pub fn functions_at<'graph, 'cell, 'x>(
    writer: Writer<'cell>,
    module: KValue<'graph, 'cell>,
    key: KeySymbol,
    types: &Types<'_, 'cell>,
    scratch: BumpAllocator<'x>,
) -> BumpVec<'x, KValue<'graph, 'cell>> {
    let mut found = BumpVec::new_in(scratch);
    let Value::Retyped(header) = module else {
        for member in registrations(module, types, scratch) {
            let shape = registered_shape(*member).expect("a registration member is a function");
            if key_of(shape, types) == key {
                found.push(*member);
            }
        }
        return found;
    };
    let bare = Value::Knotted(header.member());
    let offered = functions_at(writer, bare, key, types, scratch);
    let carried = header
        .ktype()
        .as_type()
        .expect("a module's type is a signature type");
    for declared in keyworded_members(carried, types, scratch) {
        if key_of(declared, types) != key {
            continue;
        }
        for function in covering(&offered, declared, types, scratch) {
            found.push(retyped_at_shape(writer, function, declared, types, scratch));
        }
    }
    found
}

/// The overloads among `offered` that cover `declared`, a keyworded member's shape — each whose
/// registered shape satisfies it — narrowed to those ranking first among themselves: the most
/// specific, or every one where none ranks first. Ranking among overloads that all cover one shape
/// does not depend on the arguments, so this is what every call through `declared` would pick.
pub fn covering<'graph, 'cell, 'x>(
    offered: &[KValue<'graph, 'cell>],
    declared: DeclaredType<KType>,
    types: &Types<'_, '_>,
    scratch: BumpAllocator<'x>,
) -> BumpVec<'x, KValue<'graph, 'cell>> {
    let mut covers = BumpVec::new_in(scratch);
    let mut shapes = BumpVec::new_in(scratch);
    for function in offered.iter().copied() {
        let registered = registered_shape(function).expect("a registration member is a function");
        if satisfied_by(types, scratch, declared, registered) {
            covers.push(function);
            shapes.push(DeclaredType::<Parametric>::from(registered));
        }
    }
    let mut picked = BumpVec::new_in(scratch);
    picked.extend((select_by_class(types, scratch, &shapes).iter()).map(|index| covers[*index]));
    picked
}

/// `function`, an overload covering the keyworded member shape `declared`, retyped to it in
/// `writer`'s region: seen at the shape it registers at, then at `declared`.
pub fn retyped_at_shape<'graph, 'cell>(
    writer: Writer<'cell>,
    function: KValue<'graph, 'cell>,
    declared: DeclaredType<KType>,
    types: &Types<'_, 'cell>,
    scratch: BumpAllocator<'_>,
) -> KValue<'graph, 'cell> {
    let registered = registered_shape(function).expect("a registration member is a function");
    Seen::registered(function, registered)
        .seen_at(declared, types, scratch)
        .restamped(writer)
}

/// The keyworded members the signature type `carried` declares, each read under its
/// application's pins: every application a meet holds, each shape once. A carried application
/// pins every head parameter, so each is closed.
fn keyworded_members<'x>(
    carried: KType,
    types: &Types<'_, '_>,
    scratch: BumpAllocator<'x>,
) -> BumpVec<'x, DeclaredType<KType>> {
    let mut members: BumpVec<'x, DeclaredType<KType>> = BumpVec::new_in(scratch);
    for application in applications(types, scratch, carried) {
        let (signature, pinned) = match types.node(application) {
            TypeNode::SignatureApply { signature, pins } => (signature, Some(pins)),
            _ => (application, None),
        };
        let Some(schema) = schema_of(signature, types) else {
            continue;
        };
        let pins = Members::from_pairs(
            scratch,
            (pinned.iter())
                .flat_map(|pins| pins.iter())
                .filter_map(|(name, pin)| match name {
                    BinderSymbol::Type(name) => Some((name, pin)),
                    _ => None,
                }),
        );
        for declared in schema.keyworded.iter().copied() {
            let read = match substitute_parameters(types, scratch, declared, pins) {
                DeclaredType::Type(read) => DeclaredType::Type(
                    types
                        .concrete(read)
                        .expect("a carried application pins every parameter"),
                ),
                DeclaredType::Scheme(scheme) => DeclaredType::Scheme(scheme),
            };
            if !members.contains(&read) {
                members.push(read);
            }
        }
    }
    members
}

/// The expression shape a registration member registers at: a function's own, the shape a
/// barrier shows its caller, or the keyworded member's shape a header retypes either to. `None` for
/// any other value.
pub fn registered_shape(member: KValue<'_, '_>) -> Option<DeclaredType<KType>> {
    let callable = member.as_callable()?;
    // A header a module offers at a key is at the keyworded member's shape it retypes to.
    if let Value::Retyped(header) = member {
        return Some(header.ktype());
    }
    match (callable.function(), callable.coerced()) {
        (Some(function), _) => function.registered_shape(),
        (None, Some(barrier)) => Some(barrier.ktype()),
        (None, None) => None,
    }
}

/// The bucket key of the expression shape `shape`: its keywords in place, each slot erased. A
/// scheme's body spells its keys as a type's does.
pub fn key_of(shape: DeclaredType<impl Into<Parametric>>, types: &Types<'_, '_>) -> KeySymbol {
    let TypeNode::ExpressionShape { elements, .. } = shape.view(types).body else {
        unreachable!("a registered shape is an expression shape")
    };
    KeySymbol::of(elements.iter().map(|element| match element {
        DispatchTokenElement::Keyword(keyword) => Some(keyword),
        DispatchTokenElement::Slot(_) => None,
    }))
}
