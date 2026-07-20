//! The infix `WITH` signature specialization and the `TYPE OF` value → type introspection. The
//! container type operations read as their plain-English surfaces instead: `:(LIST OF Elem)` /
//! `:(MAP Key -> Val)` (see [`super::parameterized_types`]) and the dotted `some_module.Carrier`
//! access (see [`super::attr`]). See
//! [design/typing/scheduler.md](../../design/typing/scheduler.md).

mod type_of;
mod with;

use crate::machine::model::KKind;
use crate::machine::model::KType;
use crate::machine::model::Record;
use crate::machine::model::TypeRegistry;
use crate::machine::Scope;

use super::{arg, kw, sig};

pub fn register<'a>(scope: &'a Scope<'a>, types: &TypeRegistry) {
    // Infix `<sig> WITH {Slot = Type, …}`. A lone binary
    // keyword classifies as `Keyworded` (leading-slot signature like `FROM` / `:|`), and
    // the record-literal `bindings` operand eager-evaluates so its `(name, Held::Type)`
    // fields read directly — see [`with::body`].
    let with_sig = || {
        sig(
            KType::OfKind(KKind::ProperType),
            vec![
                arg("sig", KType::OfKind(KKind::Signature)),
                kw("WITH"),
                arg("bindings", KType::record(Box::new(Record::new()))),
            ],
        )
    };
    crate::builtins::register_builtin(scope, "WITH", with_sig(), with::body, types);
    // `TYPE OF <value>`. Keys on the full `[TYPE, OF]` bucket, so it shares no candidate bucket
    // with the SIG-body `TYPE <name>` declarator ([`super::type_decl`]). The `value` slot is
    // `Any` because a module and a container are both ordinary values here; the body rejects a
    // type-channel argument, which `Any` also admits.
    crate::builtins::register_builtin(
        scope,
        "TYPE",
        sig(
            KType::OfKind(KKind::AnyType),
            vec![kw("TYPE"), kw("OF"), arg("value", KType::Any)],
        ),
        type_of::body,
        types,
    );
}
