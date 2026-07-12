//! `SIG <name:ProperType> = <body:KExpression>` — declare a module signature (an
//! interface a module can be ascribed to). See
//! [design/typing/modules.md](../../design/typing/modules.md).
//!
//! Routes [`await_body_in_scope`](super::await_body::await_body_in_scope) like
//! `module_def` / `recursive_types`: body statements dispatch against a fresh child scope
//! on the outer scheduler, and the finish captures the populated scope into a
//! [`ModuleSignature`] value, allocates it in the parent's region, and binds it under the
//! signature's name. Body declarations are `LET name = (FN <signature> -> <return> = ...)`
//! for operations and `LET Carrier = TypeIdentifier` for abstract type declarations. The
//! ascription operators (`:|` / `:!`) iterate the stored scope at ascription time.

use crate::machine::model::types::KKind;
use crate::machine::model::values::ModuleSignature;
use crate::machine::model::KType;
use crate::machine::{Scope, TraceFrame};

use super::{arg, kw, sig};

/// The SIG body: mints the declaration scope, dispatches the SIG body block against it via
/// [`await_body_in_scope`](super::await_body::await_body_in_scope), and the finish captures
/// that scope into a [`ModuleSignature`] and installs the `KType::Signature` identity into
/// the parent scope.
pub fn body<'a>(
    ctx: &crate::machine::core::kfunction::action::BodyCtx<'a, '_>,
) -> crate::machine::core::kfunction::action::Action<'a> {
    use super::await_body::{await_body_in_scope, ChildScopeSeal};
    use crate::machine::core::kfunction::action::{
        require_bare_type_name, require_kexpression, Action,
    };

    let name = crate::try_action!(require_bare_type_name(ctx.args, "name", "SIG"));
    let body_expr = crate::try_action!(require_kexpression(ctx.args, "SIG", "body"));

    let decl_scope = ctx
        .scope
        .brand()
        .alloc_scope(Scope::child_under_sig(ctx.scope, name.clone()));

    let bind_index = ctx.bind_index();
    let name_for_finish = name;
    await_body_in_scope(
        decl_scope,
        body_expr,
        ChildScopeSeal::SealBeforeFinish,
        move |fctx| {
            let sig: &'a ModuleSignature<'a> = fctx
                .scope
                .brand()
                .alloc_signature(ModuleSignature::new(name_for_finish.clone(), decl_scope));
            let identity = KType::Signature {
                sig,
                pinned_slots: Vec::new(),
            };
            match fctx
                .scope
                .register_nominal_upsert(name_for_finish.clone(), identity, bind_index)
            {
                Ok(kt_ref) => Action::Done(fctx.ctx.alloc_type_checked(kt_ref.clone())),
                Err(e) => Action::Done(Err(e.with_frame(TraceFrame::bare(
                    "<signature>",
                    format!("SIG {} body", name_for_finish),
                )))),
            }
        },
    )
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    let signature = sig(
        KType::OfKind(KKind::Signature),
        vec![
            kw("SIG"),
            arg("name", KType::OfKind(KKind::ProperType)),
            kw("="),
            arg("body", KType::KExpression),
        ],
    );
    crate::builtins::register_builtin_full(
        scope,
        "SIG",
        signature,
        body,
        Some((super::type_part_binder_name, crate::machine::BindKind::Type)),
        None,
        false,
    );
}

#[cfg(test)]
mod tests {
    use crate::builtins::test_support::{run, run_root_silent};
    use crate::machine::core::run_root_storage;
    use crate::parse::parse;

    #[test]
    fn binder_name_extracts_sig_name() {
        let mut exprs = parse("SIG OrderedSig = (VAL x :Number)").expect("parse should succeed");
        let expr = exprs.remove(0);
        let name = expr.binder_name_from_type_part();
        assert_eq!(name.as_deref(), Some("OrderedSig"));
    }

    #[test]
    fn sig_binds_under_name_in_scope() {
        use crate::machine::model::types::KType;
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        run(scope, "SIG OrderedSig = (VAL x :Number)");
        // SIG installs a single type-side identity; nothing lands in `bindings.data`.
        assert!(scope.bindings().data().get("OrderedSig").is_none());
        assert!(matches!(
            scope.resolve_type("OrderedSig"),
            Some(KType::Signature { .. })
        ));
    }

    #[test]
    fn sig_path_records_name() {
        use crate::machine::model::types::KType;
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        run(scope, "SIG OrderedSig = (VAL x :Number)");
        let sig = match scope.resolve_type("OrderedSig") {
            Some(KType::Signature { sig, .. }) => *sig,
            _ => panic!("OrderedSig should be a signature"),
        };
        assert_eq!(sig.path, "OrderedSig");
    }

    /// Body-statement forward-reference: a SIG body's `VAL x :SomeType` parks on an
    /// outer-scope-bound type alias and resolves once the alias finalizes.
    #[test]
    fn sig_body_parks_on_outer_placeholder() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        run(scope, "LET MyAlias = Number\nSIG Foo = (VAL x :MyAlias)");
        use crate::machine::model::types::KType;
        let sig = match scope.resolve_type("Foo") {
            Some(KType::Signature { sig, .. }) => *sig,
            _ => panic!("Foo should be a signature"),
        };
        let x = sig
            .decl_scope()
            .bindings()
            .lookup_type("x", None)
            .and_then(crate::machine::NameLookup::bound)
            .expect("VAL slot `x` must live in SIG's type table");
        assert!(
            matches!(x, KType::Number),
            "x's declared type must elaborate to Number through the alias, got {x:?}",
        );
    }

    /// A failing body statement surfaces as the SIG node's error and must not bind
    /// `Foo` (type side) in the parent scope.
    #[test]
    fn sig_body_error_short_circuits_finalize() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        run(scope, "SIG Foo = (VAL x :NonexistentType)");
        assert!(
            scope.resolve_type("Foo").is_none(),
            "Foo must not bind (type side) when its body errors",
        );
    }
}
