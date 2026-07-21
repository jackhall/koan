//! `Result` — a builtin two-variant tagged union, registered once at prelude build like
//! `List`/`Dict`, not via `UNION`/`NEWTYPE`.
//!
//! Type-only: `bindings.types["Result"]` holds the interned member handle of a one-member
//! [`KKind::TypeConstructor`] group whose member carries the variant schema
//! (`{Ok: Any, Error: Any}`) and the matching `param_names` `["Ok", "Error"]` — each tag's
//! payload type is the arg bound to the same-named parameter.
//! `:(Result {Ok = Number, Error = MyError})` drives the resolver's `ConstructorApply` arm;
//! `(Result (Ok v))` constructs by reading the schema off the member node. No value-side
//! carrier.
//!
//! Type parameters are erased at runtime (as for `List`/`Dict`): the member handle is the
//! constructor's identity and never descends its arguments, so every `:(Result …)` resolves
//! to the one identity.

use crate::machine::model::TypeRegistry;
use std::collections::HashMap;

use crate::machine::model::{KType, RecursiveGroupWindow, RelativeSchema};
use crate::machine::Scope;

pub fn register<'a>(scope: &'a Scope<'a>, types: &TypeRegistry) {
    let mut schema: HashMap<String, KType> = HashMap::with_capacity(2);
    schema.insert("Ok".into(), KType::ANY);
    schema.insert("Error".into(), KType::ANY);
    // A one-member window sealed in miniature: the sole `TypeConstructor` member's component is a
    // singleton, so its interned handle is `Result`'s identity.
    let identity = RecursiveGroupWindow::seal_singleton(
        "Result".into(),
        RelativeSchema::TypeConstructor {
            schema,
            param_names: vec!["Ok".into(), "Error".into()],
        },
        None,
        types,
    );
    // Type-only: the variant schema rides the sealed member, so construction reads it via a
    // fresh `types["Result"]` lookup — no value-side carrier. Prelude build runs once; a
    // collision would be a programming error.
    scope.register_builtin_type("Result".into(), identity);
}

#[cfg(test)]
mod tests;
