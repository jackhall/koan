use std::rc::Rc;

use crate::machine::core::{PendingBinderGuard, PendingTypeEntry};
use crate::machine::model::{KObject, KType};
use crate::machine::model::types::UserTypeKind;
use crate::machine::{
    ArgumentBundle, BindingIndex, BodyResult, CombineFinish, Frame, KError, KErrorKind, NodeId,
    Scope, SchedulerHandle,
};
use crate::machine::model::types::{
    parse_typed_field_list_via_elaborator, Elaborator, FieldListOutcome,
};

use crate::machine::model::ast::KExpression;

use crate::machine::core::kfunction::argument_bundle::{extract_bare_type_name, extract_kexpression};
use super::{arg, err, kw, register_nominal_binder_with_pre_run, sig};

/// `STRUCT <name:TypeExprRef> = (<schema>)` — declare a named record type.
///
/// The schema slot is `KType::KExpression`: the user writes a parens-wrapped expression of
/// repeated `<field:Identifier> : <type:Type>` triples (`STRUCT Point = (x: Number, y: Number)`).
/// Same triple shape as `UNION` — both delegate to
/// [`crate::machine::model::types::parse_typed_field_list_via_elaborator`] so the
/// parsing logic and error messages stay consistent.
///
/// Unlike `UNION`, struct schemas preserve declaration order so [`struct_value::apply`]
/// (super::struct_value::apply) can reorder the user's named-arg pairs (`Point (x: 3, y: 4)`
/// or `Point (y: 4, x: 3)`) into a stable canonical order before the construction primitive
/// runs. The registered schema is therefore an ordered `Vec<(String, KType)>` rather than a
/// `HashMap`.
///
/// Empty schemas, unknown type names, duplicate field names, and malformed triples all
/// surface as `ShapeError` with the offending position called out. The named form
/// registers the type token (`Point`) in the current scope so it can be used as a
/// constructor downstream via the type-call dispatch path.
pub fn body<'a>(
    scope: &'a Scope<'a>,
    sched: &mut dyn SchedulerHandle<'a>,
    mut bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    // `TypeExprRef`-typed slot resolves to `KObject::KTypeValue(kt)` for builtin leaves
    // / structural shapes or `KObject::TypeNameRef(t, _)` for bare user-bound names.
    // The shared helper accepts either carrier; reject parameterized forms like
    // `Point<X>` at definition time.
    let name = match extract_bare_type_name(&bundle, "name", "STRUCT") {
        Ok(n) => n,
        Err(e) => return err(e),
    };
    let schema_expr = match extract_kexpression(&mut bundle, "schema") {
        Some(e) => e,
        None => {
            return err(KError::new(KErrorKind::ShapeError(
                "STRUCT schema slot must be a parenthesized expression".to_string(),
            )));
        }
    };
    // Stage-3.2 SCC: register this binder in `pending_types` so a fellow in-flight
    // STRUCT / named-UNION's elaboration can detect a closing cycle when it parks on
    // our placeholder. The entry carries the schema + kind + scope_id so cycle-close
    // can install our identity without re-entering dispatch. The returned guard's
    // Drop removes the entry — synchronous arms let it drop at body exit; the Park
    // path moves it into the Combine-finish closure.
    let scope_id = scope.id;
    let pending_guard = scope.bindings().insert_pending_type(
        name.clone(),
        PendingTypeEntry {
            kind: UserTypeKind::Struct,
            scope_id,
            schema_expr: schema_expr.clone(),
            edges: Vec::new(),
        },
    );
    // Phase-3 elaborator: seeds the threaded set with this STRUCT's binder name so a
    // self-reference (`STRUCT Tree { children: List<Tree> }`) resolves to
    // `KType::RecursiveRef("Tree")` rather than parking on the binder's own placeholder.
    // `with_current_decl` arms the SCC edge-recording / cycle-detection arm.
    let mut elaborator = Elaborator::new(scope)
        .with_threaded([name.clone()])
        .with_current_decl(name.clone(), UserTypeKind::Struct, scope_id);
    let outcome = parse_typed_field_list_via_elaborator(
        &schema_expr,
        "STRUCT schema",
        &mut elaborator,
    );
    // STRUCT is a nominal binder (D7 carve-out): the placeholder install at submission
    // already stamped `nominal_binder: true`; the eventual `register_nominal` must
    // carry the same flag so the visibility tag stays consistent across the
    // placeholder → finalized-binding transition.
    let bind_index = sched
        .current_lexical_chain()
        .map(|chain| BindingIndex::nominal(chain.index))
        .unwrap_or(BindingIndex::BUILTIN);
    match outcome {
        FieldListOutcome::Done(fields) => finalize_struct(scope, name, fields, bind_index),
        FieldListOutcome::Err(msg) => err(KError::new(KErrorKind::ShapeError(msg))),
        FieldListOutcome::Park(producers) => defer_struct_via_combine(
            scope,
            sched,
            name,
            schema_expr,
            producers,
            pending_guard,
            bind_index,
        ),
    }
}

/// Build and bind the `KObject::StructType` once every field type has elaborated.
/// Shared between the synchronous (no-park) path and the Combine-finish path.
fn finalize_struct<'a>(
    scope: &'a Scope<'a>,
    name: String,
    fields: Vec<(String, KType<'a>)>,
    bind_index: BindingIndex,
) -> BodyResult<'a> {
    // Pending-types lifecycle is owned by the caller's `PendingBinderGuard`; the
    // guard drops on body / Combine-finish return and removes the entry. Cycle-close
    // never removes entries (carrier-write is the finalize's job), so by the time
    // we land here the guard is the sole source of truth for cleanup.
    //
    // Idempotent-finalize guard: if both maps are already populated for this name,
    // a parallel finalize (cycle-close + Combine-finish, or two Combine-finishes)
    // already produced a carrier. Return it without re-allocating. Defense-in-depth
    // — the carrier-write today routes through `try_register_nominal`'s idempotent
    // arm which tolerates a pre-installed identity, but cannot tolerate a
    // pre-installed carrier.
    let bindings = scope.bindings();
    if bindings.types().get(&name).is_some() {
        if let Some((existing, _)) = bindings.data().get(&name).copied() {
            return BodyResult::Value(existing);
        }
    }
    if fields.is_empty() {
        return err(KError::new(KErrorKind::ShapeError(
            "STRUCT schema must have at least one field".to_string(),
        )));
    }
    let arena = scope.arena;
    // Per-declaration identity: `scope_id` is the declaring (parent) scope's address —
    // the same `*const _ as usize` scheme `Module::scope_id()` uses, stable for the
    // run because scopes are arena-allocated and never moved. The schema carrier and
    // the dual-written `KType::UserType` identity tag share these `(scope_id, name)`
    // fields so dispatch on the carrier (via its `ktype()`) and dispatch through a
    // slot typed by the identity reach the same `UserType` value.
    let scope_id = scope.id;
    let struct_obj: &'a KObject<'a> = arena.alloc(KObject::StructType {
        name: name.clone(),
        scope_id,
        fields: Rc::new(fields),
    });
    let identity = KType::UserType {
        kind: UserTypeKind::Struct,
        scope_id,
        name: name.clone(),
    };
    match scope.register_nominal(name, identity, struct_obj, bind_index) {
        Ok(obj) => BodyResult::Value(obj),
        Err(e) => err(e),
    }
}

/// Schedule a `Combine` over `producers` and re-run the schema elaboration in the finish
/// closure. Same shape MODULE / SIG / FN-def use post-phase-3. `pending_guard` is moved
/// into the closure so the `pending_types` entry survives the wait and is dropped
/// regardless of which finish arm fires.
fn defer_struct_via_combine<'a>(
    scope: &'a Scope<'a>,
    sched: &mut dyn SchedulerHandle<'a>,
    name: String,
    schema_expr: KExpression<'a>,
    producers: Vec<NodeId>,
    pending_guard: PendingBinderGuard<'a>,
    bind_index: BindingIndex,
) -> BodyResult<'a> {
    let name_for_finish = name.clone();
    let finish: CombineFinish<'a> = Box::new(move |scope, _sched, _results| {
        // Move the guard into the closure's frame so its Drop runs on closure exit
        // regardless of finish arm.
        let _pending_guard = pending_guard;
        // Producers terminalized — re-elaborate against the now-final scope. The
        // Combine-finish path runs AFTER the dispatch-time park; if cycle-close
        // populated `bindings.types` while we were parked, `resolve_type` resolves
        // the cross-members synchronously here. No `current_decl` seeding — cycle
        // detection only matters for in-flight binders that might park, and by the
        // time we're here all producers have terminalized.
        let mut elaborator = Elaborator::new(scope).with_threaded([name_for_finish.clone()]);
        match parse_typed_field_list_via_elaborator(
            &schema_expr,
            "STRUCT schema",
            &mut elaborator,
        ) {
            FieldListOutcome::Done(fields) => {
                finalize_struct(scope, name_for_finish.clone(), fields, bind_index)
            }
            FieldListOutcome::Err(msg) => BodyResult::Err(
                KError::new(KErrorKind::ShapeError(msg))
                    .with_frame(Frame::bare("<struct>", format!("STRUCT {} schema", name_for_finish))),
            ),
            FieldListOutcome::Park(_) => BodyResult::Err(KError::new(KErrorKind::ShapeError(
                "STRUCT schema elaboration parked again after Combine wake".to_string(),
            ))),
        }
    });
    // `producers` are sibling slots the schema parked on while elaborating;
    // this Combine reads their values at finish-time but does NOT own them.
    let combine_id = sched.add_combine(vec![], producers, scope, finish);
    BodyResult::DeferTo(combine_id)
}

/// Dispatch-time placeholder extractor for STRUCT. The name slot at `parts[1]` is a
/// `Type(t)` token (the `TypeExprRef`-typed `name` argument). Only fires for bare leaves —
/// parameterized forms (`STRUCT Foo<X> = ...`) aren't supported until functors land.
pub(crate) fn pre_run(expr: &KExpression<'_>) -> Option<String> {
    expr.binder_name_from_type_part()
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    register_nominal_binder_with_pre_run(
        scope,
        "STRUCT",
        sig(KType::Type, vec![
            kw("STRUCT"),
            arg("name", KType::TypeExprRef),
            kw("="),
            arg("schema", KType::KExpression),
        ]),
        body,
        Some(pre_run),
    );
}

#[cfg(test)]
mod tests;
