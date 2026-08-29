//! The infix `WITH` signature specialization and the `TYPE OF` value → type introspection. The
//! container type operations read as their plain-English surfaces instead: `:(LIST OF Elem)` /
//! `:(MAP Key -> Val)` (see [`super::parameterized_types`]) and the dotted `some_module.Carrier`
//! access (see [`super::attr`]). See
//! [design/typing/scheduler.md](../../design/typing/scheduler.md).

mod type_of;
mod with;

use crate::machine::Scope;
use crate::machine::WriteGate;
use crate::machine::model::KKind;
use crate::machine::model::KType;
use crate::machine::model::Record;

use super::{arg, kw, sig};
use crate::machine::model::RunRegistries;

// This builtin's slot spellings, minted once and read back by symbol.
crate::slots! { SLOTS { bindings, sig, value } }

pub fn register<'a>(scope: &'a Scope<'a>, registries: &RunRegistries, gate: &mut WriteGate) {
    let types = &registries.types;
    // Infix `<sig> WITH {Slot = Type, …}`. A lone binary
    // keyword classifies as `Keyworded` (leading-slot signature like `FROM` / `:|`), and
    // the record-literal `bindings` operand eager-evaluates so its `(name, Held::Type)`
    // fields read directly — see [`with::body`].
    let with_sig = || {
        sig(
            KType::of_kind(KKind::ProperType),
            vec![
                arg(registries, &SLOTS.sig, KType::of_kind(KKind::Signature)),
                kw(registries, "WITH"),
                arg(registries, &SLOTS.bindings, types.record(Record::new())),
            ],
        )
    };
    crate::builtins::register_builtin(scope, with_sig(), with::body, registries, gate);
    // `TYPE OF <value>`. Keys on the full `[TYPE, OF]` bucket, so it shares no candidate bucket
    // with the SLOTS.sig-body `TYPE <name>` declarator ([`super::type_decl`]). The `value` slot is
    // `Any` because a module and a container are both ordinary values here; the body rejects a
    // type-channel argument, which `Any` also admits.
    crate::builtins::register_builtin(
        scope,
        sig(
            KType::of_kind(KKind::AnyType),
            vec![
                kw(registries, "TYPE"),
                kw(registries, "OF"),
                arg(registries, &SLOTS.value, KType::ANY),
            ],
        ),
        type_of::body,
        registries,
        gate,
    );
}
