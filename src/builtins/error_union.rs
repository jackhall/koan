//! `KError` — the prelude union of every catchable error kind, registered once at prelude build.
//!
//! Type-only: `bindings.types["KError"]` holds the anonymous union of one sealed `NewType` member
//! per [`KErrorKind`](crate::machine::KErrorKind) surface name, each owned by the `KError` binder.
//! A lowered error is a [`KObject::Wrapped`](crate::machine::model::KObject::Wrapped) carrying its
//! kind's member handle, so `TRY` and `MATCH … OVER KError` select an arm by the same member walk
//! a user `UNION` is eliminated through.
//!
//! The member names come from [`KIND`], the same table
//! [`KError::to_wrapped`](crate::machine::KError::to_wrapped) reads a lowering's member name off,
//! so registration and lowering cannot disagree about what a kind is called.

use crate::machine::WriteGate;

use crate::machine::Scope;
use crate::machine::core::kerror::{KERROR, KIND};
use crate::machine::model::RunRegistries;
use crate::machine::model::{KType, RecursiveGroupWindow, RelativeSchema};

pub fn register<'a>(scope: &'a Scope<'a>, registries: &RunRegistries, gate: &mut WriteGate) {
    let types = &registries.types;
    let kerror = registries.labels.record(&KERROR);
    let names = KIND.all();
    let window = RecursiveGroupWindow::for_binder(
        kerror,
        names
            .iter()
            .map(|name| registries.labels.record(*name))
            .collect(),
    );
    // Each member's payload is the kind's field record, whose shape varies per kind, so the
    // declared repr is `Any` and a handler reads the fields it knows by name.
    let mut sealed = None;
    for index in 0..names.len() {
        sealed = window.fill_member(index, RelativeSchema::NewType(KType::ANY), types);
    }
    let union = sealed
        .expect("the last fill seals the KError window")
        .binder_type(kerror)
        .expect("the `KError` binder owns every sealed member");
    scope.register_builtin_type(kerror, union, registries, gate);
}
