//! `Result` — a builtin two-variant tagged union (`ok :T`, `error :E`) over two
//! type parameters, registered once at prelude build.
//!
//! Shaped like `List`/`Dict` (a specific pre-registered type), not a `UNION`/`STRUCT`
//! declarator. Two registrations ride under the one name, dual-written by
//! [`Scope::register_nominal`] exactly as [`union::finalize_union`](super::union):
//!   - `bindings.types["Result"]` = a [`UserTypeKind::TypeConstructor`] identity, so
//!     `:(Result Number MyErr)` drives the resolver's `ConstructorApply` arm.
//!   - `bindings.data["Result"]` = a [`KObject::TaggedUnionType`] carrier with schema
//!     `{ok: Any, error: Any}`, so `(Result (ok v))` / `(Result (error e))` construct
//!     through the existing `dispatch_constructor` route with no new construction code.
//!
//! Type parameters are erased at runtime (consistent with `List`/`Dict`): the type-side
//! identity keys on `(kind, scope_id, name)` and `UserTypeKind`'s `PartialEq` ignores
//! `param_names`, so every `:(Result …)` resolves to the one registered carrier.

use std::collections::HashMap;
use std::rc::Rc;

use crate::machine::core::Scope;
use crate::machine::model::types::{KType, UserTypeKind};
use crate::machine::model::values::KObject;

pub fn register<'a>(scope: &'a Scope<'a>) {
    let arena = scope.arena;
    let scope_id = scope.id;
    let mut schema: HashMap<String, KType> = HashMap::with_capacity(2);
    schema.insert("ok".into(), KType::Any);
    schema.insert("error".into(), KType::Any);
    let carrier: &'a KObject<'a> = arena.alloc_object(KObject::TaggedUnionType {
        schema: Rc::new(schema),
        name: "Result".into(),
        scope_id,
    });
    let identity = KType::UserType {
        kind: UserTypeKind::TypeConstructor { param_names: vec!["T".into(), "E".into()] },
        scope_id,
        name: "Result".into(),
    };
    // Registration runs once at prelude build, so a collision is a programming error;
    // `register_nominal` panics on borrow conflict and we drop the success value.
    let _ = scope.register_nominal("Result".into(), identity, carrier);
}

#[cfg(test)]
mod tests;
