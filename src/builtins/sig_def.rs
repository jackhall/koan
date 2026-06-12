//! `SIG <name:TypeExprRef> = <body:KExpression>` — declare a module signature (an
//! interface a module can be ascribed to). See
//! [design/typing/modules.md](../../design/typing/modules.md).
//!
//! Construction mirrors [`module_def`](super::module_def): body statements dispatch
//! against a fresh child scope on the outer scheduler; a `Combine` over those slots
//! captures the populated scope into a [`Signature`] value, allocates it in the
//! parent's arena, and binds it under the signature's name. Body declarations are
//! `LET name = (FN <signature> -> <return> = ...)` for operations and
//! `LET Carrier = TypeName` for abstract type declarations. The ascription operators
//! (`:|` / `:!`) iterate the stored scope at ascription time.

use crate::machine::model::types::KKind;
use crate::machine::model::values::Signature;
use crate::machine::model::KType;
use crate::machine::{
    ArgumentBundle, BindingIndex, BodyResult, CombineFinish, Frame, SchedulerHandle, Scope,
};

use super::{arg, err, kw, sig};
#[cfg(not(feature = "action-harness"))]
use super::register_builtin_with_binder;
use crate::machine::core::kfunction::argument_bundle::extract_bare_type_name;

pub fn body<'a, 's>(
    sched: &mut dyn SchedulerHandle<'a, 's>,
    mut bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let name = match extract_bare_type_name(&bundle, "name", "SIG") {
        Ok(n) => n,
        Err(e) => return err(e),
    };
    let body_expr = match bundle.extract_kexpression_or_shape_error("SIG", "body") {
        Ok(e) => e,
        Err(e) => return err(e),
    };

    let arena = sched.current_scope().arena;
    let decl_scope = arena.alloc_scope(Scope::child_under_sig(sched.current_scope(), name.clone()));

    let deps = sched.enter_body_block(decl_scope, body_expr);

    let bind_index = sched
        .current_lexical_chain()
        .map(|chain| BindingIndex::value(chain.index))
        .unwrap_or(BindingIndex::BUILTIN);
    let name_for_finish = name.clone();
    let finish: CombineFinish<'a> = Box::new(move |_sched, _results| {
        let arena = _sched.current_scope().arena;
        let sig: &'a Signature<'a> =
            arena.alloc_signature(Signature::new(name_for_finish.clone(), decl_scope));
        // One unified identity in `bindings.types`: `KType::Signature { sig, pinned_slots }`
        // is both the introspectable value (`decl_scope` via `sig`) and the dispatch
        // constraint. A slot annotation `:OrderedSig` means "any module satisfying
        // OrderedSig"; the signature value is recovered via `resolve_type_leaf_carrier`,
        // which synthesizes `KTypeValue(KType::Signature { .. })`. SIG doesn't join an SCC
        // type cycle, so the upsert's overwrite arm never fires — its insert-if-absent /
        // non-equal-Rebind behaviour (two `SIG Foo` in one scope error) carries here.
        let identity = KType::Signature {
            sig,
            pinned_slots: Vec::new(),
        };
        match _sched.current_scope().register_type_upsert(
            name_for_finish.clone(),
            identity,
            bind_index,
        ) {
            Ok(kt_ref) => BodyResult::ktype(arena.alloc_ktype(kt_ref.clone())),
            Err(e) => BodyResult::Err(e.with_frame(Frame::bare(
                "<signature>",
                format!("SIG {} body", name_for_finish),
            ))),
        }
    });
    let combine_id = sched.add_combine_here(deps, vec![], finish);
    BodyResult::DeferTo(combine_id)
}

/// `Action`-harness twin of [`body`]: mints the declaration scope, dispatches the SIG body block
/// against it (an `InScope` Combine dep), and the finish captures that scope into a [`Signature`]
/// and installs the `KType::Signature` identity into the parent scope.
#[cfg(feature = "action-harness")]
pub fn body_action<'a>(
    ctx: &crate::machine::core::kfunction::action::BodyCtx<'a, '_>,
) -> crate::machine::core::kfunction::action::Action<'a> {
    use crate::machine::core::kfunction::action::{arg_type, require_kexpression, Action, Cont, Dep, DepPlacement};
    use crate::machine::core::kfunction::argument_bundle::bare_type_name;
    use crate::machine::model::Carried;
    use crate::machine::{KError, KErrorKind};

    let name = match arg_type(ctx.args, "name") {
        Some(t) => match bare_type_name(t, "name", "SIG") {
            Ok(n) => n,
            Err(e) => return Action::Done(Err(e)),
        },
        None => return Action::Done(Err(KError::new(KErrorKind::MissingArg("name".to_string())))),
    };
    let body_expr = match require_kexpression(ctx.args, "SIG", "body") {
        Ok(e) => e,
        Err(e) => return Action::Done(Err(e)),
    };

    let decl_scope = ctx
        .scope
        .arena
        .alloc_scope(Scope::child_under_sig(ctx.scope, name.clone()));

    let bind_index = ctx
        .chain
        .as_ref()
        .map(|chain| BindingIndex::value(chain.index))
        .unwrap_or(BindingIndex::BUILTIN);
    let name_for_finish = name;
    let finish: Cont<'a> = Box::new(move |fctx, _results| {
        let sig: &'a Signature<'a> = fctx
            .scope
            .arena
            .alloc_signature(Signature::new(name_for_finish.clone(), decl_scope));
        let identity = KType::Signature {
            sig,
            pinned_slots: Vec::new(),
        };
        match fctx
            .scope
            .register_type_upsert(name_for_finish.clone(), identity, bind_index)
        {
            Ok(kt_ref) => {
                Action::Done(Ok(Carried::Type(fctx.scope.arena.alloc_ktype(kt_ref.clone()))))
            }
            Err(e) => Action::Done(Err(e.with_frame(Frame::bare(
                "<signature>",
                format!("SIG {} body", name_for_finish),
            )))),
        }
    });
    Action::Combine {
        deps: vec![Dep::Dispatch {
            expr: body_expr,
            placement: DepPlacement::InScope(decl_scope),
        }],
        finish,
    }
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    let signature = sig(
        KType::OfKind(KKind::Signature),
        vec![
            kw("SIG"),
            arg("name", KType::OfKind(KKind::Proper)),
            kw("="),
            arg("body", KType::KExpression),
        ],
    );
    #[cfg(feature = "action-harness")]
    crate::builtins::register_action_builtin_full(
        scope, "SIG", signature, body_action, Some(super::type_part_binder_name), None, false,
    );
    #[cfg(not(feature = "action-harness"))]
    register_builtin_with_binder(scope, "SIG", signature, body, Some(super::type_part_binder_name));
}

#[cfg(test)]
mod tests {
    use crate::builtins::test_support::{run, run_root_silent};
    use crate::machine::RuntimeArena;
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
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
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
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
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
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
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
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "SIG Foo = (VAL x :NonexistentType)");
        assert!(
            scope.resolve_type("Foo").is_none(),
            "Foo must not bind (type side) when its body errors",
        );
    }
}
