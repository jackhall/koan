//! Keyworded parameterized-type constructor builtins reached through the `:(...)` sigil.
//! See [type-language-via-dispatch](../../design/typing/type-language-via-dispatch.md).
//!
//! - `LIST OF :Type` → `Carried::Type(KType::List { .. })`
//! - `MAP :Type -> :Type` → `Carried::Type(KType::Dict { .. })`
//! - `FN <sig> -> :Type` → `Carried::Type(KType::KFunction { .. })`
//!
//! Fully-uppercase head keywords keep parameterized-type construction in
//! narrow candidate buckets so user-defined overloads of short connector words
//! like `OF` don't pay a bucket-walk cost on every dispatched parameterized type.

use crate::machine::model::KKind;
use crate::machine::model::{
    parse_typed_field_list_via_elaborator, Elaborator, FieldListOutcome, FieldNameKind,
};
use crate::machine::model::{KType, Record};
use crate::machine::{KError, KErrorKind, Scope};

use super::{arg, kw, sig};
use crate::machine::{defer_field_list_action_composed, BrandCompose};

/// Diagnostic context string for the shared field-list parser when it walks an `:(FN …)`
/// parameter list.
const FN_PARAMS_CONTEXT: &str = "FN parameters";

/// Field-name policy for an `:(FN …)` parameter list: capitalized `Type` param names like
/// `Ty` are admitted alongside ordinary identifiers.
const FN_PARAM_NAME_KIND: FieldNameKind = FieldNameKind::IdentifierOrType;

/// Fold the elaborated `(name, type)` pairs into the parameter record and wrap the
/// `KFunction` identity. Shared by the synchronous and dep-finish paths.
fn finalize_carrier(fields: Vec<(String, KType)>, ret: KType) -> KType {
    KType::function_type(Record::from_pairs(fields), Box::new(ret))
}

/// `Action`-harness twins of the type-constructor bodies. LIST/MAP/AS compose from resolved type
/// args directly (`Done`); FN routes the parameter list through [`build_carrier`], which either
/// resolves synchronously or defers via `defer_field_list_action_composed`.
mod action_bodies {
    use super::build_carrier;
    use crate::machine::model::{KKind, ProjectedSchema, RecursiveSet};
    use crate::machine::{require_ktype, Action, BodyCtx};

    use crate::machine::model::KType;
    use crate::machine::{KError, KErrorKind};

    /// LIST / MAP / AS read each embedded arg (`elem` / `k` / `v` / `applied` / `ctor`) as an
    /// owned `KType` and assemble the composite from those values, then allocate it into the
    /// step's own region through the single type door.
    pub(super) fn body_list_of<'a>(ctx: &BodyCtx<'a, '_>) -> Action<'a> {
        let elem = crate::try_action!(require_ktype(ctx.args, "elem"));
        Action::Done(Ok(ctx.ctx.alloc_type(KType::list(Box::new(elem)))))
    }

    pub(super) fn body_map<'a>(ctx: &BodyCtx<'a, '_>) -> Action<'a> {
        let k = crate::try_action!(require_ktype(ctx.args, "k"));
        let v = crate::try_action!(require_ktype(ctx.args, "v"));
        Action::Done(Ok(ctx
            .ctx
            .alloc_type(KType::dict(Box::new(k), Box::new(v)))))
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
        Action::Done(Ok(ctx
            .ctx
            .alloc_type(KType::constructor_apply(Box::new(ctor), vec![applied]))))
    }

    pub(super) fn body_fn<'a>(ctx: &BodyCtx<'a, '_>) -> Action<'a> {
        build_carrier(ctx, "sig", "ret")
    }
}

/// The composer [`build_carrier`]'s deferred arm hands to the field-list deferral. The return type
/// is owned data, so it rides the closure directly and pairs with the re-walked parameter list to
/// finish the `KFunction`.
fn ret_compose<'a>(ret: KType) -> BrandCompose<'a> {
    Box::new(move |fields| Ok(finalize_carrier(fields, ret)))
}

/// Walk the parameter list through the shared field-list parser (the same one UNION / NEWTYPE use),
/// so nested parameterized param types like `xs :(LIST OF Number)` sub-Dispatch and capitalized
/// param names like `Ty` are accepted. Resolves synchronously or defers via
/// [`defer_field_list_action_composed`] (no self-reference binder, no pending guard).
fn build_carrier<'a>(
    ctx: &crate::machine::BodyCtx<'a, '_>,
    sig_slot: &str,
    ret_slot: &str,
) -> crate::machine::Action<'a> {
    use crate::machine::{require_kexpression, require_ktype, Action};
    let sig_expr = crate::try_action!(require_kexpression(ctx.args, "FN", sig_slot));
    let ret = crate::try_action!(require_ktype(ctx.args, ret_slot));
    let mut elaborator = Elaborator::new(ctx.scope);
    match parse_typed_field_list_via_elaborator(
        &sig_expr,
        FN_PARAMS_CONTEXT,
        FN_PARAM_NAME_KIND,
        &mut elaborator,
        None,
    ) {
        FieldListOutcome::Done(fields) => {
            Action::Done(Ok(ctx.ctx.alloc_type(finalize_carrier(fields, ret))))
        }
        FieldListOutcome::Err(msg) => Action::Done(Err(KError::new(KErrorKind::ShapeError(msg)))),
        FieldListOutcome::Pending {
            park_producers,
            sub_dispatches,
        } => defer_field_list_action_composed(
            sig_expr,
            park_producers,
            sub_dispatches,
            FN_PARAMS_CONTEXT,
            FN_PARAM_NAME_KIND,
            Vec::new(),
            None,
            None,
            None,
            ret_compose(ret),
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
                // `OfKind(AnyType)` admits every type value — a `-> Ordered` signature return
                // and `-> Module` (which lowers to the empty signature) included.
                arg("ret", KType::OfKind(KKind::AnyType)),
            ],
        ),
        action_bodies::body_fn,
    );
}

#[cfg(test)]
mod tests {
    use crate::builtins::test_support::{
        parse_one, run, run_one_err, run_one_type, run_root_silent,
    };
    use crate::machine::model::{KKind, KType, Record};
    use crate::machine::run_root_storage;
    use crate::machine::{KErrorKind, Scope};

    #[test]
    fn list_of_number_lowers_to_list_number() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        let result = run_one_type(scope, parse_one(":(LIST OF Number)"));
        assert_eq!(*result, KType::list(Box::new(KType::Number)));
    }

    // A root-scope-bound `Wrap` TypeConstructor applied with `:(Number AS Wrap)`
    // lowers to `ConstructorApply(Wrap, [Number])`.
    #[test]
    fn apply_as_lowers_to_constructor_apply() {
        use crate::machine::model::{KKind, NominalSchema, RecursiveSet};
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
        scope.register_builtin_type(
            "Wrap".into(),
            KType::SetRef {
                set: wrap_set,
                index: 0,
            },
            BindingIndex::BUILTIN,
        );
        let result = run_one_type(scope, parse_one(":(Number AS Wrap)"));
        match result {
            KType::ConstructorApply { ctor, args, .. } => {
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
            KType::dict(Box::new(KType::Str), Box::new(KType::Number))
        );
    }

    #[test]
    fn fn_lowers_to_kfunction() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        let result = run_one_type(scope, parse_one(":(FN (x :Number, y :Str) -> Bool)"));
        assert_eq!(
            *result,
            KType::function_type(
                Record::from_pairs(vec![("x".into(), KType::Number), ("y".into(), KType::Str),]),
                Box::new(KType::Bool),
            )
        );
    }

    #[test]
    fn fn_nullary_lowers_to_kfunction() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        let result = run_one_type(scope, parse_one(":(FN () -> Number)"));
        assert_eq!(
            *result,
            KType::function_type(Record::new(), Box::new(KType::Number),)
        );
    }

    /// A functor — a module-returning function — types as an ordinary `KFunction`.
    // Param name `Ty` uses two letters because koan rejects single-uppercase-letter tokens.
    #[test]
    fn fn_with_type_param_and_module_return_lowers_to_kfunction() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        let result = run_one_type(scope, parse_one(":(FN (Ty :Signature) -> Module)"));
        assert_eq!(
            *result,
            KType::function_type(
                Record::from_pairs(vec![("Ty".into(), KType::OfKind(KKind::Signature))]),
                Box::new(KType::empty_signature()),
            )
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
            KType::function_type(
                Record::from_pairs(vec![("xs".into(), KType::list(Box::new(KType::Number)),)]),
                Box::new(KType::Bool),
            )
        );
    }

    /// A `:{…}` record type that mixes a scope-alias field (`:Wrapped`, resolved from the crossed
    /// scope during the deferred re-walk) with a sigil field (`:(LIST OF Number)`, which forces
    /// deferral) composes its `KType::Record` at the fold brand with both field types resolved. The
    /// scope-alias field reads through the brand-delivered scope envelope; the sigil field pops its
    /// sub-Dispatch carrier from the fed views.
    #[test]
    fn record_sigil_defers_and_mixes_scope_read_with_sub_dispatch() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        run(scope, "LET Wrapped = :{a :Number}");
        let result = run_one_type(scope, parse_one(":{x :Wrapped, y :(LIST OF Number)}"));
        assert_eq!(
            *result,
            KType::record(Box::new(Record::from_pairs(vec![
                (
                    "x".into(),
                    KType::record(Box::new(Record::from_pairs(vec![(
                        "a".into(),
                        KType::Number,
                    )]))),
                ),
                ("y".into(), KType::list(Box::new(KType::Number))),
            ]))),
        );
    }

    /// A deferred FN (the `:(LIST OF Number)` param forces deferral) whose return type names a
    /// `NEWTYPE` alias — a `SetRef` that is not region-free, so it cannot be rebuilt from a `'static`
    /// value — composes its `KType::KFunction` by cloning the return type out of its own carrier
    /// view (the `Some(Carried::Type(_))` compose arm). Were the return type to arrive without a
    /// carrier, `build_carrier`'s guard would error instead of producing a function type, so a
    /// successful compose proves the carrier-view path ran.
    #[test]
    fn fn_deferred_with_reaching_ret_composes_from_carrier_view() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        run(scope, "NEWTYPE Wrapped = :{a :Number}");
        let result = run_one_type(scope, parse_one(":(FN (xs :(LIST OF Number)) -> Wrapped)"));
        match result {
            KType::KFunction { params, ret, .. } => {
                assert_eq!(
                    params.get("xs"),
                    Some(&KType::list(Box::new(KType::Number))),
                    "the sigil param must lower to LIST OF Number",
                );
                assert_eq!(
                    ret.name(),
                    "Wrapped",
                    "the reaching return type must survive the carrier-view crossing",
                );
            }
            other => panic!("expected a KFunction carrier, got {other:?}"),
        }
    }

    /// A deferred record field whose sigil sub-Dispatch resolves to a non-type value (`:(1)` → the
    /// number `1`) surfaces the walker's shape error directly: `compose_field_list` propagates the
    /// rewalk's `Err` before any allocation runs.
    #[test]
    fn record_field_sub_dispatch_to_non_type_value_errors() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        let err = run_one_err(scope, parse_one(":{x :(1)}"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("resolved to non-type value")),
            "expected a non-type-value ShapeError through the deferred side-channel, got {err}",
        );
    }

    /// `t.name()` round-trips: rendering `expected` and re-running its surface form yields
    /// a type carrier equal to `expected`. The expected value is built at each call site so
    /// it shares the scope's lifetime, keeping the comparison off `'static`.
    fn assert_round_trips<'a>(scope: &'a Scope<'a>, expected: KType) {
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
            KType::function_type(
                Record::from_pairs(vec![("x".into(), KType::Number), ("y".into(), KType::Str)]),
                Box::new(KType::Bool),
            ),
        );
    }

    #[test]
    fn fn_nullary_round_trips() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        assert_round_trips(
            scope,
            KType::function_type(Record::new(), Box::new(KType::Any)),
        );
    }

    #[test]
    fn fn_nested_param_round_trips() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        assert_round_trips(
            scope,
            KType::function_type(
                Record::from_pairs(vec![("xs".into(), KType::list(Box::new(KType::Number)))]),
                Box::new(KType::Bool),
            ),
        );
    }

    #[test]
    fn fn_capitalized_param_round_trips_and_preserves_name() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        let expected = KType::function_type(
            Record::from_pairs(vec![("Ty".into(), KType::OfKind(KKind::Signature))]),
            Box::new(KType::empty_signature()),
        );
        // Param name `Ty` (capitalized, a `Type` token) must survive the round-trip.
        assert!(matches!(&expected, KType::KFunction { params, .. } if params.get("Ty").is_some()),);
        assert_round_trips(scope, expected);
    }

    /// `:(MAP Str -> Wrapped)` correlates a scalar-literal key with no carrier (`k`) and a
    /// reaching value type (`v`) by total operand position, not by carrier presence: the scalar
    /// lands in `k` and the reaching type survives the carrier-view crossing into `v`.
    #[test]
    fn map_scalar_key_reaching_value_correlates() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        run(scope, "NEWTYPE Wrapped = :{a :Number}");
        let result = run_one_type(scope, parse_one(":(MAP Str -> Wrapped)"));
        match result {
            KType::Dict {
                key: k, value: v, ..
            } => {
                assert_eq!(**k, KType::Str, "scalar key must lower to Str");
                assert_eq!(
                    v.name(),
                    "Wrapped",
                    "reaching value type must survive the carrier-view crossing",
                );
            }
            other => panic!("expected a Dict carrier, got {other:?}"),
        }
    }

    /// Mirror of `map_scalar_key_reaching_value_correlates`: the reaching type lands in `k` and
    /// the scalar lands in `v`, proving the correlation is positional, not carrier-presence-based.
    #[test]
    fn map_reaching_key_scalar_value_correlates() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        run(scope, "NEWTYPE Wrapped = :{a :Number}");
        let result = run_one_type(scope, parse_one(":(MAP Wrapped -> Str)"));
        match result {
            KType::Dict {
                key: k, value: v, ..
            } => {
                assert_eq!(
                    k.name(),
                    "Wrapped",
                    "reaching key type must survive the carrier-view crossing",
                );
                assert_eq!(**v, KType::Str, "scalar value must lower to Str");
            }
            other => panic!("expected a Dict carrier, got {other:?}"),
        }
    }

    /// A sync record type whose field names a `NEWTYPE` alias (`:Wrapped`, a `SetRef`) resolves in
    /// one ambient walk — no sigil field forces deferral — so its `KType::Record` composes directly
    /// from the elaborated pairs, where the `SetRef` field survives as owned data.
    #[test]
    fn record_sync_reaching_field_folds_at_brand() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        run(scope, "NEWTYPE Wrapped = :{a :Number}");
        let result = run_one_type(scope, parse_one(":{x :Wrapped}"));
        match result {
            KType::Record { fields: record, .. } => {
                let field = record.get("x").expect("record must have field x");
                assert_eq!(
                    field.name(),
                    "Wrapped",
                    "the reaching field must survive the sync brand re-fold",
                );
            }
            other => panic!("expected a Record, got {other:?}"),
        }
    }

    /// A sync FN whose parameter type names a `NEWTYPE` alias (`x :Wrapped`, a `SetRef`) resolves
    /// in one ambient walk, so its `KType::KFunction` composes directly from the elaborated pairs:
    /// the `SetRef` param survives as owned data alongside the plain `Bool` return type.
    #[test]
    fn fn_sync_reaching_param_folds_at_brand() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        run(scope, "NEWTYPE Wrapped = :{a :Number}");
        let result = run_one_type(scope, parse_one(":(FN (x :Wrapped) -> Bool)"));
        match result {
            KType::KFunction { params, ret, .. } => {
                assert_eq!(
                    params.get("x").map(|kt| kt.name()),
                    Some("Wrapped".to_string()),
                    "the SetRef param must survive the sync compose",
                );
                assert_eq!(
                    **ret,
                    KType::Bool,
                    "the region-free return type must be Bool"
                );
            }
            other => panic!("expected a KFunction, got {other:?}"),
        }
    }

    /// A sync FN whose return type names a `NEWTYPE` alias (`-> Wrapped`) resolves in one ambient
    /// walk, so its `KType::KFunction` composes directly from the elaborated pairs: the `ret`
    /// argument the caller closed over crosses into the composed carrier as owned data.
    #[test]
    fn fn_sync_reaching_ret_folds_at_brand() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        run(scope, "NEWTYPE Wrapped = :{a :Number}");
        let result = run_one_type(scope, parse_one(":(FN (x :Number) -> Wrapped)"));
        match result {
            KType::KFunction { params, ret, .. } => {
                assert_eq!(
                    params.get("x"),
                    Some(&KType::Number),
                    "the region-free param must be Number",
                );
                assert_eq!(
                    ret.name(),
                    "Wrapped",
                    "the SetRef return type must survive the carrier-view crossing",
                );
            }
            other => panic!("expected a KFunction, got {other:?}"),
        }
    }

    /// `:(LIST OF Wrapped)` lowers with the reaching elem type surviving the carrier-view
    /// crossing (the single-operand analog of the MAP correlation tests above).
    #[test]
    fn list_of_reaching_elem_lowers() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        run(scope, "NEWTYPE Wrapped = :{a :Number}");
        let result = run_one_type(scope, parse_one(":(LIST OF Wrapped)"));
        match result {
            KType::List { element: elem, .. } => {
                assert_eq!(
                    elem.name(),
                    "Wrapped",
                    "reaching elem type must survive the carrier-view crossing",
                );
            }
            other => panic!("expected a List carrier, got {other:?}"),
        }
    }
}
