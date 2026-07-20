//! `Result` — a builtin two-variant tagged union, registered once at prelude build like
//! `List`/`Dict`, not via `UNION`/`NEWTYPE`.
//!
//! Type-only: `bindings.types["Result"]` holds a `KType::SetRef` into a singleton
//! [`RecursiveSet`] whose one [`KKind::TypeConstructor`] member carries the variant
//! schema (`{Ok: Any, Error: Any}`) and the matching `param_names` `["Ok", "Error"]` — each
//! tag's payload type is the arg bound to the same-named parameter.
//! `:(Result {Ok = Number, Error = MyError})` drives the resolver's `ConstructorApply` arm;
//! `(Result (Ok v))` constructs by reading the projected schema off the member. No
//! value-side carrier.
//!
//! Type parameters are erased at runtime (as for `List`/`Dict`): `SetRef` identity is
//! `(set ptr, index)` and never descends the schema, so every `:(Result …)` resolves to
//! the one identity.

use std::collections::HashMap;

use crate::machine::model::{KType, NominalSchema, RecursiveSet};
use crate::machine::{BindingIndex, Scope};

pub fn register<'a>(scope: &'a Scope<'a>) {
    let scope_id = scope.id;
    let mut schema: HashMap<String, KType> = HashMap::with_capacity(2);
    schema.insert("Ok".into(), KType::Any);
    schema.insert("Error".into(), KType::Any);
    let set = RecursiveSet::singleton(
        "Result".into(),
        scope_id,
        NominalSchema::TypeConstructor {
            schema,
            param_names: vec!["Ok".into(), "Error".into()],
        },
    );
    let identity = KType::SetRef { set, index: 0 };
    // Type-only: the variant schema rides the sealed member, so construction reads it via a
    // fresh `types["Result"]` lookup — no value-side carrier. Prelude build runs once; a
    // collision would be a programming error.
    scope.register_builtin_type("Result".into(), identity, BindingIndex::BUILTIN);
}

#[cfg(test)]
mod tests;
