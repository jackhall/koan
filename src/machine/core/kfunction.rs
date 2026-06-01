//! `KFunction` — the callable Koan function value. Carries an `ExpressionSignature`,
//! a `Body` (builtin `fn` pointer or captured user-defined `KExpression`), and the
//! lexical scope captured at definition time.

use std::collections::HashMap;
use std::marker::PhantomData;
use std::ptr::NonNull;
use std::rc::Rc;

use crate::machine::core::source::Spanned;
use crate::machine::model::ast::{ExpressionPart, KExpression};

use crate::machine::core::{KError, KErrorKind, KFuture, Scope};
use crate::machine::model::types::{ExpressionSignature, Parseable, SignatureElement};
use crate::machine::model::values::{KObject, NamedPairs};

pub mod argument_bundle;
pub mod body;
pub mod invoke;
pub mod pick;
pub mod scheduler_handle;

pub use argument_bundle::ArgumentBundle;
pub use body::{BinderBucketFn, BinderNameFn, Body, BodyResult, BuiltinFn};
pub use pick::ClassifiedSlots;
pub use scheduler_handle::{CatchFinish, CombineFinish, NodeId, SchedulerHandle};

/// SAFETY: the captured scope is allocated in a `RuntimeArena` that outlives this
/// `KFunction` — they share the arena (FN registers the function in the same scope it
/// captures; builtins are registered in run-root). See `core/arena.rs` for the broader
/// lifetime-erasure pattern.
pub struct KFunction<'a> {
    pub signature: ExpressionSignature<'a>,
    pub body: Body<'a>,
    /// **Variance-load-bearing.** `Scope<'a>` is invariant in `'a` (it contains
    /// `RefCell`s), so the paired `PhantomData<&'a Scope<'a>>` is required to keep
    /// `KFunction<'a>` invariant in `'a`. Do **not** simplify `_p` to
    /// `PhantomData<&'a ()>` — that would make `KFunction` covariant in `'a` and
    /// silently reintroduce a soundness bug.
    captured: NonNull<Scope<'a>>,
    _p: PhantomData<&'a Scope<'a>>,
    /// `Some(_)` for binder builtins (LET, FN, STRUCT, UNION, SIG, MODULE).
    pub binder_name: Option<BinderNameFn>,
    /// `Some(_)` for binder builtins whose body registers a callable function (`FN`,
    /// `FUNCTOR`). Returns the *inner-call* bucket key (e.g. `(MAKESET _)`) so the
    /// dispatch driver installs an entry in `bindings.pending_overloads` and a
    /// sibling bare-arg call form like `(MAKESET IntOrd)` parks on the binder slot
    /// instead of surfacing `DispatchFailed` before finalize.
    pub binder_bucket: Option<BinderBucketFn>,
    /// Flipped on by the `FUNCTOR` binder. Distinguishes the same underlying
    /// `KFunction` shape into the two type-language families: `function_value_ktype`
    /// projects `is_functor → KType::KFunctor`, else `KType::KFunction`. See
    /// [design/typing/functors.md](../../../design/typing/functors.md).
    pub is_functor: bool,
    /// Flipped on by binder builtins whose binding installs a *nominal* identity —
    /// `STRUCT`, named `UNION`, `SIG`, `FUNCTOR`, `MODULE`. Carves the entry out of
    /// the strict-lexical-cutoff visibility test so siblings on the same block can
    /// refer to one another regardless of source order (mutual recursion across
    /// nominal binders).
    pub is_nominal_binder: bool,
}

impl<'a> KFunction<'a> {
    pub fn new(
        signature: ExpressionSignature<'a>,
        body: Body<'a>,
        captured: &'a Scope<'a>,
    ) -> Self {
        Self::with_binder_name(signature, body, captured, None)
    }

    pub fn with_binder_name(
        signature: ExpressionSignature<'a>,
        body: Body<'a>,
        captured: &'a Scope<'a>,
        binder_name: Option<BinderNameFn>,
    ) -> Self {
        Self::with_binder_and_functor(signature, body, captured, binder_name, None, false, false)
    }

    pub fn with_binder_and_functor(
        mut signature: ExpressionSignature<'a>,
        body: Body<'a>,
        captured: &'a Scope<'a>,
        binder_name: Option<BinderNameFn>,
        binder_bucket: Option<BinderBucketFn>,
        is_functor: bool,
        is_nominal_binder: bool,
    ) -> Self {
        signature.normalize();
        Self {
            signature,
            body,
            captured: NonNull::from(captured),
            _p: PhantomData,
            binder_name,
            binder_bucket,
            is_functor,
            is_nominal_binder,
        }
    }

    /// SAFETY: `captured` was built from `NonNull::from(&'a Scope<'a>)` in
    /// [`Self::with_binder_and_functor`], so the pointer is non-null and points at a
    /// `Scope<'a>` that outlives this `KFunction<'a>` by the broader runtime-arena
    /// SAFETY argument (see `core/arena.rs::RuntimeArena`).
    pub fn captured_scope(&self) -> &'a Scope<'a> {
        unsafe { self.captured.as_ref() }
    }

    pub fn summarize(&self) -> String {
        let parts: Vec<String> = self
            .signature
            .elements
            .iter()
            .map(|el| match el {
                SignatureElement::Keyword(s) => s.clone(),
                SignatureElement::Argument(arg) => format!("<{}>", arg.name),
            })
            .collect();
        format!("fn({})", parts.join(" "))
    }

    pub fn bind(&'a self, expr: KExpression<'a>) -> Result<KFuture<'a>, KError> {
        if self.signature.elements.len() != expr.parts.len() {
            return Err(KError::new(KErrorKind::ArityMismatch {
                expected: self.signature.elements.len(),
                got: expr.parts.len(),
            }));
        }
        let mut args: HashMap<String, Rc<KObject<'a>>> = HashMap::new();
        for (el, part) in self.signature.elements.iter().zip(expr.parts.iter()) {
            let part_value = &part.value;
            match el {
                SignatureElement::Keyword(s) => match part_value {
                    ExpressionPart::Keyword(t) if s == t => {}
                    ExpressionPart::Keyword(t) => {
                        return Err(KError::new(KErrorKind::DispatchFailed {
                            expr: expr.summarize(),
                            reason: format!("expected keyword '{s}', got '{t}'"),
                        }));
                    }
                    _ => {
                        return Err(KError::new(KErrorKind::DispatchFailed {
                            expr: expr.summarize(),
                            reason: format!("expected keyword '{s}'"),
                        }));
                    }
                },
                SignatureElement::Argument(arg) => {
                    if !arg.matches(part_value) {
                        return Err(KError::new(KErrorKind::TypeMismatch {
                            arg: arg.name.clone(),
                            expected: arg.ktype.name(),
                            got: part_value.summarize(),
                        }));
                    }
                    args.insert(
                        arg.name.clone(),
                        Rc::new(part_value.resolve_for(&arg.ktype)),
                    );
                }
            }
        }
        Ok(KFuture {
            parsed: expr,
            function: self,
            bundle: ArgumentBundle { args },
        })
    }

    /// Validation precedence (first wins): malformed pair shape (`ShapeError` from
    /// `NamedPairs::parse`) → missing arg (`MissingArg`) → unknown arg
    /// (`ShapeError("unknown name ...")`). Arity is implicit — `NamedPairs` rejects
    /// duplicate names at parse time, so consuming every declared argument and
    /// finding the residual empty witnesses an exact match.
    pub fn reconstruct_positional<'b>(
        &self,
        args: Vec<Spanned<ExpressionPart<'b>>>,
    ) -> Result<KExpression<'b>, KError> {
        let tmp_expr = KExpression::new(args);
        let mut pairs = NamedPairs::parse(&tmp_expr, "function call")
            .map_err(|msg| KError::new(KErrorKind::ShapeError(msg)))?;
        let mut parts: Vec<Spanned<ExpressionPart<'b>>> =
            Vec::with_capacity(self.signature.elements.len());
        for el in &self.signature.elements {
            match el {
                SignatureElement::Keyword(s) => {
                    parts.push(Spanned::bare(ExpressionPart::Keyword(s.clone())))
                }
                SignatureElement::Argument(a) => match pairs.take(&a.name) {
                    Some(v) => parts.push(Spanned::bare(v)),
                    None => {
                        return Err(KError::new(KErrorKind::MissingArg(a.name.clone())));
                    }
                },
            }
        }
        if let Some(unknown) = pairs.into_unknown() {
            return Err(KError::new(KErrorKind::ShapeError(format!(
                "unknown name `{unknown}` in function call",
            ))));
        }
        Ok(KExpression::new(parts))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtins::test_support::{marker, run_root_bare};
    use crate::builtins::{default_scope, register_builtin};
    use crate::machine::core::{RuntimeArena, Scope};
    use crate::machine::model::ast::{KLiteral, TypeName};
    use crate::machine::model::types::{Argument, ExpressionSignature, KType, ReturnType};

    fn body_any<'a>(
        s: &'a Scope<'a>,
        _h: &mut dyn SchedulerHandle<'a>,
        _a: ArgumentBundle<'a>,
    ) -> BodyResult<'a> {
        BodyResult::Value(marker(s, "any"))
    }

    /// Coarse bucket-key lookup over the scope chain. Returns the first strict-shape
    /// match, falling back to any overload registered under the bucket so the
    /// classification check still runs against a real `KFunction` shape.
    fn find_match<'a>(scope: &'a Scope<'a>, expr: &KExpression<'a>) -> Option<&'a KFunction<'a>> {
        let key = expr.untyped_key();
        let mut current: Option<&Scope<'a>> = Some(scope);
        while let Some(s) = current {
            let functions = s.bindings().functions();
            if let Some(bucket) = functions.get(&key) {
                if let Some((f, _)) = bucket.iter().find(|(f, _)| f.signature.matches(expr)) {
                    return Some(*f);
                }
                if let Some((f, _)) = bucket.iter().next() {
                    return Some(*f);
                }
            }
            current = s.outer;
        }
        None
    }

    /// `OP <v:Number>` classified against `OP someName` (Identifier in Number slot)
    /// returns `wrap_indices = [1]` — the dispatcher wraps `someName` as a sub-Dispatch
    /// resolved through the `BareIdentifier` fast lane.
    #[test]
    fn classify_returns_wrap_indices_for_value_slot_identifiers() {
        let arena = RuntimeArena::new();
        let scope = run_root_bare(&arena);
        let sig = ExpressionSignature {
            return_type: ReturnType::Resolved(KType::Any),
            elements: vec![
                SignatureElement::Keyword("OP".into()),
                SignatureElement::Argument(Argument {
                    name: "v".into(),
                    ktype: KType::Number,
                }),
            ],
        };
        register_builtin(scope, "OP", sig, body_any);
        let expr = KExpression::new(vec![
            Spanned::bare(ExpressionPart::Keyword("OP".into())),
            Spanned::bare(ExpressionPart::Identifier("someName".into())),
        ]);
        let f = find_match(scope, &expr).expect("OP <Number> should match");
        let pick = f.classify_for_pick(&expr);
        assert_eq!(pick.wrap_indices, vec![1]);
        assert!(pick.ref_name_indices.is_empty());
        assert!(!pick.picked_has_binder_name);
    }

    /// `<verb:Identifier> <args:KExpression>` picked against `myFn (x: 1)` returns
    /// `ref_name_indices = [0]`: the Identifier slot is a literal-name reference and
    /// the function has no `binder_name`, so replay-park checks whether `myFn`
    /// resolves to a placeholder.
    #[test]
    fn classify_returns_ref_name_indices_for_non_binder_function() {
        let arena = RuntimeArena::new();
        let scope = run_root_bare(&arena);
        let sig = ExpressionSignature {
            return_type: ReturnType::Resolved(KType::Any),
            elements: vec![
                SignatureElement::Argument(Argument {
                    name: "verb".into(),
                    ktype: KType::Identifier,
                }),
                SignatureElement::Argument(Argument {
                    name: "args".into(),
                    ktype: KType::KExpression,
                }),
            ],
        };
        register_builtin(scope, "ident_call_probe", sig, body_any);
        let inner = KExpression::new(vec![
            Spanned::bare(ExpressionPart::Identifier("x".into())),
            Spanned::bare(ExpressionPart::Keyword(":".into())),
            Spanned::bare(ExpressionPart::Literal(KLiteral::Number(1.0))),
        ]);
        let expr = KExpression::new(vec![
            Spanned::bare(ExpressionPart::Identifier("myFn".into())),
            Spanned::bare(ExpressionPart::Expression(Box::new(inner))),
        ]);
        let f = find_match(scope, &expr)
            .expect("test overload should match an Identifier-leading expression");
        let pick = f.classify_for_pick(&expr);
        assert!(pick.ref_name_indices.contains(&0));
        assert!(!pick.picked_has_binder_name);
    }

    /// LET has `binder_name = Some(_)`, so its Identifier name slot is a *declaration*,
    /// not a reference, and `classify_for_pick` must exclude it from `ref_name_indices`.
    #[test]
    fn classify_skips_ref_name_indices_for_binder_function() {
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let expr = KExpression::new(vec![
            Spanned::bare(ExpressionPart::Keyword("LET".into())),
            Spanned::bare(ExpressionPart::Identifier("x".into())),
            Spanned::bare(ExpressionPart::Keyword("=".into())),
            Spanned::bare(ExpressionPart::Literal(KLiteral::Number(1.0))),
        ]);
        let f = find_match(scope, &expr).expect("LET should match");
        let pick = f.classify_for_pick(&expr);
        assert!(pick.picked_has_binder_name);
        assert!(
            pick.ref_name_indices.is_empty(),
            "LET's Identifier name slot is a declaration, not a reference; \
             should not be ref_name_index. Got {:?}",
            pick.ref_name_indices,
        );
    }

    /// A bare leaf Type-token in a `TypeExprRef` slot lands in `ref_name_indices` the
    /// same way an Identifier in an Identifier slot does. Symmetry pinned by
    /// [design/execution-model.md § Dispatch-time name placeholders](../../../design/execution-model.md#dispatch-time-name-placeholders).
    #[test]
    fn classify_type_token_in_typeexprref_slot_returns_ref_name_indices() {
        let arena = RuntimeArena::new();
        let scope = run_root_bare(&arena);
        let sig = ExpressionSignature {
            return_type: ReturnType::Resolved(KType::Any),
            elements: vec![
                SignatureElement::Keyword("OP".into()),
                SignatureElement::Argument(Argument {
                    name: "v".into(),
                    ktype: KType::TypeExprRef,
                }),
            ],
        };
        register_builtin(scope, "OP", sig, body_any);
        let expr = KExpression::new(vec![
            Spanned::bare(ExpressionPart::Keyword("OP".into())),
            Spanned::bare(ExpressionPart::Type(TypeName::leaf("IntOrd".into()))),
        ]);
        let f = find_match(scope, &expr).expect("OP <TypeExprRef> should match");
        let pick = f.classify_for_pick(&expr);
        assert_eq!(pick.ref_name_indices, vec![1]);
        assert!(pick.wrap_indices.is_empty());
        assert!(!pick.picked_has_binder_name);
    }

    /// `is_functor`-flagged `KFunction` projects through `KObject::ktype()` as
    /// `KType::KFunctor`; unflagged stays `KType::KFunction`.
    #[test]
    fn function_value_ktype_projects_kfunctor_when_flagged() {
        use crate::machine::model::types::{ExpressionSignature, ReturnType};
        let arena = RuntimeArena::new();
        let scope = run_root_bare(&arena);
        let make_sig = || ExpressionSignature {
            return_type: ReturnType::Resolved(KType::Number),
            elements: vec![
                SignatureElement::Keyword("CALL".into()),
                SignatureElement::Argument(crate::machine::model::types::Argument {
                    name: "x".into(),
                    ktype: KType::Number,
                }),
            ],
        };
        let plain = KFunction::with_binder_name(make_sig(), Body::Builtin(body_any), scope, None);
        let plain_obj = KObject::KFunction(arena.alloc_function(plain), None);
        assert!(matches!(plain_obj.ktype(), KType::KFunction { .. }));
        let functor = KFunction::with_binder_and_functor(
            make_sig(),
            Body::Builtin(body_any),
            scope,
            None,
            None,
            true,
            false,
        );
        let functor_obj = KObject::KFunction(arena.alloc_function(functor), None);
        match functor_obj.ktype() {
            KType::KFunctor { params, ret } => {
                assert_eq!(params.get("x"), Some(&KType::Number));
                assert_eq!(params.len(), 1);
                assert_eq!(*ret, KType::Number);
            }
            other => panic!("expected KFunctor, got {:?}", other),
        }
    }

    /// A bare leaf Type-token in an `Any` slot lands in `wrap_indices` — the auto-wrap
    /// pass rewrites it into a sub-Dispatch resolved through the `BareTypeLeaf` fast
    /// lane.
    #[test]
    fn classify_type_token_in_any_slot_returns_wrap_indices() {
        let arena = RuntimeArena::new();
        let scope = run_root_bare(&arena);
        let sig = ExpressionSignature {
            return_type: ReturnType::Resolved(KType::Any),
            elements: vec![
                SignatureElement::Keyword("OP".into()),
                SignatureElement::Argument(Argument {
                    name: "v".into(),
                    ktype: KType::Any,
                }),
            ],
        };
        register_builtin(scope, "OP", sig, body_any);
        let expr = KExpression::new(vec![
            Spanned::bare(ExpressionPart::Keyword("OP".into())),
            Spanned::bare(ExpressionPart::Type(TypeName::leaf("Number".into()))),
        ]);
        let f = find_match(scope, &expr).expect("OP <Any> should match");
        let pick = f.classify_for_pick(&expr);
        assert_eq!(pick.wrap_indices, vec![1]);
        assert!(pick.ref_name_indices.is_empty());
        assert!(!pick.picked_has_binder_name);
    }
}
