//! `Result` — a builtin two-variant tagged union (`ok :T`, `error :E`),
//! registered once at prelude build like `List`/`Dict`, not via `UNION`/`NEWTYPE`.
//!
//! Type-only: `bindings.types["Result"]` holds a `KType::SetRef` into a singleton
//! [`RecursiveSet`] whose one [`KKind::TypeConstructor`] member carries the variant
//! schema (`{Ok: Any, Error: Any}`) and `param_names`. `:(Result Number MyErr)` drives the
//! resolver's `ConstructorApply` arm; `(Result (Ok v))` constructs by reading the projected
//! schema off the member. No value-side carrier.
//!
//! Type parameters are erased at runtime (as for `List`/`Dict`): `SetRef` identity is
//! `(set ptr, index)` and never descends the schema, so every `:(Result …)` resolves to
//! the one identity.

use std::collections::HashMap;
use std::rc::Rc;

use crate::machine::core::{BindingIndex, Scope};
use crate::machine::model::types::{KKind, KType, NominalMember, NominalSchema, RecursiveSet};
use crate::machine::FrameSet;

pub fn register<'a>(scope: &'a Scope<'a>) {
    let scope_id = scope.id;
    let mut schema: HashMap<String, KType> = HashMap::with_capacity(2);
    schema.insert("Ok".into(), KType::Any);
    schema.insert("Error".into(), KType::Any);
    let member = NominalMember::pending("Result".into(), scope_id, KKind::TypeConstructor);
    member.fill(NominalSchema::TypeConstructor {
        schema,
        param_names: vec!["T".into(), "E".into()],
    });
    let set = Rc::new(RecursiveSet::new(vec![member]));
    let identity = KType::SetRef { set, index: 0 };
    // Type-only: the variant schema rides the sealed member, so construction reads it via a
    // fresh `types["Result"]` lookup — no value-side carrier. Prelude build runs once; a
    // collision would be a programming error.
    scope.register_type(
        "Result".into(),
        identity,
        BindingIndex::BUILTIN,
        FrameSet::empty(),
    );
}

#[cfg(test)]
mod tests;
