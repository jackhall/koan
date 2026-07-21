//! Keyworded parameterized-type constructor builtins reached through the `:(...)` sigil.
//! See [type-language-via-dispatch](../../design/typing/type-language-via-dispatch.md).
//!
//! - `LIST OF :Type` → `Carried::Type` of an interned `List` handle
//! - `MAP :Type -> :Type` → `Carried::Type` of an interned `Dict` handle
//! - `FN <sig> -> :Type` → `Carried::Type` of an interned function-type handle
//!
//! Fully-uppercase head keywords keep parameterized-type construction in
//! narrow candidate buckets so user-defined overloads of short connector words
//! like `OF` don't pay a bucket-walk cost on every dispatched parameterized type.

use crate::machine::model::KKind;
use crate::machine::model::TypeRegistry;
use crate::machine::model::{
    parse_typed_field_list_via_elaborator, Elaborator, FieldListContext, FieldListOutcome,
    FieldNameKind,
};
use crate::machine::model::{KType, Record};
use crate::machine::{KError, KErrorKind, Scope};

use super::{arg, kw, sig};
use crate::machine::{BrandCompose, FieldListDeferral};

/// Diagnostic nouns for the shared field-list parser when it walks an `:(FN …)` parameter list.
const FN_PARAMS_CONTEXT: FieldListContext = FieldListContext::FN_TYPE_PARAMETERS;

/// Field-name policy for an `:(FN …)` parameter list: capitalized `Type` param names like
/// `Ty` are admitted alongside ordinary identifiers.
const FN_PARAM_NAME_KIND: FieldNameKind = FieldNameKind::IdentifierOrType;

/// Fold the elaborated `(name, type)` pairs into the parameter record and intern the function
/// type. Shared by the synchronous and dep-finish paths.
fn finalize_carrier(fields: Vec<(String, KType)>, ret: KType, types: &TypeRegistry) -> KType {
    types.function_type(Record::from_pairs(fields), ret)
}

/// Reject a bare type constructor in a type-language argument position that demands kind `*`:
/// a list's element, a dict's key and value, a function type's return. Each names the type of a
/// value, so each must be a proper type. The parameter list of an `:(FN …)` is checked inside the
/// shared field-list walker instead, alongside every other record-shaped schema.
fn require_proper_type(
    kt: KType,
    position: &str,
    types: &crate::machine::model::TypeRegistry,
) -> Result<(), KError> {
    match crate::machine::model::unsaturated_constructor_message(kt, position, types) {
        Some(message) => Err(KError::new(KErrorKind::ShapeError(message))),
        None => Ok(()),
    }
}

/// `Action`-harness twins of the type-constructor bodies. LIST/MAP/AS compose from resolved type
/// args directly (`Done`); FN routes the parameter list through [`build_carrier`], which either
/// resolves synchronously or defers via a `FieldListDeferral` finished through `action_composed`.
mod action_bodies {
    use super::{build_carrier, require_proper_type};
    use crate::machine::model::constructor_param_names;
    use crate::machine::{require_ktype, Action, BodyCtx};

    use crate::machine::model::Record;
    use crate::machine::{KError, KErrorKind};

    /// LIST / MAP / AS read each embedded arg (`elem` / `k` / `v` / `applied` / `ctor`) as an
    /// owned `KType` and assemble the composite from those values, then allocate it into the
    /// step's own region through the single type door.
    pub(super) fn body_list_of<'a>(ctx: &BodyCtx<'a, '_>) -> Action<'a> {
        let elem = crate::try_action!(require_ktype(ctx.args, "elem", ctx.types));
        crate::try_action!(require_proper_type(
            elem,
            "the element type of `LIST OF`",
            ctx.types
        ));
        let list = ctx.types.list(elem);
        Action::Done(Ok(ctx.ctx.type_carried(list)))
    }

    pub(super) fn body_map<'a>(ctx: &BodyCtx<'a, '_>) -> Action<'a> {
        let k = crate::try_action!(require_ktype(ctx.args, "k", ctx.types));
        let v = crate::try_action!(require_ktype(ctx.args, "v", ctx.types));
        crate::try_action!(require_proper_type(k, "the key type of `MAP`", ctx.types));
        crate::try_action!(require_proper_type(v, "the value type of `MAP`", ctx.types));
        let dict = ctx.types.dict(k, v);
        Action::Done(Ok(ctx.ctx.type_carried(dict)))
    }

    pub(super) fn body_apply_as<'a>(ctx: &BodyCtx<'a, '_>) -> Action<'a> {
        let applied = crate::try_action!(require_ktype(ctx.args, "applied", ctx.types));
        let ctor = crate::try_action!(require_ktype(ctx.args, "ctor", ctx.types));
        // A declared family and a SIG's abstract constructor slot both name their parameters.
        let Some(param_names) = constructor_param_names(ctor, ctx.types) else {
            return Action::Done(Err(KError::new(KErrorKind::ShapeError(format!(
                "right-hand side of `AS` must be a type constructor, got `{}`",
                ctor.name(ctx.types),
            )))));
        };
        let [param_name] = &param_names[..] else {
            return Action::Done(Err(KError::new(KErrorKind::ShapeError(format!(
                "`{}` takes {} type arguments; the `AS` form supplies one, so \
                 multi-parameter application is not yet supported",
                ctor.name(ctx.types),
                param_names.len(),
            )))));
        };
        // `AS` is arity-1 sugar: the applied type fills the constructor's sole parameter, so
        // `:(Number AS Wrap)` elaborates exactly as `:(Wrap {Elem = Number})` does.
        let args = Record::from_pairs([(param_name.clone(), applied)]);
        let apply = ctx.types.constructor_apply(ctor, args);
        Action::Done(Ok(ctx.ctx.type_carried(apply)))
    }

    pub(super) fn body_fn<'a>(ctx: &BodyCtx<'a, '_>) -> Action<'a> {
        build_carrier(ctx, "sig", "ret")
    }
}

/// The composer [`build_carrier`]'s deferred arm hands to the field-list deferral. The return type
/// is owned data, so it rides the closure directly and pairs with the re-walked parameter list to
/// finish the `KFunction`.
fn ret_compose<'a>(ret: KType) -> BrandCompose<'a> {
    Box::new(move |fields, types| Ok(finalize_carrier(fields, ret, types)))
}

/// Walk the parameter list through the shared field-list parser (the same one UNION / NEWTYPE use),
/// so nested parameterized param types like `xs :(LIST OF Number)` sub-Dispatch and capitalized
/// param names like `Ty` are accepted. Resolves synchronously or defers via
/// [`FieldListDeferral::action_composed`] (no self-reference binder, no pending guard).
fn build_carrier<'a>(
    ctx: &crate::machine::BodyCtx<'a, '_>,
    sig_slot: &str,
    ret_slot: &str,
) -> crate::machine::Action<'a> {
    use crate::machine::{require_kexpression, require_ktype, Action};
    let sig_expr = crate::try_action!(require_kexpression(ctx.args, "FN", sig_slot));
    let ret = crate::try_action!(require_ktype(ctx.args, ret_slot, ctx.types));
    crate::try_action!(require_proper_type(
        ret,
        "the return type of an `:(FN …)` type",
        ctx.types
    ));
    let mut elaborator = Elaborator::new(ctx.scope);
    match parse_typed_field_list_via_elaborator(
        &sig_expr,
        FN_PARAMS_CONTEXT,
        FN_PARAM_NAME_KIND,
        &mut elaborator,
        None,
        ctx.types,
    ) {
        FieldListOutcome::Done(fields) => {
            let carrier = finalize_carrier(fields, ret, ctx.types);
            Action::Done(Ok(ctx.ctx.type_carried(carrier)))
        }
        FieldListOutcome::Err(msg) => Action::Done(Err(KError::new(KErrorKind::ShapeError(msg)))),
        FieldListOutcome::Pending {
            park_producers,
            sub_dispatches,
        } => FieldListDeferral::new(
            sig_expr,
            park_producers,
            sub_dispatches,
            FN_PARAMS_CONTEXT,
            FN_PARAM_NAME_KIND,
        )
        .action_composed(ret_compose(ret)),
    }
}

pub fn register<'a>(scope: &'a Scope<'a>, types: &TypeRegistry) {
    use crate::builtins::register_builtin;
    register_builtin(
        scope,
        "LIST",
        sig(
            KType::of_kind(KKind::AnyType),
            vec![
                kw("LIST"),
                kw("OF"),
                arg("elem", KType::of_kind(KKind::AnyType)),
            ],
        ),
        action_bodies::body_list_of,
        types,
    );
    register_builtin(
        scope,
        "MAP",
        sig(
            KType::of_kind(KKind::AnyType),
            vec![
                kw("MAP"),
                arg("k", KType::of_kind(KKind::AnyType)),
                kw("->"),
                arg("v", KType::of_kind(KKind::AnyType)),
            ],
        ),
        action_bodies::body_map,
        types,
    );
    register_builtin(
        scope,
        "AS",
        sig(
            KType::of_kind(KKind::AnyType),
            vec![
                arg("applied", KType::of_kind(KKind::AnyType)),
                kw("AS"),
                arg("ctor", KType::of_kind(KKind::AnyType)),
            ],
        ),
        action_bodies::body_apply_as,
        types,
    );
    register_builtin(
        scope,
        "FN",
        sig(
            KType::of_kind(KKind::AnyType),
            vec![
                kw("FN"),
                arg("sig", KType::KEXPRESSION),
                kw("->"),
                // `OfKind(AnyType)` admits every type value — a `-> Ordered` signature return
                // and `-> Module` (which lowers to the empty signature) included.
                arg("ret", KType::of_kind(KKind::AnyType)),
            ],
        ),
        action_bodies::body_fn,
        types,
    );
}

#[cfg(test)]
mod tests {
    use crate::builtins::test_support::{parse_one, TestRun};
    use crate::machine::model::{KKind, KType, Record, TypeNode};
    use crate::machine::run_root_storage;
    use crate::machine::KErrorKind;

    #[test]
    fn list_of_number_lowers_to_list_number() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        let result = test_run.run_one_type(parse_one(":(LIST OF Number)"));
        let types = test_run.types();
        assert_eq!(result, types.list(KType::NUMBER));
    }

    // A root-scope-bound `Wrap` TypeConstructor applied with `:(Number AS Wrap)`
    // lowers to `ConstructorApply(Wrap, {Type = Number})` — `AS` fills the sole parameter.
    #[test]
    fn apply_as_lowers_to_constructor_apply() {
        use crate::machine::model::{declarator_window, RelativeSchema};
        use crate::machine::BindingIndex;
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        let scope = test_run.scope;
        // Seal a singleton `Wrap` constructor member through the real declaration window, then
        // bind its absolute handle as a builtin type.
        let window = declarator_window(scope, "Wrap", KKind::TypeConstructor);
        let sealed = window
            .fill_member(
                0,
                RelativeSchema::TypeConstructor {
                    schema: std::collections::HashMap::new(),
                    param_names: vec!["Type".into()],
                },
                test_run.types(),
            )
            .expect("a singleton window seals on its sole fill");
        scope.register_builtin_type("Wrap".into(), sealed.members[0], BindingIndex::BUILTIN);
        let result = test_run.run_one_type(parse_one(":(Number AS Wrap)"));
        let types = test_run.types();
        match types.node(result) {
            TypeNode::ConstructorApply {
                constructor,
                arguments,
            } => {
                match types.node(constructor) {
                    TypeNode::SetMember { kind, .. } => {
                        assert_eq!(kind, KKind::TypeConstructor);
                    }
                    _ => panic!("expected SetMember ctor, got {constructor:?}"),
                }
                assert_eq!(
                    arguments,
                    Record::from_pairs([("Type".to_string(), KType::NUMBER)]),
                );
            }
            _ => panic!("expected ConstructorApply, got {result:?}"),
        }
    }

    #[test]
    fn map_str_number_lowers_to_dict() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        let result = test_run.run_one_type(parse_one(":(MAP Str -> Number)"));
        let types = test_run.types();
        assert_eq!(result, types.dict(KType::STR, KType::NUMBER));
    }

    #[test]
    fn fn_lowers_to_kfunction() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        let result = test_run.run_one_type(parse_one(":(FN (x :Number, y :Str) -> Bool)"));
        let types = test_run.types();
        assert_eq!(
            result,
            types.function_type(
                Record::from_pairs(vec![("x".into(), KType::NUMBER), ("y".into(), KType::STR)]),
                KType::BOOL,
            )
        );
    }

    #[test]
    fn fn_nullary_lowers_to_kfunction() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        let result = test_run.run_one_type(parse_one(":(FN () -> Number)"));
        let types = test_run.types();
        assert_eq!(result, types.function_type(Record::new(), KType::NUMBER));
    }

    /// A functor — a module-returning function — types as an ordinary `KFunction`.
    // Param name `Ty` uses two letters because koan rejects single-uppercase-letter tokens.
    #[test]
    fn fn_with_type_param_and_module_return_lowers_to_kfunction() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        let result = test_run.run_one_type(parse_one(":(FN (Ty :Signature) -> Module)"));
        let types = test_run.types();
        assert_eq!(
            result,
            types.function_type(
                Record::from_pairs(vec![("Ty".into(), KType::of_kind(KKind::Signature))]),
                KType::EMPTY_SIGNATURE,
            )
        );
    }

    /// A nested parameterized param type (`:(LIST OF Number)`) sub-Dispatches through the
    /// shared field-list parser and lands in the parameter record.
    #[test]
    fn fn_with_nested_list_param_lowers_to_kfunction() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        let result = test_run.run_one_type(parse_one(":(FN (xs :(LIST OF Number)) -> Bool)"));
        let types = test_run.types();
        assert_eq!(
            result,
            types.function_type(
                Record::from_pairs(vec![("xs".into(), types.list(KType::NUMBER))]),
                KType::BOOL,
            )
        );
    }

    /// A `:{…}` record type that mixes a scope-alias field (`:Wrapped`, resolved from the crossed
    /// scope during the deferred re-walk) with a sigil field (`:(LIST OF Number)`, which forces
    /// deferral) composes its `Record` handle at the fold brand with both field types resolved. The
    /// scope-alias field reads through the brand-delivered scope envelope; the sigil field pops its
    /// sub-Dispatch carrier from the fed views.
    #[test]
    fn record_sigil_defers_and_mixes_scope_read_with_sub_dispatch() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        test_run.run("LET Wrapped = :{a :Number}");
        let result = test_run.run_one_type(parse_one(":{x :Wrapped, y :(LIST OF Number)}"));
        let types = test_run.types();
        let inner = types.record(Record::from_pairs(vec![("a".into(), KType::NUMBER)]));
        assert_eq!(
            result,
            types.record(Record::from_pairs(vec![
                ("x".into(), inner),
                ("y".into(), types.list(KType::NUMBER)),
            ])),
        );
    }

    /// A deferred FN (the `:(LIST OF Number)` param forces deferral) whose return type names a
    /// `NEWTYPE` alias — a `SetMember` that is not region-free, so it cannot be rebuilt from a
    /// `'static` value — composes its function type by cloning the return type out of its own carrier
    /// view (the `Some(Carried::Type(_))` compose arm). Were the return type to arrive without a
    /// carrier, `build_carrier`'s guard would error instead of producing a function type, so a
    /// successful compose proves the carrier-view path ran.
    #[test]
    fn fn_deferred_with_reaching_ret_composes_from_carrier_view() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        test_run.run("NEWTYPE Wrapped = :{a :Number}");
        let result = test_run.run_one_type(parse_one(":(FN (xs :(LIST OF Number)) -> Wrapped)"));
        let types = test_run.types();
        match types.node(result) {
            TypeNode::KFunction { params, ret } => {
                assert_eq!(
                    params.get("xs").copied(),
                    Some(types.list(KType::NUMBER)),
                    "the sigil param must lower to LIST OF Number",
                );
                assert_eq!(
                    ret.name(types),
                    "Wrapped",
                    "the reaching return type must survive the carrier-view crossing",
                );
            }
            _ => panic!("expected a KFunction carrier, got {result:?}"),
        }
    }

    /// A deferred record field whose sigil sub-Dispatch resolves to a non-type value (`:(1)` → the
    /// number `1`) surfaces the walker's shape error directly: `compose_field_list` propagates the
    /// rewalk's `Err` before any allocation runs.
    #[test]
    fn record_field_sub_dispatch_to_non_type_value_errors() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        let err = test_run.run_one_err(parse_one(":{x :(1)}"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("resolved to non-type value")),
            "expected a non-type-value ShapeError through the deferred side-channel, got {err}",
        );
    }

    /// `t.name()` round-trips: rendering `expected` and re-running its surface form yields
    /// a type carrier equal to `expected`. The expected value is built at each call site so
    /// it shares the scope's lifetime, keeping the comparison off `'static`.
    fn assert_round_trips(test_run: &mut TestRun<'_>, expected: KType) {
        let rendered = expected.name(test_run.types());
        let result = test_run.run_one_type(parse_one(&rendered));
        assert_eq!(
            result, expected,
            "round-trip of `{rendered}` did not reproduce the original KType",
        );
    }

    #[test]
    fn fn_multi_param_round_trips() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        let expected = test_run.types().function_type(
            Record::from_pairs(vec![("x".into(), KType::NUMBER), ("y".into(), KType::STR)]),
            KType::BOOL,
        );
        assert_round_trips(&mut test_run, expected);
    }

    #[test]
    fn fn_nullary_round_trips() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        let expected = test_run.types().function_type(Record::new(), KType::ANY);
        assert_round_trips(&mut test_run, expected);
    }

    #[test]
    fn fn_nested_param_round_trips() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        let types = test_run.types();
        let expected = types.function_type(
            Record::from_pairs(vec![("xs".into(), types.list(KType::NUMBER))]),
            KType::BOOL,
        );
        assert_round_trips(&mut test_run, expected);
    }

    #[test]
    fn fn_capitalized_param_round_trips_and_preserves_name() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        let types = test_run.types();
        let expected = types.function_type(
            Record::from_pairs(vec![("Ty".into(), KType::of_kind(KKind::Signature))]),
            KType::EMPTY_SIGNATURE,
        );
        // Param name `Ty` (capitalized, a `Type` token) must survive the round-trip.
        assert!(
            matches!(types.node(expected), TypeNode::KFunction { params, .. } if params.get("Ty").is_some()),
        );
        assert_round_trips(&mut test_run, expected);
    }

    /// `:(MAP Str -> Wrapped)` correlates a scalar-literal key with no carrier (`k`) and a
    /// reaching value type (`v`) by total operand position, not by carrier presence: the scalar
    /// lands in `k` and the reaching type survives the carrier-view crossing into `v`.
    #[test]
    fn map_scalar_key_reaching_value_correlates() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        test_run.run("NEWTYPE Wrapped = :{a :Number}");
        let result = test_run.run_one_type(parse_one(":(MAP Str -> Wrapped)"));
        let types = test_run.types();
        match types.node(result) {
            TypeNode::Dict { key, value } => {
                assert_eq!(key, KType::STR, "scalar key must lower to Str");
                assert_eq!(
                    value.name(types),
                    "Wrapped",
                    "reaching value type must survive the carrier-view crossing",
                );
            }
            _ => panic!("expected a Dict carrier, got {result:?}"),
        }
    }

    /// Mirror of `map_scalar_key_reaching_value_correlates`: the reaching type lands in `k` and
    /// the scalar lands in `v`, proving the correlation is positional, not carrier-presence-based.
    #[test]
    fn map_reaching_key_scalar_value_correlates() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        test_run.run("NEWTYPE Wrapped = :{a :Number}");
        let result = test_run.run_one_type(parse_one(":(MAP Wrapped -> Str)"));
        let types = test_run.types();
        match types.node(result) {
            TypeNode::Dict { key, value } => {
                assert_eq!(
                    key.name(types),
                    "Wrapped",
                    "reaching key type must survive the carrier-view crossing",
                );
                assert_eq!(value, KType::STR, "scalar value must lower to Str");
            }
            _ => panic!("expected a Dict carrier, got {result:?}"),
        }
    }

    /// A sync record type whose field names a `NEWTYPE` alias (`:Wrapped`, a `SetMember`) resolves
    /// in one ambient walk — no sigil field forces deferral — so its `Record` handle composes
    /// directly from the elaborated pairs, where the `SetMember` field survives as owned data.
    #[test]
    fn record_sync_reaching_field_folds_at_brand() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        test_run.run("NEWTYPE Wrapped = :{a :Number}");
        let result = test_run.run_one_type(parse_one(":{x :Wrapped}"));
        let types = test_run.types();
        match types.node(result) {
            TypeNode::Record { fields: record } => {
                let field = record.get("x").expect("record must have field x");
                assert_eq!(
                    field.name(types),
                    "Wrapped",
                    "the reaching field must survive the sync brand re-fold",
                );
            }
            _ => panic!("expected a Record, got {result:?}"),
        }
    }

    /// A sync FN whose parameter type names a `NEWTYPE` alias (`x :Wrapped`, a `SetMember`) resolves
    /// in one ambient walk, so its function type composes directly from the elaborated pairs:
    /// the `SetMember` param survives as owned data alongside the plain `Bool` return type.
    #[test]
    fn fn_sync_reaching_param_folds_at_brand() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        test_run.run("NEWTYPE Wrapped = :{a :Number}");
        let result = test_run.run_one_type(parse_one(":(FN (x :Wrapped) -> Bool)"));
        let types = test_run.types();
        match types.node(result) {
            TypeNode::KFunction { params, ret } => {
                assert_eq!(
                    params.get("x").map(|kt| kt.name(types)),
                    Some("Wrapped".to_string()),
                    "the SetMember param must survive the sync compose",
                );
                assert_eq!(ret, KType::BOOL, "the region-free return type must be Bool");
            }
            _ => panic!("expected a KFunction, got {result:?}"),
        }
    }

    /// A sync FN whose return type names a `NEWTYPE` alias (`-> Wrapped`) resolves in one ambient
    /// walk, so its `KType::KFunction` composes directly from the elaborated pairs: the `ret`
    /// argument the caller closed over crosses into the composed carrier as owned data.
    #[test]
    fn fn_sync_reaching_ret_folds_at_brand() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        test_run.run("NEWTYPE Wrapped = :{a :Number}");
        let result = test_run.run_one_type(parse_one(":(FN (x :Number) -> Wrapped)"));
        let types = test_run.types();
        match types.node(result) {
            TypeNode::KFunction { params, ret } => {
                assert_eq!(
                    params.get("x").copied(),
                    Some(KType::NUMBER),
                    "the region-free param must be Number",
                );
                assert_eq!(
                    ret.name(types),
                    "Wrapped",
                    "the SetMember return type must survive the carrier-view crossing",
                );
            }
            _ => panic!("expected a KFunction, got {result:?}"),
        }
    }

    /// `:(LIST OF Wrapped)` lowers with the reaching elem type surviving the carrier-view
    /// crossing (the single-operand analog of the MAP correlation tests above).
    #[test]
    fn list_of_reaching_elem_lowers() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        test_run.run("NEWTYPE Wrapped = :{a :Number}");
        let result = test_run.run_one_type(parse_one(":(LIST OF Wrapped)"));
        let types = test_run.types();
        match types.node(result) {
            TypeNode::List { element } => {
                assert_eq!(
                    element.name(types),
                    "Wrapped",
                    "reaching elem type must survive the carrier-view crossing",
                );
            }
            _ => panic!("expected a List carrier, got {result:?}"),
        }
    }
}
