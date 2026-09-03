//! The ascription-barrier coercion walk: rewrite a value so it inhabits a *different* binding of
//! one signature's abstract members.
//!
//! An opaque view's child scope is born holding coerced member values — each VAL member whose
//! SIG-declared slot type substitutes differently under the view's per-call mints is rewritten as
//! the view scope is filled ([`crate::builtins::ascribe`]). Every read surface therefore reports
//! the view's types by construction: ATTR, a `USING <view> SCOPE` window over the same binding
//! table, a dynamic read, and a functor's deferred return alike.
//!
//! The walk recurses on the **SIG-declared** slot type, never on the two substituted types in
//! lockstep: `TypeRegistry::union_of` canonicalizes member order and dedupes, so positional
//! correspondence between two separate substitutions is not stable. Recursing on the declared type
//! keeps every position's from/to pair exact, and [`CoercionTables::substitutions`] is the fast
//! path that stops the descent wherever the two substitutions agree.
//!
//! See [design/typing/modules.md](../../../../design/typing/modules.md).

use std::collections::HashMap;

use crate::machine::core::{Body, KFunction, SubstrateDoor};
use crate::machine::model::labels::Symbol;
use crate::machine::model::registries::RunRegistries;
use crate::machine::model::types::{
    Argument, CoercionTables, KType, ReturnType, SignatureElement, TypeNode,
};

use super::{Held, KKey, KObject};

/// Rewrite `value` — currently inhabiting `tables`' `from` substitution of `declared` — so it
/// inhabits the `to` substitution, building whatever has to be rebuilt at `door`.
///
/// Arms mirror `substitute_sig_members`, so the walk is finite for the same reason that
/// substitution is: a sealed `SetMember` and a `Signature` are leaves there and pass through here.
///
/// - A position whose two substitutions agree returns the value untouched — the fast path covering
///   a concrete slot, a manifest-only slot, and every sub-position naming no abstract member.
/// - An **`AbstractType` / `ConstructorApply`** position is the re-tag shape: the value's identity
///   handle is replaced with the `to` substitution, sharing the payload substrate
///   ([`KObject::wrapped_peel`] collapses one layer, so the single-layer invariant holds).
/// - A **container** is rebuilt cell-by-cell against its declared element / value / field type and
///   re-stamped to the substituted container type.
/// - A **`KFunction`** position takes the eta-wrapper ([`coerce_function`]) rather than recursing
///   into the value: a callable is coerced at its own boundary, per call.
/// - A **`Union`** position picks the declared member whose `from` substitution the value
///   inhabits and coerces by that member alone.
pub fn coerce_object_into<'b>(
    value: &KObject<'b>,
    declared: KType,
    tables: &CoercionTables,
    door: SubstrateDoor<'b, '_>,
    registries: &RunRegistries,
) -> KObject<'b> {
    let types = &registries.types;
    let Some((_, to)) = tables.substitutions(declared, types) else {
        return value.deep_clone();
    };
    match types.node(declared) {
        TypeNode::AbstractType { .. } | TypeNode::ConstructorApply { .. } => {
            KObject::wrapped_peel(door, value, to)
        }
        TypeNode::List { element } => match value {
            KObject::List(substrate, _) => {
                let cells: Vec<Held<'b>> = substrate
                    .elements()
                    .iter()
                    .map(|cell| coerce_held_into(cell, element, tables, door, registries))
                    .collect();
                KObject::list_rehomed(door, cells, to)
            }
            _ => value.deep_clone(),
        },
        // A declared key type naming an abstract member re-stamps the dict's type only: a `KKey`
        // is a concrete scalar with no type identity to carry, so a key read back off a coerced
        // dict is a bare scalar. Documented limitation, tied to `KKey`'s scalar nature.
        TypeNode::Dict {
            value: declared_value,
            ..
        } => match value {
            KObject::Dict(substrate, _) => {
                let cells: HashMap<KKey<'b>, Held<'b>> = substrate
                    .entries()
                    .map(|(key, cell)| {
                        (
                            *key,
                            coerce_held_into(cell, declared_value, tables, door, registries),
                        )
                    })
                    .collect();
                KObject::dict_rehomed(door, cells, to)
            }
            _ => value.deep_clone(),
        },
        TypeNode::Record { fields } => match value {
            KObject::Record(substrate, _) => {
                let cells: Vec<(Symbol, Held<'b>)> = substrate
                    .fields()
                    .map(|(name, cell)| {
                        let declared_field = fields.get(name).copied();
                        match declared_field {
                            Some(declared_field) => (
                                name,
                                coerce_held_into(cell, declared_field, tables, door, registries),
                            ),
                            // A field the slot type does not name carries no declared type to
                            // coerce against, so it rides verbatim under the value's width.
                            None => (name, *cell),
                        }
                    })
                    .collect();
                KObject::record_rehomed(door, &cells, to)
            }
            _ => value.deep_clone(),
        },
        TypeNode::KFunction { .. } => match value {
            KObject::KFunction(underlying) => {
                KObject::KFunction(coerce_function(underlying, declared, tables, registries))
            }
            _ => value.deep_clone(),
        },
        TypeNode::Union { members } => {
            // A value inhabits exactly one member, so the coercion is that member's. Picking on the
            // `from` side is what keeps the declared-type walk exact where a canonicalized union
            // would have lost the correspondence.
            let inhabited = members.iter().copied().find(|member| {
                tables
                    .substitute_from(*member, types)
                    .matches_value(value, registries)
            });
            match inhabited {
                Some(member) => coerce_object_into(value, member, tables, door, registries),
                None => value.deep_clone(),
            }
        }
        _ => value.deep_clone(),
    }
}

/// [`coerce_object_into`]'s per-cell dispatch for a container's [`Held`] cell. A type-channel cell
/// is owned data with no value identity to re-tag, so it passes through.
fn coerce_held_into<'b>(
    cell: &Held<'b>,
    declared: KType,
    tables: &CoercionTables,
    door: SubstrateDoor<'b, '_>,
    registries: &RunRegistries,
) -> Held<'b> {
    match cell {
        Held::Object(object) => Held::Object(coerce_object_into(
            object, declared, tables, door, registries,
        )),
        other => *other,
    }
}

/// The **function boundary**: a callable filling a slot whose declared FN type names an abstract
/// member is wrapped rather than rewritten. The wrapper binds against its own — `to`-substituted —
/// signature, so dispatch and the call's argument validation admit the reading side's types; its
/// [`Body::CoercedDelegate`] coerces each argument inward, delegates to `underlying`, and coerces
/// the result outward.
///
/// The wrapper is born at `underlying`'s **own captured scope**, so it lives in the same region the
/// underlying does — the invariant every callable read depends on
/// ([`retains_home`](super::retains_home) decides a callable's residence by its captured scope's
/// region, and a cell's reach seeds that same scope). The enclosing coercion's product is a
/// `KObject::KFunction` leaf riding that borrow verbatim, exactly as a copying relocation carries
/// one.
fn coerce_function<'b>(
    underlying: &'b KFunction<'b>,
    declared: KType,
    tables: &CoercionTables,
    registries: &RunRegistries,
) -> &'b KFunction<'b> {
    let types = &registries.types;
    let declared_params = types.with_node(declared, |node| match node {
        TypeNode::KFunction { params, .. } => params.clone(),
        _ => unreachable!("the FN arm is entered only for a declared `KFunction` position"),
    });
    // The call shape is the underlying's — same keywords, same parameter names in the same order —
    // with each slot the declared FN type names re-typed to its `to` substitution. A parameter the
    // slot type leaves unnamed keeps the underlying's own declared type: nothing crosses the
    // barrier at that position.
    let elements: Vec<SignatureElement> = underlying
        .signature
        .elements()
        .iter()
        .map(|element| match element {
            SignatureElement::Argument(argument) => {
                let ktype = match declared_params.get(argument.name.symbol()) {
                    Some(declared_param) => tables.substitute_to(*declared_param, types),
                    None => argument.ktype,
                };
                SignatureElement::Argument(Argument { ktype, ..*argument })
            }
            keyword => *keyword,
        })
        .collect();
    let declared_return = types.with_node(declared, |node| match node {
        TypeNode::KFunction { ret, .. } => *ret,
        _ => unreachable!("the FN arm is entered only for a declared `KFunction` position"),
    });
    KFunction::alloc_captured_resident(
        underlying.captured_scope(),
        ReturnType::Resolved(tables.substitute_to(declared_return, types)),
        &elements,
        Body::CoercedDelegate {
            underlying,
            declared,
            coercion: tables.coercion(),
        },
        registries,
    )
}
