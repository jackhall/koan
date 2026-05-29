//! Type-constructor builtins — `LIST_OF`, `DICT_OF`, `FUNCTION_OF`,
//! `MODULE_TYPE_OF`, `TYPE_CONSTRUCTOR`, `SIG_WITH`. Each ships as a
//! scheduled `KFunction` over `TypeExprRef`-typed slots, so a
//! parameterized type assembles via sub-expression evaluation:
//! `(LIST_OF (MODULE_TYPE_OF M Type))` wakes the outer slot only after
//! the inner sub-dispatch resolves to a concrete `KType` value. See
//! [design/typing/scheduler.md](../../design/typing/scheduler.md).

mod dict_of;
mod function_of;
mod list_of;
mod module_type_of;
mod sig_with;
mod type_constructor;

use crate::machine::model::KType;
use crate::machine::Scope;

use super::{arg, kw, register_builtin, sig};

pub fn register<'a>(scope: &'a Scope<'a>) {
    register_builtin(
        scope,
        "LIST_OF",
        sig(KType::TypeExprRef, vec![kw("LIST_OF"), arg("elem", KType::TypeExprRef)]),
        list_of::body,
    );
    register_builtin(
        scope,
        "DICT_OF",
        sig(KType::TypeExprRef, vec![
            kw("DICT_OF"),
            arg("key", KType::TypeExprRef),
            arg("value", KType::TypeExprRef),
        ]),
        dict_of::body,
    );
    register_builtin(
        scope,
        "FUNCTION_OF",
        sig(KType::TypeExprRef, vec![
            kw("FUNCTION_OF"),
            arg("args", KType::KExpression),
            kw("->"),
            arg("ret", KType::TypeExprRef),
        ]),
        function_of::body,
    );
    register_builtin(
        scope,
        "MODULE_TYPE_OF",
        sig(KType::TypeExprRef, vec![
            kw("MODULE_TYPE_OF"),
            arg("m", KType::AnyModule),
            arg("name", KType::TypeExprRef),
        ]),
        module_type_of::body,
    );
    register_builtin(
        scope,
        "TYPE_CONSTRUCTOR",
        sig(KType::TypeExprRef, vec![
            kw("TYPE_CONSTRUCTOR"),
            arg("param", KType::TypeExprRef),
        ]),
        type_constructor::body,
    );
    // `bindings` is `KExpression` (lazy) so sub-Dispatch of inner value
    // expressions stays the body's responsibility — see [`sig_with::body`].
    register_builtin(
        scope,
        "SIG_WITH",
        sig(KType::TypeExprRef, vec![
            kw("SIG_WITH"),
            arg("sig", KType::AnySignature),
            arg("bindings", KType::KExpression),
        ]),
        sig_with::body,
    );
}
