//! Residual type-operation builtins — `TEMPLATE` (a higher-kinded type-constructor
//! parameter) and the infix `WITH` signature specialization. The container and module
//! type operations read as their plain-English surfaces instead: `:(LIST OF T)` /
//! `:(MAP K -> V)` (see [`super::type_constructors`]) and the dotted `M.T` access (see
//! [`super::attr`]). See [design/typing/scheduler.md](../../design/typing/scheduler.md).

mod type_constructor;
mod with;

use crate::machine::model::types::KKind;
use crate::machine::model::types::Record;
use crate::machine::model::KType;
use crate::machine::Scope;

use super::{arg, kw, register_builtin, sig};

pub fn register<'a>(scope: &'a Scope<'a>) {
    register_builtin(
        scope,
        "TEMPLATE",
        sig(
            KType::OfKind(KKind::Proper),
            vec![kw("TEMPLATE"), arg("param", KType::OfKind(KKind::Proper))],
        ),
        type_constructor::body,
    );
    // Infix `<sig> WITH {Slot = Type, …}`. A lone binary
    // keyword classifies as `Keyworded` (leading-slot signature like `FROM` / `:|`), and
    // the record-literal `bindings` operand eager-evaluates so its `(name, KTypeValue)`
    // fields read directly — see [`with::body`].
    register_builtin(
        scope,
        "WITH",
        sig(
            KType::OfKind(KKind::Proper),
            vec![
                arg("sig", KType::OfKind(KKind::Signature)),
                kw("WITH"),
                arg("bindings", KType::Record(Box::new(Record::new()))),
            ],
        ),
        with::body,
    );
}
