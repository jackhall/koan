//! `Result` — a builtin two-variant tagged union (`ok :T`, `error :E`),
//! registered once at prelude build like `List`/`Dict`, not via `UNION`/`STRUCT`.
//!
//! Type-only: `bindings.types["Result"]` holds a [`UserTypeKind::TypeConstructor`]
//! identity whose `schema` payload (`{ok: Any, error: Any}`) and `param_names` ride the
//! identity. `:(Result Number MyErr)` drives the resolver's `ConstructorApply` arm;
//! `(Result (ok v))` constructs by reading that schema off a fresh `types["Result"]`
//! lookup. No value-side carrier.
//!
//! Type parameters are erased at runtime (as for `List`/`Dict`): `UserTypeKind`'s
//! `PartialEq` ignores `schema` and `param_names`, so every `:(Result …)` resolves to
//! the one identity.

use std::collections::HashMap;
use std::rc::Rc;

use crate::machine::core::{BindingIndex, Scope};
use crate::machine::model::types::{KType, UserTypeKind};

pub fn register<'a>(scope: &'a Scope<'a>) {
    let scope_id = scope.id;
    let mut schema: HashMap<String, KType> = HashMap::with_capacity(2);
    schema.insert("ok".into(), KType::Any);
    schema.insert("error".into(), KType::Any);
    let identity = KType::UserType {
        kind: UserTypeKind::TypeConstructor {
            schema: Rc::new(schema),
            param_names: vec!["T".into(), "E".into()],
        },
        scope_id,
        name: "Result".into(),
    };
    // Type-only: the variant schema rides the identity, so construction reads it via a
    // fresh `types["Result"]` lookup — no value-side carrier. Prelude build runs once; a
    // collision would be a programming error.
    scope.register_type("Result".into(), identity, BindingIndex::BUILTIN);
}

#[cfg(test)]
mod tests;
