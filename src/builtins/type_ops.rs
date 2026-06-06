//! Type-constructor builtins — `LIST_OF`, `DICT_OF`, `TEMPLATE`, `WITH`. Each
//! ships as a scheduled `KFunction` over `TypeExprRef`-typed slots, so a
//! parameterized type assembles via sub-expression evaluation:
//! `(LIST_OF (DICT_OF Str Number))` wakes the outer slot only after
//! the inner sub-dispatch resolves to a concrete `KType` value. See
//! [design/typing/scheduler.md](../../design/typing/scheduler.md). A module
//! type-member is named by the dotted `M.T` access (see [`super::attr`]), not a
//! dedicated builtin.

mod dict_of;
mod list_of;
mod type_constructor;
mod with;

use crate::machine::model::types::Record;
use crate::machine::model::KType;
use crate::machine::Scope;

use super::{arg, kw, register_builtin, sig};

pub fn register<'a>(scope: &'a Scope<'a>) {
    register_builtin(
        scope,
        "LIST_OF",
        sig(
            KType::TypeExprRef,
            vec![kw("LIST_OF"), arg("elem", KType::TypeExprRef)],
        ),
        list_of::body,
    );
    register_builtin(
        scope,
        "DICT_OF",
        sig(
            KType::TypeExprRef,
            vec![
                kw("DICT_OF"),
                arg("key", KType::TypeExprRef),
                arg("value", KType::TypeExprRef),
            ],
        ),
        dict_of::body,
    );
    register_builtin(
        scope,
        "TEMPLATE",
        sig(
            KType::TypeExprRef,
            vec![kw("TEMPLATE"), arg("param", KType::TypeExprRef)],
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
            KType::TypeExprRef,
            vec![
                arg("sig", KType::AnySignature),
                kw("WITH"),
                arg("bindings", KType::Record(Box::new(Record::new()))),
            ],
        ),
        with::body,
    );
}
