//! `ArgumentBundle` — the resolved name-to-value map produced by `KFunction::bind` and
//! consumed by a builtin or user-defined body.
//!
//! Also home to the slot-extraction helpers (`extract_kexpression`, `extract_ktype`,
//! `extract_type_name_ref`, `extract_bare_type_name`) that collapse the
//! `Rc::try_unwrap` + variant-match dance used to pull `KExpression`, an elaborated
//! `KType`, a `TypeNameRef` carrier's `TypeExpr`, or a surface type name out of a
//! bundle slot.

use std::collections::HashMap;
use std::rc::Rc;

use crate::machine::model::ast::{KExpression, TypeExpr, TypeParams};

use crate::machine::core::{KError, KErrorKind};
use crate::machine::model::types::KType;
use crate::machine::model::values::{KObject, Module, Signature};

/// Name to resolved value, produced by `KFunction::bind` and consumed by the body.
pub struct ArgumentBundle<'a> {
    pub args: HashMap<String, Rc<KObject<'a>>>,
}

impl<'a> ArgumentBundle<'a> {
    pub fn get(&self, name: &str) -> Option<&KObject<'a>> {
        self.args.get(name).map(|v| v.as_ref())
    }

    /// Fully independent copy: each value is `deep_clone`d into a fresh `Rc`. Sharing in
    /// the original bundle's `Rc`s is not preserved.
    pub fn deep_clone(&self) -> ArgumentBundle<'a> {
        ArgumentBundle {
            args: self
                .args
                .iter()
                .map(|(k, v)| (k.clone(), Rc::new(v.deep_clone())))
                .collect(),
        }
    }

    /// Borrow `name`'s slot as a `&KExpression`. `MissingArg` if absent;
    /// `TypeMismatch { expected: "KExpression" }` if the slot holds a non-`KExpression`
    /// variant.
    pub fn require_kexpression(&self, name: &str) -> Result<&KExpression<'a>, KError> {
        let obj = self.get_or_missing(name)?;
        obj.as_kexpression().ok_or_else(|| mismatch(name, "KExpression", obj))
    }

    /// Borrow `name`'s slot as a `&KType`. `MissingArg` / `TypeMismatch` shaped the same
    /// way as [`require_kexpression`](Self::require_kexpression).
    pub fn require_ktype(&self, name: &str) -> Result<&KType, KError> {
        let obj = self.get_or_missing(name)?;
        obj.as_ktype().ok_or_else(|| mismatch(name, "TypeExprRef", obj))
    }

    /// Borrow `name`'s slot as a `&Module`. Same error shape as the sister `require_*`
    /// methods.
    pub fn require_module(&self, name: &str) -> Result<&'a Module<'a>, KError> {
        let obj = self.get_or_missing(name)?;
        obj.as_module().ok_or_else(|| mismatch(name, "Module", obj))
    }

    /// Borrow `name`'s slot as a `&Signature`. Same error shape as the sister `require_*`
    /// methods.
    pub fn require_signature(&self, name: &str) -> Result<&'a Signature<'a>, KError> {
        let obj = self.get_or_missing(name)?;
        obj.as_signature().ok_or_else(|| mismatch(name, "Signature", obj))
    }

    /// Borrow `name`'s slot as any `&KObject`. `MissingArg` if absent; no variant
    /// narrowing — the caller dispatches on `KObject` arms itself. Use the variant-typed
    /// `require_*` siblings when only one shape is acceptable.
    pub fn require(&self, name: &str) -> Result<&KObject<'a>, KError> {
        self.get_or_missing(name)
    }

    fn get_or_missing(&self, name: &str) -> Result<&KObject<'a>, KError> {
        self.get(name)
            .ok_or_else(|| KError::new(KErrorKind::MissingArg(name.to_string())))
    }
}

fn mismatch(arg: &str, expected: &str, got: &KObject<'_>) -> KError {
    KError::new(KErrorKind::TypeMismatch {
        arg: arg.to_string(),
        expected: expected.to_string(),
        got: got.ktype().name(),
    })
}

/// Take ownership of a `KType::KExpression`-typed argument out of `bundle.args`, cloning
/// only if the bundle is not the sole `Rc` holder. Returns `None` if the slot is missing
/// or holds a non-`KExpression` variant.
pub(crate) fn extract_kexpression<'a>(
    bundle: &mut ArgumentBundle<'a>,
    name: &str,
) -> Option<KExpression<'a>> {
    let rc = bundle.args.remove(name)?;
    match Rc::try_unwrap(rc) {
        Ok(KObject::KExpression(e)) => Some(e),
        Ok(_) => None,
        Err(rc) => match &*rc {
            KObject::KExpression(e) => Some(e.clone()),
            _ => None,
        },
    }
}

/// Take ownership of the elaborated `KType` carried by a `KObject::KTypeValue`-variant
/// `KType::TypeExprRef` slot. Returns `None` for the sibling `KObject::TypeNameRef`
/// carrier (callers route to [`extract_type_name_ref`] for that path) and for missing
/// slots. Clones the inner `KType` if the bundle is not the sole `Rc` holder.
///
/// Both extractors consume the slot via `remove`; a caller that wants to try both must
/// peek with `bundle.get(...)` first to pick the right one.
pub(crate) fn extract_ktype<'a>(
    bundle: &mut ArgumentBundle<'a>,
    name: &str,
) -> Option<KType> {
    let rc = bundle.args.remove(name)?;
    match Rc::try_unwrap(rc) {
        Ok(KObject::KTypeValue(t)) => Some(t),
        Ok(_) => None,
        Err(rc) => match &*rc {
            KObject::KTypeValue(t) => Some(t.clone()),
            _ => None,
        },
    }
}

/// Resolve a `KType::TypeExprRef` slot to its bare type name. Two carrier variants share
/// the slot post-stage-2:
///
/// - `KObject::KTypeValue(t)` — the parser-side `TypeExpr` resolved to a builtin `KType`
///   at `resolve_for` time. Leaf-named variants surface their `KType::name()`;
///   structural / recursive shapes (`List<X>`, function types, `Mu` / `RecursiveRef`)
///   are not valid binder / constructor / type-call names and surface a `ShapeError`.
/// - `KObject::TypeNameRef(t, _)` — a `resolve_for`-time fallback for bare-leaf names
///   not in `KType::from_name`'s builtin table. The surface name is `t.name` directly;
///   parameterized shapes on the carrier's `TypeExpr` are rejected with the same
///   `ShapeError` text shape as the parameterized-`KType` rejection.
///
/// `surface` is the surface-form keyword (`"STRUCT"`, `"UNION"`, …) embedded in the
/// message.
pub(crate) fn extract_bare_type_name<'a>(
    bundle: &ArgumentBundle<'a>,
    name: &str,
    surface: &str,
) -> Result<String, KError> {
    match bundle.get(name) {
        Some(KObject::TypeNameRef(t)) => match &t.params {
            TypeParams::None => Ok(t.name.clone()),
            // Parameterized surface form on a `TypeNameRef` carrier — the parser saw
            // something like `Foo<Bar>` where `Foo` isn't a builtin and the user wrote
            // it in a binder / constructor slot. Reject with the same message shape as
            // the `KTypeValue` parameterized rejection.
            TypeParams::List(_) | TypeParams::Function { .. } => {
                Err(KError::new(KErrorKind::ShapeError(format!(
                    "{surface} {name} must be a bare type name, got `{}`",
                    t.render(),
                ))))
            }
        },
        Some(KObject::KTypeValue(t)) => match t {
            // Leaf-named variants: surface name is the user-facing identifier. Both
            // `UserType` (per-declaration tag) and `AnyUserType` (wildcard kind tag)
            // join the leaf set — their `name()` renders either the declared name or
            // the surface keyword (`Struct`/`Tagged`/`Module`), both valid binder /
            // constructor / type-call names.
            KType::Number
            | KType::Str
            | KType::Bool
            | KType::Null
            | KType::Identifier
            | KType::KExpression
            | KType::TypeExprRef
            | KType::Type
            | KType::Signature
            | KType::Any
            | KType::UserType { .. }
            | KType::AnyUserType { .. }
            | KType::SignatureBound { .. } => Ok(t.name()),
            // Structural / recursive shapes are not valid binder names — the caller wants
            // a leaf identifier, not a parameterized container. `ConstructorApply` joins
            // this group: an applied higher-kinded type is structural, not a leaf.
            KType::List(_)
            | KType::Dict(_, _)
            | KType::KFunction { .. }
            | KType::Mu { .. }
            | KType::RecursiveRef(_)
            | KType::ConstructorApply { .. } => Err(KError::new(KErrorKind::ShapeError(format!(
                "{surface} {name} must be a bare type name, got `{}`",
                t.render(),
            )))),
        },
        Some(other) => Err(KError::new(KErrorKind::TypeMismatch {
            arg: name.to_string(),
            expected: "TypeExprRef".to_string(),
            got: other.ktype().name(),
        })),
        None => Err(KError::new(KErrorKind::MissingArg(name.to_string()))),
    }
}

/// Take ownership of a `TypeNameRef` carrier's `TypeExpr` out of `bundle.args`, cloning
/// if the bundle is not the sole `Rc` holder. Returns `None` when the slot is missing or
/// holds a non-`TypeNameRef` variant (the caller typically tried `extract_ktype` first
/// and falls through here for the unresolved-leaf carrier path).
///
/// FN's return-type elaboration consumes the helper to recover the bare-leaf name into
/// its existing `ReturnTypeState::Pending(name, …)` / `ReturnTypeCapture::Unresolved`
/// machinery; the parser-preserved `TypeExpr` is the source of truth for the surface
/// form that survives bind for diagnostics.
pub(crate) fn extract_type_name_ref<'a>(
    bundle: &mut ArgumentBundle<'a>,
    name: &str,
) -> Option<TypeExpr> {
    let rc = bundle.args.remove(name)?;
    match Rc::try_unwrap(rc) {
        Ok(KObject::TypeNameRef(t)) => Some(t),
        Ok(_) => None,
        Err(rc) => match &*rc {
            KObject::TypeNameRef(t) => Some(t.clone()),
            _ => None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::model::ast::{ExpressionPart, KExpression};

    fn one_slot_bundle<'a>(name: &str, obj: KObject<'a>) -> ArgumentBundle<'a> {
        let mut args = HashMap::new();
        args.insert(name.to_string(), Rc::new(obj));
        ArgumentBundle { args }
    }

    fn type_name_ref<'a>(name: &str, params: TypeParams) -> KObject<'a> {
        KObject::TypeNameRef(TypeExpr {
            name: name.into(),
            params,
            builtin_cache: std::cell::OnceCell::new(),
        })
    }

    // ---------- shared-Rc clone paths on the extract_* helpers ----------

    /// `extract_kexpression`'s `Err(rc) => KExpression` arm: when the bundle's slot is
    /// shared with an outside holder, `Rc::try_unwrap` fails and the helper falls back
    /// to cloning the inner `KExpression`.
    #[test]
    fn extract_kexpression_clones_when_rc_is_shared() {
        let expr = KExpression {
            parts: vec![ExpressionPart::Identifier("k".into())],
        };
        let shared = Rc::new(KObject::KExpression(expr));
        let _outside = Rc::clone(&shared);
        let mut bundle = ArgumentBundle { args: HashMap::new() };
        bundle.args.insert("e".into(), shared);
        let got = extract_kexpression(&mut bundle, "e").expect("clone path should return Some");
        assert!(matches!(got.parts.as_slice(), [ExpressionPart::Identifier(n)] if n == "k"));
    }

    /// `extract_kexpression`'s `Err(rc) => _` arm: shared `Rc` holding a non-`KExpression`
    /// variant yields `None`.
    #[test]
    fn extract_kexpression_shared_non_matching_variant_returns_none() {
        let shared = Rc::new(KObject::Number(1.0));
        let _outside = Rc::clone(&shared);
        let mut bundle = ArgumentBundle { args: HashMap::new() };
        bundle.args.insert("e".into(), shared);
        assert!(extract_kexpression(&mut bundle, "e").is_none());
    }

    /// `extract_ktype`'s `Err(rc) => KTypeValue` arm: shared `Rc` clones the inner
    /// `KType`.
    #[test]
    fn extract_ktype_clones_when_rc_is_shared() {
        let shared = Rc::new(KObject::KTypeValue(KType::Number));
        let _outside = Rc::clone(&shared);
        let mut bundle = ArgumentBundle { args: HashMap::new() };
        bundle.args.insert("t".into(), shared);
        assert_eq!(extract_ktype(&mut bundle, "t"), Some(KType::Number));
    }

    /// `extract_ktype`'s `Err(rc) => _` arm.
    #[test]
    fn extract_ktype_shared_non_matching_variant_returns_none() {
        let shared = Rc::new(KObject::Number(2.0));
        let _outside = Rc::clone(&shared);
        let mut bundle = ArgumentBundle { args: HashMap::new() };
        bundle.args.insert("t".into(), shared);
        assert!(extract_ktype(&mut bundle, "t").is_none());
    }

    /// `extract_type_name_ref`'s `Err(rc) => TypeNameRef` arm clones the carried
    /// `TypeExpr` when the slot's `Rc` is shared.
    #[test]
    fn extract_type_name_ref_clones_when_rc_is_shared() {
        let shared = Rc::new(type_name_ref("Foo", TypeParams::None));
        let _outside = Rc::clone(&shared);
        let mut bundle = ArgumentBundle { args: HashMap::new() };
        bundle.args.insert("t".into(), shared);
        let got = extract_type_name_ref(&mut bundle, "t").expect("clone path should return Some");
        assert_eq!(got.name, "Foo");
    }

    /// `extract_type_name_ref`'s `Err(rc) => _` arm.
    #[test]
    fn extract_type_name_ref_shared_non_matching_variant_returns_none() {
        let shared = Rc::new(KObject::KTypeValue(KType::Number));
        let _outside = Rc::clone(&shared);
        let mut bundle = ArgumentBundle { args: HashMap::new() };
        bundle.args.insert("t".into(), shared);
        assert!(extract_type_name_ref(&mut bundle, "t").is_none());
    }

    // ---------- extract_bare_type_name arms ----------

    /// `TypeNameRef` carrier with `TypeParams::List(_)` → `ShapeError` with the
    /// rendered surface form.
    #[test]
    fn extract_bare_type_name_rejects_parameterized_type_name_ref_list() {
        let bundle = one_slot_bundle(
            "T",
            type_name_ref("Foo", TypeParams::List(vec![TypeExpr::leaf("Bar".into())])),
        );
        let err = extract_bare_type_name(&bundle, "T", "STRUCT").expect_err("should reject");
        match err.kind {
            KErrorKind::ShapeError(msg) => {
                assert!(msg.contains("STRUCT T must be a bare type name"));
                assert!(msg.contains(":(Foo Bar)"));
            }
            other => panic!("expected ShapeError, got {:?}", std::mem::discriminant(&other)),
        }
    }

    /// `TypeNameRef` carrier with `TypeParams::Function { .. }` → `ShapeError`.
    #[test]
    fn extract_bare_type_name_rejects_parameterized_type_name_ref_function() {
        let bundle = one_slot_bundle(
            "T",
            type_name_ref(
                "Foo",
                TypeParams::Function {
                    args: vec![TypeExpr::leaf("A".into())],
                    ret: Box::new(TypeExpr::leaf("R".into())),
                },
            ),
        );
        let err = extract_bare_type_name(&bundle, "T", "UNION").expect_err("should reject");
        match err.kind {
            KErrorKind::ShapeError(msg) => {
                assert!(msg.contains("UNION T must be a bare type name"));
                assert!(msg.contains("Foo"));
            }
            other => panic!("expected ShapeError, got {:?}", std::mem::discriminant(&other)),
        }
    }

    /// `KTypeValue` leaf-variant arm: surface name is the `KType::name()` rendering.
    /// Picks `KType::Number` as a representative leaf — the arm shares one body across
    /// every leaf variant in the match.
    #[test]
    fn extract_bare_type_name_accepts_ktypevalue_leaf() {
        let bundle = one_slot_bundle("T", KObject::KTypeValue(KType::Number));
        let name = extract_bare_type_name(&bundle, "T", "STRUCT").expect("leaf should be accepted");
        assert_eq!(name, "Number");
    }

    /// `KTypeValue` structural arm: `List<Number>` is parameterized and rejected with
    /// the rendered `:(List Number)` surface form embedded in the message.
    #[test]
    fn extract_bare_type_name_rejects_ktypevalue_structural() {
        let list = KType::List(Box::new(KType::Number));
        let bundle = one_slot_bundle("T", KObject::KTypeValue(list));
        let err = extract_bare_type_name(&bundle, "T", "STRUCT").expect_err("should reject");
        match err.kind {
            KErrorKind::ShapeError(msg) => {
                assert!(msg.contains("STRUCT T must be a bare type name"));
                assert!(msg.contains(":(List Number)"));
            }
            other => panic!("expected ShapeError, got {:?}", std::mem::discriminant(&other)),
        }
    }

    /// `Some(other)` arm: a slot holding a value-typed `KObject` (not a `TypeNameRef`
    /// or `KTypeValue` carrier) returns `TypeMismatch { expected: "TypeExprRef" }`.
    #[test]
    fn extract_bare_type_name_rejects_non_type_carrier() {
        let bundle = one_slot_bundle("T", KObject::Number(1.0));
        let err = extract_bare_type_name(&bundle, "T", "STRUCT").expect_err("should reject");
        match err.kind {
            KErrorKind::TypeMismatch { arg, expected, got } => {
                assert_eq!(arg, "T");
                assert_eq!(expected, "TypeExprRef");
                assert_eq!(got, "Number");
            }
            other => panic!("expected TypeMismatch, got {:?}", std::mem::discriminant(&other)),
        }
    }

    /// `None` arm: missing slot returns `MissingArg`.
    #[test]
    fn extract_bare_type_name_missing_slot_returns_missing_arg() {
        let bundle = ArgumentBundle { args: HashMap::new() };
        let err = extract_bare_type_name(&bundle, "T", "STRUCT").expect_err("should reject");
        match err.kind {
            KErrorKind::MissingArg(name) => assert_eq!(name, "T"),
            other => panic!("expected MissingArg, got {:?}", std::mem::discriminant(&other)),
        }
    }

    // ---------- require_* mismatch + missing closures ----------

    fn unwrap_err<T>(r: Result<T, KError>) -> KError {
        match r {
            Ok(_) => panic!("expected Err"),
            Err(e) => e,
        }
    }

    /// `require_kexpression` mismatch arm: a non-`KExpression` slot routes through the
    /// shared `mismatch` helper.
    #[test]
    fn require_kexpression_mismatch_routes_through_mismatch_helper() {
        let bundle = one_slot_bundle("e", KObject::Number(1.0));
        let err = unwrap_err(bundle.require_kexpression("e"));
        match err.kind {
            KErrorKind::TypeMismatch { arg, expected, got } => {
                assert_eq!(arg, "e");
                assert_eq!(expected, "KExpression");
                assert_eq!(got, "Number");
            }
            other => panic!("expected TypeMismatch, got {:?}", std::mem::discriminant(&other)),
        }
    }

    /// `require_ktype` mismatch arm.
    #[test]
    fn require_ktype_mismatch_routes_through_mismatch_helper() {
        let bundle = one_slot_bundle("t", KObject::Number(1.0));
        let err = unwrap_err(bundle.require_ktype("t"));
        assert!(matches!(err.kind, KErrorKind::TypeMismatch { .. }));
    }

    /// `require_module` mismatch arm.
    #[test]
    fn require_module_mismatch_routes_through_mismatch_helper() {
        let bundle = one_slot_bundle("m", KObject::Number(1.0));
        let err = unwrap_err(bundle.require_module("m"));
        assert!(matches!(err.kind, KErrorKind::TypeMismatch { .. }));
    }

    /// `require_signature` mismatch arm.
    #[test]
    fn require_signature_mismatch_routes_through_mismatch_helper() {
        let bundle = one_slot_bundle("s", KObject::Number(1.0));
        let err = unwrap_err(bundle.require_signature("s"));
        assert!(matches!(err.kind, KErrorKind::TypeMismatch { .. }));
    }

    /// `require` (the no-narrow variant) routes a missing slot through `get_or_missing`'s
    /// `MissingArg` closure — exercises the second arm of `ok_or_else`.
    #[test]
    fn require_missing_slot_returns_missing_arg() {
        let bundle = ArgumentBundle { args: HashMap::new() };
        let err = unwrap_err(bundle.require("x"));
        match err.kind {
            KErrorKind::MissingArg(name) => assert_eq!(name, "x"),
            other => panic!("expected MissingArg, got {:?}", std::mem::discriminant(&other)),
        }
    }

    // ---------- unique-Rc Ok(_) => None arms on the extract_* helpers ----------

    /// `extract_kexpression`'s `Ok(_) => None` arm: the bundle owns the only `Rc`
    /// (`try_unwrap` succeeds) but the inner variant isn't `KExpression`, so the helper
    /// returns `None`. Distinct from the shared-`Rc` mismatch arm covered above.
    #[test]
    fn extract_kexpression_unique_non_matching_variant_returns_none() {
        let mut bundle = one_slot_bundle("e", KObject::Number(1.0));
        assert!(extract_kexpression(&mut bundle, "e").is_none());
    }

    /// `extract_ktype`'s `Ok(_) => None` arm.
    #[test]
    fn extract_ktype_unique_non_matching_variant_returns_none() {
        let mut bundle = one_slot_bundle("t", KObject::Number(1.0));
        assert!(extract_ktype(&mut bundle, "t").is_none());
    }

    /// `extract_type_name_ref`'s `Ok(_) => None` arm.
    #[test]
    fn extract_type_name_ref_unique_non_matching_variant_returns_none() {
        let mut bundle = one_slot_bundle("t", KObject::KTypeValue(KType::Number));
        assert!(extract_type_name_ref(&mut bundle, "t").is_none());
    }
}
