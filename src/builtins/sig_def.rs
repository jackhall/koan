//! `SIG <name:TypeExprRef> = <body:KExpression>` — declare a module signature (an
//! interface a module can be ascribed to). See
//! [design/typing/modules.md](../../design/typing/modules.md).
//!
//! Construction mirrors [`module_def`](super::module_def): body statements dispatch
//! against a fresh child scope on the outer scheduler; a `Combine` over those slots
//! captures the populated scope into a [`Signature`] value, allocates it in the
//! parent's arena, and binds it under the signature's name. Body declarations are
//! `LET name = (FN <signature> -> <return> = ...)` for operations and
//! `LET Type = TypeExpr` for abstract type declarations. The ascription operators
//! (`:|` / `:!`) iterate the stored scope at ascription time.

use crate::machine::model::{KObject, KType};
use crate::machine::{
    ArgumentBundle, BindingIndex, BodyResult, CombineFinish, Frame, KError, KErrorKind, Scope,
    SchedulerHandle,
};
use crate::machine::model::values::Signature;

use crate::machine::model::ast::KExpression;

use crate::machine::core::kfunction::argument_bundle::{extract_bare_type_name, extract_kexpression};
use super::{arg, err, kw, register_nominal_binder, sig};

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
        .map(|chain| BindingIndex::nominal(chain.index))
        .unwrap_or(BindingIndex::BUILTIN);
    let name_for_finish = name.clone();
    let finish: CombineFinish<'a> = Box::new(move |parent_scope, _sched, _results| {
        let arena = parent_scope.arena;
        let sig: &'a Signature<'a> =
            arena.alloc_signature(Signature::new(name_for_finish.clone(), decl_scope));
        // The signature value rides `KTypeValue(KType::Signature(s))`; the type-side
        // identity carries the *constraint* form `SatisfiesSignature` so slot
        // annotations `:OrderedSig` mean "any module satisfying OrderedSig" rather
        // than "this signature value itself."
        let identity = KType::SatisfiesSignature {
            sig_id: sig.sig_id(),
            sig_path: name_for_finish.clone(),
            pinned_slots: Vec::new(),
        };
        let sig_obj: &'a KObject<'a> = arena.alloc(KObject::KTypeValue(KType::Signature(sig)));
        match parent_scope.register_nominal(name_for_finish.clone(), identity, sig_obj, bind_index)
        {
            Ok(obj) => BodyResult::Value(obj),
            Err(e) => BodyResult::Err(
                e.with_frame(Frame::bare("<signature>", format!("SIG {} body", name_for_finish))),
            ),
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
    register_nominal_binder(
        scope,
        "SIG",
        sig(KType::AnySignature, vec![
            kw("SIG"),
            arg("name", KType::TypeExprRef),
            kw("="),
            arg("body", KType::KExpression),
        ]),
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
        let data = scope.bindings().data();
        assert!(matches!(
            data.get("OrderedSig").map(|(o, _)| *o),
            Some(KObject::KTypeValue(KType::Signature(_)))
        ));
    }

    #[test]
    fn sig_path_records_name() {
        use crate::machine::model::types::KType;
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "SIG OrderedSig = (VAL x :Number)");
        let data = scope.bindings().data();
        let sig = match data.get("OrderedSig").map(|(o, _)| *o) {
            Some(KObject::KTypeValue(KType::Signature(s))) => *s,
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
        let data = scope.bindings().data();
        use crate::machine::model::types::KType;
        let sig = match data.get("Foo").map(|(o, _)| *o) {
            Some(KObject::KTypeValue(KType::Signature(s))) => *s,
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
    /// `Foo` in the parent scope.
    #[test]
    fn sig_body_error_short_circuits_finalize() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "SIG Foo = (VAL x :NonexistentType)");
        assert!(
            scope.bindings().data().get("Foo").is_none(),
            "Foo must not bind when its body errors",
        );
    }

}
