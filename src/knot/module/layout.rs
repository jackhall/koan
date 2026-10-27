//! The readers of a module value: a named member, the registration run, the functions at a key.
//! Each reads the run by the layout order [`elaborate`'s members](crate::elaborate) spell — the
//! signature places every named member, and the registrations are the tail past them. A reader
//! finds a key's functions by their registered shapes ([`functions_at`]), never by position.
//!
//! Through a header, a reader sees what the carried signature declares: a named member retyped to
//! its declared type, and at a key, per keyworded member, the most specific overloads covering it
//! ([`covering`]), each retyped to its shape. A view's build makes the same pick. Where the load saw
//! all of the module, it recorded the pick by registration position ([`header_picks`]), and the
//! reader takes it in place of its own.

use crate::elaborate::{member_count, schema_member, schema_of, signature_member, type_members};
use crate::knot::{KValue, Knotted};
use crate::memory::{BumpAllocator, BumpVec, Writer};
use crate::scope::{Pick, Table};
use crate::symbols::{BinderSymbol, KeySymbol};
use crate::type_lattice::{
    DeclaredType, DispatchTokenElement, KType, Members, Parametric, TypeNode, satisfied_by,
    select_by_class, substitute_parameters,
};
use crate::values::{Chain, Retype, Seen, TypeValue, Value, applications};

#[cfg(test)]
thread_local! {
    /// How many keyworded members a view's build or a header's reader picked the covering
    /// overloads of itself, rather than taking the load's.
    pub static COVERED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The registration members of `module` — the tail of its run past the named members, each a
/// function a keyworded use at its registered shape's key may select: one per registration of a
/// body-born module, and one per overload a view carries for a keyworded member of its signature.
/// Empty for anything that is no module.
pub fn registrations<'graph, 'cell>(
    module: KValue<'graph, 'cell>,
    types: &Chain<'_, '_, '_, '_>,
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
/// is retyped to it in `writer`'s region, a scheme included, by `retype` where the load recorded
/// the read's; a type member is a type value of it, which the node itself need not bind. `None` if
/// `module` is no module, or its type does not name `name`.
pub fn member<'graph, 'cell>(
    writer: Writer<'cell, '_>,
    module: KValue<'graph, 'cell>,
    (name, retype): (BinderSymbol, Option<Retype>),
    types: &Chain<'_, 'cell, '_, '_>,
    scratch: BumpAllocator<'_>,
) -> Option<KValue<'graph, 'cell>> {
    let node = module.knot_member()?.module()?;
    let read = signature_member(types, scratch, module.ktype().as_type()?.into(), name)?;
    if let BinderSymbol::Type(_) = name {
        let declared = types.concrete(read.declared.as_type()?)?;
        return Some(Value::Type(TypeValue::new(writer, declared, types)));
    }
    // A module carries a signature pinning every head parameter, so what it declares is closed.
    let declared = types.concrete_declared(read.declared)?;
    let own = schema_of(node.ktype(), types)?;
    let (index, _) = schema_member(&own, scratch, name)?;
    let held = *node.members().get(index)?;
    let seen = match (declared, retype) {
        (DeclaredType::Type(declared), Some(_)) => {
            Seen::of(held).seen_as(declared, retype, types, scratch)
        }
        _ => Seen::of(held).seen_at(declared, types, scratch),
    };
    Some(seen.restamped(writer, types))
}

/// Where `module`'s node stores `value`, a member a read of it came to or a restamp of one: the
/// index of its member run a kept path's member segment names. `None` for anything that is no
/// module, and a value it stores no member of.
pub fn member_slot<'graph, 'cell>(
    module: KValue<'graph, 'cell>,
    value: KValue<'graph, 'cell>,
) -> Option<usize> {
    let members = module.knot_member()?.module()?.members();
    members.iter().position(|member| value.same_storage(member))
}

/// The member `module`'s node stores at `index`, at its own memo: what a kept path's member
/// segment reaches. `None` past the run, and for anything that is no module.
pub fn stored_member<'graph, 'cell>(
    module: KValue<'graph, 'cell>,
    index: usize,
) -> Option<KValue<'graph, 'cell>> {
    module
        .knot_member()?
        .module()?
        .members()
        .get(index)
        .copied()
}

/// How many named members a module carrying `carried` shows: each name an application `carried`
/// holds declares, once.
pub fn named_count(
    carried: KType,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
) -> usize {
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
/// retyped in `writer`'s region to the member's declared shape — or, where the load saw all of the
/// module, the overloads `picks` names ([`header_picks`]).
pub fn functions_at<'graph, 'cell, 'x>(
    writer: Writer<'cell, '_>,
    module: KValue<'graph, 'cell>,
    key: KeySymbol,
    picks: Option<&[Pick<'_>]>,
    types: &Chain<'_, 'cell, '_, '_>,
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
    let offered = functions_at(writer, bare, key, None, types, scratch);
    let carried = header
        .ktype()
        .as_type()
        .expect("a module's type is a signature type");
    for (member, declared) in keyworded_members(carried, types, scratch)
        .into_iter()
        .enumerate()
    {
        if key_of(declared, types) != key {
            continue;
        }
        let picked =
            picks.and_then(|picks| picks.iter().find(|pick| usize::from(pick.member) == member));
        let functions = match picked {
            Some(pick) => {
                let picked = picked_of(bare, pick, types, scratch);
                debug_assert!(
                    same_shapes(&covering(&offered, declared, types, scratch), &picked),
                    "the load's picks are what a header covers"
                );
                picked
            }
            None => {
                #[cfg(test)]
                COVERED.with(|covered| covered.set(covered.get() + 1));
                covering(&offered, declared, types, scratch)
            }
        };
        for function in functions {
            found.push(retyped_at_shape(writer, function, declared, types, scratch));
        }
    }
    found
}

/// What a header carrying `carried` offers for each keyworded member, by its position among
/// [`keyworded_members`], over a body-born module whose registrations register at `registered`
/// in their run's order: [`functions_at`]'s pick, recorded where the load sees all of the module.
pub fn header_picks<'x>(
    carried: KType,
    registered: &[DeclaredType<KType>],
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'x>,
) -> Option<BumpVec<'x, (u16, BumpVec<'x, u32>)>> {
    let members = keyworded_members(carried, types, scratch);
    let mut picks = BumpVec::with_capacity_in(members.len(), scratch);
    for (member, declared) in members.into_iter().enumerate() {
        let offered = offered_at(registered, declared, types, scratch);
        picks.push((u16::try_from(member).ok()?, offered));
    }
    Some(picks)
}

/// The positions among `registered`, a module's registered shapes in their run's order, of the
/// registrations at `declared`'s key that cover it ([`covered`]).
pub fn offered_at<'x>(
    registered: &[DeclaredType<KType>],
    declared: DeclaredType<KType>,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'x>,
) -> BumpVec<'x, u32> {
    let key = key_of(declared, types);
    let mut at_key = BumpVec::new_in(scratch);
    at_key.extend((0..registered.len()).filter(|at| key_of(registered[*at], types) == key));
    let mut shapes = BumpVec::with_capacity_in(at_key.len(), scratch);
    shapes.extend(at_key.iter().map(|at| registered[*at]));
    let mut offered = BumpVec::new_in(scratch);
    offered.extend(
        (covered(&shapes, declared, types, scratch).iter())
            .map(|at| u32::try_from(at_key[*at]).expect("a module has fewer than 2^32 members")),
    );
    offered
}

/// The registrations of `module` `pick` names, by position in its registration run.
pub fn picked_of<'graph, 'cell, 'x>(
    module: KValue<'graph, 'cell>,
    pick: &Pick<'_>,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'x>,
) -> BumpVec<'x, KValue<'graph, 'cell>> {
    let registrations = registrations(module, types, scratch);
    let mut picked = BumpVec::with_capacity_in(pick.offered.len(), scratch);
    picked.extend(pick.offered.iter().map(|at| registrations[*at as usize]));
    picked
}

/// Whether `covered` and `picked` are the same overloads, by registered shape, in order: the
/// debug check a recorded pick is held to.
pub fn same_shapes(covered: &[KValue<'_, '_>], picked: &[KValue<'_, '_>]) -> bool {
    covered.len() == picked.len()
        && (covered.iter().zip(picked))
            .all(|(covered, picked)| registered_shape(*covered) == registered_shape(*picked))
}

/// The overloads among `offered` that cover `declared`, a keyworded member's shape: those
/// [`covered`] picks.
pub fn covering<'graph, 'cell, 'x>(
    offered: &[KValue<'graph, 'cell>],
    declared: DeclaredType<KType>,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'x>,
) -> BumpVec<'x, KValue<'graph, 'cell>> {
    let mut shapes = BumpVec::with_capacity_in(offered.len(), scratch);
    shapes.extend(
        (offered.iter()).map(|function| {
            registered_shape(*function).expect("a registration member is a function")
        }),
    );
    let mut picked = BumpVec::new_in(scratch);
    picked.extend((covered(&shapes, declared, types, scratch).iter()).map(|at| offered[*at]));
    picked
}

/// The positions among `shapes`, registered shapes, of those covering `declared`, a keyworded
/// member's shape — each that satisfies it — narrowed to those ranking first among themselves: the
/// most specific, or every one where none ranks first. Ranking among overloads that all cover one
/// shape does not depend on the arguments, so this is what every call through `declared` would
/// pick.
pub fn covered<'x>(
    shapes: &[DeclaredType<KType>],
    declared: DeclaredType<KType>,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'x>,
) -> BumpVec<'x, usize> {
    let mut covers = BumpVec::new_in(scratch);
    let mut ranked = BumpVec::new_in(scratch);
    for (at, registered) in shapes.iter().enumerate() {
        if satisfied_by(types, scratch, declared, *registered) {
            covers.push(at);
            ranked.push(DeclaredType::<Parametric>::from(*registered));
        }
    }
    let mut picked = BumpVec::new_in(scratch);
    // No registration is a builtin; more overloads than a table ranks fall to the lattice's own
    // elimination.
    match Table::ranked(scratch, types, scratch, &ranked, |_| false, &mut || {}) {
        Some(table) => {
            let survivors = table.survivors(u64::MAX >> (64 - covers.len().max(1)));
            let kept = (0..covers.len()).filter(|at| survivors & 1 << at != 0);
            picked.extend(kept.map(|at| covers[at]));
        }
        None => {
            picked.extend((select_by_class(types, scratch, &ranked).iter()).map(|at| covers[*at]))
        }
    }
    picked
}

/// `function`, an overload covering the keyworded member shape `declared`, retyped to it in
/// `writer`'s region: seen at the shape it registers at, then at `declared`.
pub fn retyped_at_shape<'graph, 'cell>(
    writer: Writer<'cell, '_>,
    function: KValue<'graph, 'cell>,
    declared: DeclaredType<KType>,
    types: &Chain<'_, 'cell, '_, '_>,
    scratch: BumpAllocator<'_>,
) -> KValue<'graph, 'cell> {
    let registered = registered_shape(function).expect("a registration member is a function");
    Seen::registered(function, registered)
        .seen_at(declared, types, scratch)
        .restamped(writer, types)
}

/// The keyworded members the signature type `carried` declares, each read under its
/// application's pins: every application a meet holds, each shape once. A carried application
/// pins every head parameter, so each is closed.
pub fn keyworded_members<'x>(
    carried: KType,
    types: &Chain<'_, '_, '_, '_>,
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
pub fn key_of(
    shape: DeclaredType<impl Into<Parametric>>,
    types: &Chain<'_, '_, '_, '_>,
) -> KeySymbol {
    let TypeNode::ExpressionShape { elements, .. } = shape.view(types).body else {
        unreachable!("a registered shape is an expression shape")
    };
    KeySymbol::of(elements.iter().map(|element| match element {
        DispatchTokenElement::Keyword(keyword) => Some(keyword),
        DispatchTokenElement::Slot(_) => None,
    }))
}
