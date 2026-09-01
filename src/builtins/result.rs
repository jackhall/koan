//! `Result` — a builtin two-variant union, registered once at prelude build like `List`/`Dict`,
//! not via `UNION`.
//!
//! Type-only: `bindings.types["Result"]` holds the anonymous union of two sealed `NewType`
//! members, `Ok` and `Error`, each over `Any` and owned by the `Result` binder — the shape a user
//! `UNION Result = (Ok :Any, Error :Any)` would seal. Construction is projection-only
//! (`Result.Ok 1`, `Result.Error e`), and applying the union head directly raises the
//! member-projection guidance error every union head gives.
//!
//! `:(Result {Ok = Number, Error = MyError})` is *type application* over the union head: it
//! lowers to the union of `ConstructorApply` nodes over each named member, so a slot so typed
//! runtime-checks the inhabited member's payload against its same-named argument.

use crate::machine::WriteGate;

use crate::machine::Scope;
use crate::machine::model::RunRegistries;
use crate::machine::model::{KType, RecursiveGroupWindow, RelativeSchema, StaticName, TypeSymbol};

/// The family's own name and its two variant names — the three labels of the `Result` shape, each
/// fixed in Rust source, so each is minted once for the process and recorded into a run's interner
/// at registration. [`catch`](super::catch) and [`try_with`](super::try_with) read the variants
/// back through these same statics, so the member a `Result` is built under and the member
/// registration declared cannot drift apart.
pub(crate) static RESULT: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "Result");
pub(crate) static OK: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "Ok");
pub(crate) static ERROR: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "Error");

pub fn register<'a>(scope: &'a Scope<'a>, registries: &RunRegistries, gate: &mut WriteGate) {
    let types = &registries.types;
    let result = registries.labels.record(&RESULT);
    let ok = registries.labels.record(&OK);
    let error = registries.labels.record(&ERROR);
    // The one-binder window a standalone `UNION` runs: `Result` owns both members, so neither
    // variant name binds on its own and the binder denotes their union. Each member's payload is
    // `Any` — a `Result`'s variant types are supplied by application, never by the declaration.
    let window = RecursiveGroupWindow::for_binder(result, vec![ok, error]);
    window.fill_member(0, RelativeSchema::NewType(KType::ANY), types);
    let sealed = window
        .fill_member(1, RelativeSchema::NewType(KType::ANY), types)
        .expect("a two-member window seals on its second fill");
    let union = sealed
        .binder_type(result)
        .expect("the `Result` binder owns both sealed members");
    // Type-only: the members carry the variant identities, so construction reaches them by
    // projection off a fresh `types["Result"]` lookup — no value-side carrier. Prelude build runs
    // once; a collision would be a programming error.
    scope.register_builtin_type(result, union, registries, gate);
}

#[cfg(test)]
mod tests;
