//! `Result` — a builtin two-variant tagged union (`ok :T`, `error :E`),
//! registered once at prelude build like `List`/`Dict`, not via `UNION`/`STRUCT`.
//!
//! Dual-written by [`Scope::register_nominal`] just as [`union::finalize_union`](super::union):
//!   - `bindings.types["Result"]` — a [`UserTypeKind::TypeConstructor`] identity, so
//!     `:(Result Number MyErr)` drives the resolver's `ConstructorApply` arm.
//!   - `bindings.data["Result"]` — a [`KObject::TaggedUnionType`] carrier with schema
//!     `{ok: Any, error: Any}`, so `(Result (ok v))` constructs via `dispatch_constructor`.
//!
//! Type parameters are erased at runtime (as for `List`/`Dict`): `UserTypeKind`'s
//! `PartialEq` ignores `param_names`, so every `:(Result …)` resolves to the one carrier.

use std::collections::HashMap;
use std::rc::Rc;

use crate::machine::core::{BindingIndex, Scope};
use crate::machine::model::types::{KType, UserTypeKind};
use crate::machine::model::values::KObject;

pub fn register<'a>(scope: &'a Scope<'a>) {
    let arena = scope.arena;
    let scope_id = scope.id;
    let mut schema: HashMap<String, KType> = HashMap::with_capacity(2);
    schema.insert("ok".into(), KType::Any);
    schema.insert("error".into(), KType::Any);
    let carrier: &'a KObject<'a> = arena.alloc(KObject::TaggedUnionType {
        schema: Rc::new(schema),
        name: "Result".into(),
        scope_id,
    });
    let identity = KType::UserType {
        kind: UserTypeKind::TypeConstructor { param_names: vec!["T".into(), "E".into()] },
        scope_id,
        name: "Result".into(),
    };
    // Prelude build runs once; a collision here would be a programming error.
    let _ = scope.register_nominal("Result".into(), identity, carrier, BindingIndex::BUILTIN);
}

#[cfg(test)]
mod tests;
