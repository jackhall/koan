//! Keyworded parameterized-type constructor builtins reached through the `:(...)` sigil.
//! See [type-language-via-dispatch](../../design/typing/type-language-via-dispatch.md).
//!
//! - `LIST OF :Type` → `Carried::Type(KType::List(_))`
//! - `MAP :Type -> :Type` → `Carried::Type(KType::Dict(_, _))`
//! - `FN <sig> -> :Type` → `Carried::Type(KType::KFunction { .. })`
//! - `FUNCTOR <sig> -> :Type` → `Carried::Type(KType::KFunctor { .. })`
//!
//! Fully-uppercase head keywords keep parameterized-type construction in
//! narrow candidate buckets so user-defined functors overloading short
//! connector words like `OF` don't pay a bucket-walk cost on every dispatched
//! parameterized type.

use crate::machine::model::types::KKind;
use crate::machine::model::types::{
    parse_typed_field_list_via_elaborator, Elaborator, FieldListOutcome, FieldNameKind,
};
use crate::machine::model::{KType, Record};
use crate::machine::{KError, KErrorKind, Scope};

use super::{arg, kw, sig};
use crate::machine::execute::defer_field_list_action;

/// Which carrier the shared field-list path builds. All three ride the same parser and
/// dep-finish/defer machinery; they differ only in the `KType` they fold their fields into,
/// the diagnostic context string, and the field-name policy (both admit capitalized `Type`
/// param names like `Ty`). The record type `:{…}` is structural — a first-class
/// `ExpressionPart::RecordType` the dispatcher folds to `KType::Record` directly — so it is
/// not a carrier kind here.
#[derive(Clone, Copy)]
enum CarrierKind {
    Function,
    Functor,
}

impl CarrierKind {
    fn context(self) -> &'static str {
        match self {
            CarrierKind::Function => "FN parameters",
            CarrierKind::Functor => "FUNCTOR parameters",
        }
    }

    fn field_name_kind(self) -> FieldNameKind {
        match self {
            CarrierKind::Function | CarrierKind::Functor => FieldNameKind::IdentifierOrType,
        }
    }
}

/// Fold the elaborated `(name, type)` pairs into the parameter record and wrap the
/// `KFunction` / `KFunctor` identity in a `Carried::Type`. Shared by the synchronous and
/// dep-finish paths.
fn finalize_carrier<'a>(
    fields: Vec<(String, KType<'a>)>,
    ret: KType<'a>,
    kind: CarrierKind,
) -> KType<'a> {
    let record = Record::from_pairs(fields);
    match kind {
        // A `:(FUNCTOR …)` type-position annotation is a shape, not a bound
        // callable, so it carries no body.
        CarrierKind::Functor => KType::KFunctor {
            params: record,
            ret: Box::new(ret),
            body: None,
        },
        CarrierKind::Function => KType::KFunction {
            params: record,
            ret: Box::new(ret),
        },
    }
}

/// `Action`-harness twins of the type-constructor bodies. LIST/MAP/AS fold resolved type args
/// directly (`Done`); FN/FUNCTOR route the parameter list through [`build_carrier`], which
/// either folds synchronously or defers via [`defer_field_list_action`].
mod action_bodies {
    use super::{build_carrier, CarrierKind};
    use crate::machine::core::kfunction::action::{require_ktype, Action, BodyCtx};
    use crate::machine::core::KoanStepContextExt;
    use crate::machine::model::types::{KKind, ProjectedSchema, RecursiveSet};
    use crate::machine::DeliveredCarried;

    use crate::machine::model::KType;
    use crate::machine::{KError, KErrorKind};

    /// LIST / MAP / AS fold the carrier(s) of the arg(s) their result `KType` embeds by clone:
    /// `elem` / `k` / `v` / `applied` / `ctor` can be any caller-supplied type (a bound `KFunctor`,
    /// a `SetRef` into a nominal set, ...), so the clone can carry a borrow into the argument's own
    /// producer region — [`BodyCtx::arg_carrier`] names that reach, folded in via `alloc_type_with`
    /// (`None` — a scalar-literal argument — folds nothing, matching `alloc_type`).
    pub(super) fn body_list_of<'a>(ctx: &BodyCtx<'a, '_>) -> Action<'a> {
        let elem = crate::try_action!(require_ktype(ctx.args, "elem"));
        let carriers: Vec<&DeliveredCarried> = ctx.arg_carrier("elem").into_iter().collect();
        Action::Done(Ok(ctx
            .ctx
            .alloc_type_with(&carriers, KType::List(Box::new(elem)))))
    }

    pub(super) fn body_map<'a>(ctx: &BodyCtx<'a, '_>) -> Action<'a> {
        let k = crate::try_action!(require_ktype(ctx.args, "k"));
        let v = crate::try_action!(require_ktype(ctx.args, "v"));
        let carriers: Vec<&DeliveredCarried> = [ctx.arg_carrier("k"), ctx.arg_carrier("v")]
            .into_iter()
            .flatten()
            .collect();
        Action::Done(Ok(ctx
            .ctx
            .alloc_type_with(&carriers, KType::Dict(Box::new(k), Box::new(v)))))
    }

    pub(super) fn body_apply_as<'a>(ctx: &BodyCtx<'a, '_>) -> Action<'a> {
        let applied = crate::try_action!(require_ktype(ctx.args, "applied"));
        let ctor = crate::try_action!(require_ktype(ctx.args, "ctor"));
        let param_count = match &ctor {
            KType::SetRef { set, index } if set.member(*index).kind == KKind::TypeConstructor => {
                match RecursiveSet::projected_schema(set, *index) {
                    ProjectedSchema::TypeConstructor { param_names, .. } => param_names.len(),
                    _ => unreachable!(
                        "TypeConstructor-kind member projects a TypeConstructor schema"
                    ),
                }
            }
            other => {
                return Action::Done(Err(KError::new(KErrorKind::ShapeError(format!(
                    "right-hand side of `AS` must be a type constructor, got `{}`",
                    other.name(),
                )))))
            }
        };
        if param_count != 1 {
            return Action::Done(Err(KError::new(KErrorKind::ShapeError(format!(
                "`{}` takes {param_count} type arguments; the `AS` form supplies one, so \
                 multi-parameter application is not yet supported",
                ctor.name(),
            )))));
        }
        let carriers: Vec<&DeliveredCarried> =
            [ctx.arg_carrier("applied"), ctx.arg_carrier("ctor")]
                .into_iter()
                .flatten()
                .collect();
        Action::Done(Ok(ctx.ctx.alloc_type_with(
            &carriers,
            KType::ConstructorApply {
                ctor: Box::new(ctor),
                args: vec![applied],
            },
        )))
    }

    pub(super) fn body_fn<'a>(ctx: &BodyCtx<'a, '_>) -> Action<'a> {
        build_carrier(ctx, "sig", "ret", CarrierKind::Function)
    }

    pub(super) fn body_functor<'a>(ctx: &BodyCtx<'a, '_>) -> Action<'a> {
        build_carrier(ctx, "sig", "ret", CarrierKind::Functor)
    }
}

/// Walk the parameter list through the shared field-list parser (the same one UNION / NEWTYPE use),
/// so nested parameterized param types like `xs :(LIST OF Number)` sub-Dispatch and capitalized
/// FUNCTOR param names like `Ty` are accepted. Folds synchronously or defers via
/// [`defer_field_list_action`] (no self-reference binder, no pending guard).
fn build_carrier<'a>(
    ctx: &crate::machine::core::kfunction::action::BodyCtx<'a, '_>,
    sig_slot: &str,
    ret_slot: &str,
    kind: CarrierKind,
) -> crate::machine::core::kfunction::action::Action<'a> {
    use crate::machine::core::kfunction::action::{require_kexpression, require_ktype, Action};
    use crate::machine::core::KoanStepContextExt;
    let sig_expr = crate::try_action!(require_kexpression(ctx.args, "FN", sig_slot));
    let ret = crate::try_action!(require_ktype(ctx.args, ret_slot));
    let mut elaborator = Elaborator::new(ctx.scope);
    match parse_typed_field_list_via_elaborator(
        &sig_expr,
        kind.context(),
        kind.field_name_kind(),
        &mut elaborator,
        None,
    ) {
        FieldListOutcome::Done(fields) => {
            let kt = finalize_carrier(fields, ret, kind);
            Action::Done(Ok(crate::try_action!(ctx.ctx.alloc_type_pure(kt))))
        }
        FieldListOutcome::Err(msg) => Action::Done(Err(KError::new(KErrorKind::ShapeError(msg)))),
        FieldListOutcome::Pending {
            park_producers,
            sub_dispatches,
        } => defer_field_list_action(
            sig_expr,
            park_producers,
            sub_dispatches,
            kind.context(),
            kind.field_name_kind(),
            Vec::new(),
            None,
            None,
            None,
            Box::new(move |fctx, fields, carriers| {
                let kt = finalize_carrier(fields, ret, kind);
                Ok(fctx.ctx.alloc_type_with(carriers, kt))
            }),
        ),
    }
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    use crate::builtins::register_builtin;
    register_builtin(
        scope,
        "LIST",
        sig(
            KType::OfKind(KKind::AnyType),
            vec![
                kw("LIST"),
                kw("OF"),
                arg("elem", KType::OfKind(KKind::AnyType)),
            ],
        ),
        action_bodies::body_list_of,
    );
    register_builtin(
        scope,
        "MAP",
        sig(
            KType::OfKind(KKind::AnyType),
            vec![
                kw("MAP"),
                arg("k", KType::OfKind(KKind::AnyType)),
                kw("->"),
                arg("v", KType::OfKind(KKind::AnyType)),
            ],
        ),
        action_bodies::body_map,
    );
    register_builtin(
        scope,
        "AS",
        sig(
            KType::OfKind(KKind::AnyType),
            vec![
                arg("applied", KType::OfKind(KKind::AnyType)),
                kw("AS"),
                arg("ctor", KType::OfKind(KKind::AnyType)),
            ],
        ),
        action_bodies::body_apply_as,
    );
    register_builtin(
        scope,
        "FN",
        sig(
            KType::OfKind(KKind::AnyType),
            vec![
                kw("FN"),
                arg("sig", KType::KExpression),
                kw("->"),
                arg("ret", KType::OfKind(KKind::AnyType)),
            ],
        ),
        action_bodies::body_fn,
    );
    register_builtin(
        scope,
        "FUNCTOR",
        sig(
            KType::OfKind(KKind::AnyType),
            vec![
                kw("FUNCTOR"),
                arg("sig", KType::KExpression),
                kw("->"),
                arg("ret", KType::OfKind(KKind::AnyType)),
            ],
        ),
        action_bodies::body_functor,
    );
}

#[cfg(test)]
mod tests {
    use crate::builtins::test_support::{parse_one, run_one_type, run_root_silent};
    use crate::machine::core::run_root_storage;
    use crate::machine::core::StoredReach;
    use crate::machine::model::{KKind, KType, Record};
    use crate::machine::Scope;

    #[test]
    fn list_of_number_lowers_to_list_number() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        let result = run_one_type(scope, parse_one(":(LIST OF Number)"));
        assert_eq!(*result, KType::List(Box::new(KType::Number)));
    }

    // A root-scope-bound `Wrap` TypeConstructor applied with `:(Number AS Wrap)`
    // lowers to `ConstructorApply(Wrap, [Number])`.
    #[test]
    fn apply_as_lowers_to_constructor_apply() {
        use crate::machine::model::types::{KKind, NominalSchema, RecursiveSet};
        use crate::machine::{BindingIndex, ScopeId};
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        let wrap_set = RecursiveSet::singleton(
            "Wrap".into(),
            ScopeId::from_raw(0, 0xC0DE),
            NominalSchema::TypeConstructor {
                schema: std::collections::HashMap::new(),
                param_names: vec!["Type".into()],
            },
        );
        scope.register_type(
            "Wrap".into(),
            KType::SetRef {
                set: wrap_set,
                index: 0,
            },
            BindingIndex::BUILTIN,
            StoredReach::empty(),
        );
        let result = run_one_type(scope, parse_one(":(Number AS Wrap)"));
        match result {
            KType::ConstructorApply { ctor, args } => {
                match ctor.as_ref() {
                    KType::SetRef { set, index } => {
                        assert_eq!(set.member(*index).kind, KKind::TypeConstructor);
                    }
                    other => panic!("expected SetRef ctor, got {other:?}"),
                }
                assert_eq!(*args, vec![KType::Number]);
            }
            other => panic!("expected ConstructorApply, got {other:?}"),
        }
    }

    #[test]
    fn map_str_number_lowers_to_dict() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        let result = run_one_type(scope, parse_one(":(MAP Str -> Number)"));
        assert_eq!(
            *result,
            KType::Dict(Box::new(KType::Str), Box::new(KType::Number))
        );
    }

    #[test]
    fn fn_lowers_to_kfunction() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        let result = run_one_type(scope, parse_one(":(FN (x :Number, y :Str) -> Bool)"));
        assert_eq!(
            *result,
            KType::KFunction {
                params: Record::from_pairs(vec![
                    ("x".into(), KType::Number),
                    ("y".into(), KType::Str),
                ]),
                ret: Box::new(KType::Bool),
            }
        );
    }

    #[test]
    fn fn_nullary_lowers_to_kfunction() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        let result = run_one_type(scope, parse_one(":(FN () -> Number)"));
        assert_eq!(
            *result,
            KType::KFunction {
                params: Record::new(),
                ret: Box::new(KType::Number),
            }
        );
    }

    // Param name `Ty` uses two letters because koan rejects single-uppercase-letter tokens.
    #[test]
    fn functor_lowers_to_kfunctor() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        let result = run_one_type(scope, parse_one(":(FUNCTOR (Ty :Signature) -> Module)"));
        assert_eq!(
            *result,
            KType::KFunctor {
                params: Record::from_pairs(vec![("Ty".into(), KType::OfKind(KKind::Signature))]),
                ret: Box::new(KType::OfKind(KKind::Module)),
                body: None,
            }
        );
    }

    /// A nested parameterized param type (`:(LIST OF Number)`) sub-Dispatches through the
    /// shared field-list parser and lands in the parameter record.
    #[test]
    fn fn_with_nested_list_param_lowers_to_kfunction() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        let result = run_one_type(scope, parse_one(":(FN (xs :(LIST OF Number)) -> Bool)"));
        assert_eq!(
            *result,
            KType::KFunction {
                params: Record::from_pairs(vec![(
                    "xs".into(),
                    KType::List(Box::new(KType::Number)),
                )]),
                ret: Box::new(KType::Bool),
            }
        );
    }

    /// `t.name()` round-trips: rendering `expected` and re-running its surface form yields
    /// a type carrier equal to `expected`. The expected value is built at each call site so
    /// it shares the scope's lifetime, keeping the comparison off `'static`.
    fn assert_round_trips<'a>(scope: &'a Scope<'a>, expected: KType<'a>) {
        let rendered = expected.name();
        let result = run_one_type(scope, parse_one(&rendered));
        assert_eq!(
            *result, expected,
            "round-trip of `{rendered}` did not reproduce the original KType",
        );
    }

    #[test]
    fn fn_multi_param_round_trips() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        assert_round_trips(
            scope,
            KType::KFunction {
                params: Record::from_pairs(vec![
                    ("x".into(), KType::Number),
                    ("y".into(), KType::Str),
                ]),
                ret: Box::new(KType::Bool),
            },
        );
    }

    #[test]
    fn fn_nullary_round_trips() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        assert_round_trips(
            scope,
            KType::KFunction {
                params: Record::new(),
                ret: Box::new(KType::Any),
            },
        );
    }

    #[test]
    fn fn_nested_param_round_trips() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        assert_round_trips(
            scope,
            KType::KFunction {
                params: Record::from_pairs(vec![(
                    "xs".into(),
                    KType::List(Box::new(KType::Number)),
                )]),
                ret: Box::new(KType::Bool),
            },
        );
    }

    #[test]
    fn functor_capitalized_param_round_trips_and_preserves_name() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        let expected = KType::KFunctor {
            params: Record::from_pairs(vec![("Ty".into(), KType::OfKind(KKind::Signature))]),
            ret: Box::new(KType::OfKind(KKind::Module)),
            body: None,
        };
        // Param name `Ty` (capitalized, a `Type` token) must survive the round-trip.
        assert!(matches!(&expected, KType::KFunctor { params, .. } if params.get("Ty").is_some()),);
        assert_round_trips(scope, expected);
    }
}
