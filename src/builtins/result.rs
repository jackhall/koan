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

use crate::machine::WriteGate;

use crate::machine::Scope;
use crate::machine::model::RunRegistries;
use crate::machine::model::{
    KType, RecursiveGroupWindow, RelativeSchema, StaticName, TypeMemberMap, TypeSymbol,
};

/// The family's own name and its two variant tags — the three labels of the `Result` shape, each
/// fixed in Rust source, so each is minted once for the process and recorded into a run's interner
/// at registration. [`catch`](super::catch) reads the tags back through these same statics, so the
/// tag a `Result` is built under and the tag registration declared cannot drift apart.
static RESULT: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "Result");
pub(crate) static OK: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "Ok");
pub(crate) static ERROR: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "Error");

pub fn register<'a>(scope: &'a Scope<'a>, registries: &RunRegistries, gate: &mut WriteGate) {
    let types = &registries.types;
    let ok = registries.labels.record(&OK);
    let error = registries.labels.record(&ERROR);
    let mut schema = TypeMemberMap::default();
    schema.insert(ok, KType::ANY);
    schema.insert(error, KType::ANY);
    // A one-member window sealed in miniature: the sole `TypeConstructor` member's component is a
    // singleton, so its interned handle is `Result`'s identity.
    let identity = RecursiveGroupWindow::seal_singleton(
        registries.labels.record(&RESULT),
        RelativeSchema::TypeConstructor {
            schema,
            param_names: vec![ok, error],
        },
        None,
        types,
    );
    // Type-only: the variant schema rides the sealed member, so construction reads it via a
    // fresh `types["Result"]` lookup — no value-side carrier. Prelude build runs once; a
    // collision would be a programming error.
    scope.register_builtin_type(
        registries.labels.record(&RESULT),
        identity,
        registries,
        gate,
    );
}

#[cfg(test)]
mod tests;
