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

use crate::machine::core::{Body, KFunction, Scope, SubstrateDoor, ViewMembers};
use crate::machine::model::labels::Symbol;
use crate::machine::model::registries::RunRegistries;
use crate::machine::model::types::{
    Argument, CoercionTables, KType, ReturnType, SigSchema, SignatureElement, TypeNode,
};

use super::{Held, KKey, KObject, Module, ModuleDraft};

/// Rewrite `value` — currently inhabiting `tables`' `from` substitution of `declared` — so it
/// inhabits the `to` substitution, building whatever has to be rebuilt at `door`.
///
/// Arms mirror `substitute_sig_members`, so the walk is finite for the same reason that
/// substitution is: a sealed `SetMember` is a leaf there and passes through here.
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
/// - A **`Signature`** position is the nested-module shape: the member is rebuilt as an opaque view
///   of itself whose own members read at the outer view's types ([`coerce_module`]).
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
    types.with_node(declared, |node| match node {
        TypeNode::AbstractType { .. } | TypeNode::ConstructorApply { .. } => {
            KObject::wrapped_peel(door, value, to)
        }
        TypeNode::List { element } => match value {
            KObject::List(substrate, _) => {
                let cells: Vec<Held<'b>> = substrate
                    .elements()
                    .iter()
                    .map(|cell| coerce_held_into(cell, *element, tables, door, registries))
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
                            coerce_held_into(cell, *declared_value, tables, door, registries),
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
        TypeNode::Signature { schema, .. } => match value {
            KObject::Module(m) => KObject::Module(coerce_module(m, schema, tables, registries)),
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
    })
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

/// [`coerce_function_cell`]'s product rested at the wrapper's own home and read back there — the
/// value-lane form, for a `KObject::KFunction` leaf the enclosing coercion carries verbatim.
fn coerce_function<'b>(
    underlying: &'b KFunction<'b>,
    declared: KType,
    tables: &CoercionTables,
    registries: &RunRegistries,
) -> &'b KFunction<'b> {
    let home = underlying.captured_scope();
    let cell = coerce_function_cell(underlying, declared, tables, registries);
    let sealed = cell.rest_into(home.brand().handle());
    home.open_function(&sealed).value()
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
/// region, and a cell's reach seeds that same scope).
///
/// The birth envelope is handed back rather than rested, because the two boundaries need it at
/// different homes: a VAL slot's wrapper rests where it was born ([`coerce_function`]), while an
/// opaque view's keyworded member rests in the *view* scope, whose bucket entry then pins the
/// underlying's region for the view's life.
pub(crate) fn coerce_function_cell<'b>(
    underlying: &'b KFunction<'b>,
    declared: KType,
    tables: &CoercionTables,
    registries: &RunRegistries,
) -> crate::machine::core::DeliveredFunction {
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
    KFunction::alloc_captured(
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

/// The **module boundary**: a member filling a slot whose declared type is a nested `Signature` is
/// rebuilt as an opaque view *of itself*, so its own members read at the outer view's types rather
/// than at the source's representation. `nested` is the declared slot's schema, whose members and
/// slots name the *enclosing* signature's abstract members — the references the outer plan binds.
///
/// The view is born at the source module's **own child scope**, both as the new scope's parent and
/// as the table it replays: it lives in the same region the source does, exactly as
/// [`coerce_function`]'s wrapper lives in its underlying's, and the enclosing coercion's pin keeps
/// that region for the product's life. Reusing [`Scope::alloc_module_view`] is what makes the
/// nested members *born* coerced, on the same terms the outer view's own members are.
///
/// The nested view is a narrowing on the same terms the outer one is: it surfaces exactly what
/// `nested` declares — type members, value slots, keyworded members — and of those, only the ones
/// whose two substitutions differ are born coerced.
fn coerce_module<'b>(
    m: &'b Module<'b>,
    nested: &SigSchema,
    tables: &CoercionTables,
    registries: &RunRegistries,
) -> &'b Module<'b> {
    let types = &registries.types;
    // A nested signature's own abstract members shadow the enclosing binder's by name, so the
    // descent carries the narrowed plan — the same subtraction `substitute_sig_members` makes.
    let narrowed;
    let tables = if nested.abstract_members.is_empty() {
        tables
    } else {
        narrowed = tables.shadowed_by(nested, types);
        &narrowed
    };

    // The view's type members: exactly the ones the nested signature declares. A manifest member
    // reads at its `to` substitution; an abstract member reads at the source module's own binding
    // for it, which is where the nested binder's identity lives — the enclosing plan does not
    // rewrite it, since a nested binder shadows by name. Nothing is minted — a nested view's
    // *enclosing* abstract identities are the outer view's mints, arriving through the
    // substitution — so the plan closure ignores its nonce.
    let mut view_types: HashMap<crate::machine::model::labels::TypeSymbol, KType> = nested
        .abstract_members
        .keys()
        .filter_map(|name| m.type_members.get(name).map(|kt| (*name, *kt)))
        .collect();
    for (name, kt) in &nested.manifest_members {
        view_types.insert(*name, tables.substitute_to(*kt, types));
    }

    let view_members = ViewMembers::of_schema(
        nested,
        view_types.into_iter().collect(),
        tables.coercion(),
        registries,
    );
    let source = m.child_scope();
    let scope = Scope::alloc_module_view(source, source, registries, |_nonce| view_members)
        .expect("a module's own tables replay into a newborn view scope without colliding");
    // Nothing binds into the scope past the replay, so its reach-set seals here, before the module
    // captures it — the same close the outer view's construction makes.
    scope.close();

    let mut draft = ModuleDraft::empty();
    for (name, kt) in scope.bindings().iter_types() {
        draft.type_members.insert(name, kt);
    }
    // The raw derivation is exact here: the scope is born holding coerced member values, so the
    // ktype read off each member already reports the outer view's identity — no slot needs
    // re-expressing against the declared types, and none can be claimed the module does not hold.
    let self_sig = SigSchema::raw_self_sig(scope, &draft);
    Module::alloc_at_child_scope(m.path, scope, draft, types.signature(self_sig))
}
