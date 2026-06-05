//! `SIG <name:TypeExprRef> = <body:KExpression>` — declare a module signature (an
//! interface a module can be ascribed to). See
//! [design/typing/modules.md](../../design/typing/modules.md).
//!
//! Construction mirrors [`module_def`](super::module_def): body statements dispatch
//! against a fresh child scope on the outer scheduler; a `Combine` over those slots
//! captures the populated scope into a [`Signature`] value, allocates it in the
//! parent's arena, and binds it under the signature's name. Body declarations are
//! `LET name = (FN <signature> -> <return> = ...)` for operations and
//! `LET Type = TypeName` for abstract type declarations. The ascription operators
//! (`:|` / `:!`) iterate the stored scope at ascription time.

use crate::machine::model::values::Signature;
use crate::machine::model::{KObject, KType};
use crate::machine::{
    ArgumentBundle, BindingIndex, BodyResult, CombineFinish, Frame, KError, KErrorKind,
    SchedulerHandle, Scope,
};

use crate::machine::model::ast::KExpression;

use super::{arg, err, kw, register_builtin_with_binder, sig};
use crate::machine::core::kfunction::argument_bundle::{
    extract_bare_type_name, extract_kexpression,
};

pub fn body<'a>(
    scope: &'a Scope<'a>,
    sched: &mut dyn SchedulerHandle<'a>,
    mut bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let name = match extract_bare_type_name(&bundle, "name", "SIG") {
        Ok(n) => n,
        Err(e) => return err(e),
    };
    let body_expr = match extract_kexpression(&mut bundle, "body") {
        Some(e) => e,
        None => {
            return err(KError::new(KErrorKind::ShapeError(
                "SIG body slot must be a parenthesized expression".to_string(),
            )));
        }
    };

    let arena = scope.arena;
    let decl_scope = arena.alloc_scope(Scope::child_under_sig(scope, name.clone()));

    let deps = sched.enter_body_block(decl_scope, body_expr);

    let bind_index = sched
        .current_lexical_chain()
        .map(|chain| BindingIndex::value(chain.index))
        .unwrap_or(BindingIndex::BUILTIN);
    let name_for_finish = name.clone();
    let finish: CombineFinish<'a> = Box::new(move |parent_scope, _sched, _results| {
        let arena = parent_scope.arena;
        let sig: &'a Signature<'a> =
            arena.alloc_signature(Signature::new(name_for_finish.clone(), decl_scope));
        // One unified identity in `bindings.types`: `KType::Signature { sig, pinned_slots }`
        // is both the introspectable value (`decl_scope` via `sig`) and the dispatch
        // constraint. A slot annotation `:OrderedSig` means "any module satisfying
        // OrderedSig"; the signature value is recovered via `coerce_type_token_value`,
        // which synthesizes `KTypeValue(KType::Signature { .. })`. SIG doesn't join an SCC
        // type cycle, so the upsert's overwrite arm never fires — its insert-if-absent /
        // non-equal-Rebind behaviour (two `SIG Foo` in one scope error) carries here.
        let identity = KType::Signature {
            sig,
            pinned_slots: Vec::new(),
        };
        match parent_scope.register_type_upsert(name_for_finish.clone(), identity, bind_index) {
            Ok(kt_ref) => {
                BodyResult::Value(arena.alloc_object(KObject::KTypeValue(kt_ref.clone())))
            }
            Err(e) => BodyResult::Err(e.with_frame(Frame::bare(
                "<signature>",
                format!("SIG {} body", name_for_finish),
            ))),
        }
    });
    let combine_id = sched.add_combine(deps, vec![], scope, finish);
    BodyResult::DeferTo(combine_id)
}

/// Dispatch-time placeholder extractor: pulls the signature name from `parts[1]`'s
/// `Type(t)` token. Same shape as STRUCT / MODULE / named UNION.
pub(crate) fn binder_name(expr: &KExpression<'_>) -> Option<String> {
    expr.binder_name_from_type_part()
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    register_builtin_with_binder(
        scope,
        "SIG",
        sig(
            KType::AnySignature,
            vec![
                kw("SIG"),
                arg("name", KType::TypeExprRef),
                kw("="),
                arg("body", KType::KExpression),
            ],
        ),
        body,
        Some(binder_name),
    );
}

#[cfg(test)]
mod tests {
    use crate::builtins::test_support::{run, run_root_silent};
    use crate::machine::model::KObject;
    use crate::machine::RuntimeArena;
    use crate::parse::parse;

    #[test]
    fn binder_name_extracts_sig_name() {
        let mut exprs = parse("SIG OrderedSig = (VAL x :Number)").expect("parse should succeed");
        let expr = exprs.remove(0);
        let name = super::binder_name(&expr);
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
        let inner = sig.decl_scope().bindings().data();
        let (x, _) = inner.get("x").expect("x must live in SIG's data");
        assert!(
            matches!(x, KObject::KTypeValue(crate::machine::model::KType::Number)),
            "x's declared type must elaborate to Number through the alias, got {:?}",
            x.ktype(),
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
