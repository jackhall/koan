//! The infix `WITH` signature specialization. The container and module type operations read
//! as their plain-English surfaces instead: `:(LIST OF T)` / `:(MAP K -> V)` (see
//! [`super::parameterized_types`]) and the dotted `M.T` access (see [`super::attr`]). See
//! [design/typing/scheduler.md](../../design/typing/scheduler.md).

mod with;

use crate::machine::model::types::KKind;
use crate::machine::model::types::Record;
use crate::machine::model::KType;
use crate::machine::Scope;

use super::{arg, kw, sig};

pub fn register<'a>(scope: &'a Scope<'a>) {
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
                arg("bindings", KType::Record(Box::new(Record::new()))),
            ],
        )
    };
    crate::builtins::register_builtin(scope, "WITH", with_sig(), with::body);
}
