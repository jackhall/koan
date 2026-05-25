//! Type-constructor builtins — `LIST_OF`, `DICT_OF`, `FUNCTION_OF`,
//! `MODULE_TYPE_OF`, `TYPE_CONSTRUCTOR`, `SIG_WITH`. Each ships as an
//! ordinary scheduled `KFunction` whose inputs are `TypeExprRef`-typed
//! slots (resolved to `KObject::KTypeValue(kt)`) and whose outputs are
//! also `KObject::KTypeValue(kt)` carrying the elaborated `KType`
//! directly. Dispatching them through the same `Dispatch` / `Bind`
//! machinery values use means a parameterized type can be assembled by
//! sub-expression evaluation: `(LIST_OF (MODULE_TYPE_OF M Type))` wakes
//! the outer slot only after the inner sub-dispatch resolves to a
//! concrete `KType` value.
//!
//! Why builtins rather than a parallel registration table: the design in
//! [design/typing/scheduler.md](../../design/typing/scheduler.md) reduces
//! type-expression evaluation to ordinary dispatch — same scope-lookup
//! chain, same `Bind`-waits-for-subs refinement, same `lift_kobject`
//! rules. No new node kind, no `KType::TypeVar`, no second registration
//! table; a `TypeExprRef`-typed binding lives in `Scope::data` like any
//! other value.

mod dict_of;
mod function_of;
mod list_of;
mod module_type_of;
mod sig_with;
mod type_constructor;

use crate::machine::model::types::UserTypeKind;
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
    // Single overload: the `m` slot is `Module`. Bare Type-token operands
    // (`MODULE_TYPE_OF Foo Type`) ride the unified auto-wrap path and resolve through the
    // `value_lookup`-TypeExprRef overload to a `Future(KModule)`, which then matches this
    // slot strictly. Same shape as the ascription operators — no parallel TypeExprRef-lhs
    // overload needed.
    register_builtin(
        scope,
        "MODULE_TYPE_OF",
        sig(KType::TypeExprRef, vec![
            kw("MODULE_TYPE_OF"),
            arg("m", KType::AnyUserType { kind: UserTypeKind::Module }),
            arg("name", KType::TypeExprRef),
        ]),
        module_type_of::body,
    );
    // `TYPE_CONSTRUCTOR <param:TypeExprRef>` — declares a higher-kinded type-constructor
    // slot (template form). Inside a SIG body, `LET Wrap = (TYPE_CONSTRUCTOR Type)` binds
    // `Wrap` to a `KTypeValue(UserType { kind: TypeConstructor { param_names: ["T"] }, .. })`
    // template; `ascribe.rs:body_opaque` re-mints the slot with a fresh per-call
    // `scope_id` and the slot's declared name (e.g. `Wrap`) on opaque ascription.
    register_builtin(
        scope,
        "TYPE_CONSTRUCTOR",
        sig(KType::TypeExprRef, vec![
            kw("TYPE_CONSTRUCTOR"),
            arg("param", KType::TypeExprRef),
        ]),
        type_constructor::body,
    );
    // `SIG_WITH <sig:Signature> <bindings:KExpression>` — see
    // [`sig_with::body`] for the inner-triple parsing rules. The `bindings`
    // slot is `KExpression` (lazy), so the dispatcher hands the parens group
    // to the body verbatim; sub-Dispatch of inner value expressions
    // (`(Elt: (MODULE_TYPE_OF E Type))`) is the body's responsibility.
    register_builtin(
        scope,
        "SIG_WITH",
        sig(KType::TypeExprRef, vec![
            kw("SIG_WITH"),
            arg("sig", KType::MetaSignature),
            arg("bindings", KType::KExpression),
        ]),
        sig_with::body,
    );
}
