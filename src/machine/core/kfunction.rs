//! `KFunction` — the callable Koan function value, plus the scheduler-facing types
//! a body depends on. A `KFunction` carries an `ExpressionSignature` (its call shape),
//! a `Body` (builtin `fn` pointer or captured user-defined `KExpression`), and the
//! lexical scope captured at definition time. `bind` produces a `KFuture` from a
//! positional call; `apply` rewrites a named-argument call into a tail-form
//! `BodyResult` for the scheduler to run.
//!
//! Submodules:
//! - [`argument_bundle`] — the resolved name-to-value map passed to a body, plus the
//!   slot-extraction helpers used by binder builtins.
//! - [`scheduler_handle`] — `NodeId`, the `SchedulerHandle` trait, and `CombineFinish`.
//! - [`body`] — `BodyResult`, `BuiltinFn`, `PreRunFn`, and the `Body` enum.
//! - [`invoke`] — `KFunction::invoke` (the body-runner) that binds parameters into a
//!   per-call child scope and returns a tail-call.
//! - [`pick`] — dispatch-shape classification (`accepts_for_wrap`,
//!   `classify_for_pick`, `ClassifiedSlots`) used by `resolve_dispatch`.

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
pub use body::{Body, BodyResult, BuiltinFn, PreRunBucketFn, PreRunFn};
pub use pick::ClassifiedSlots;
pub use scheduler_handle::{CatchFinish, CombineFinish, NodeId, SchedulerHandle};

/// A callable Koan function: signature, body, and the lexical environment captured at
/// definition time (the scope that ran the `FN ...` form, or run-root for builtins).
///
/// The captured-scope handle is carried at the type level via `NonNull<Scope<'a>>` +
/// `PhantomData<&'a Scope<'a>>` (see the `captured` field). One `unsafe { NonNull::as_ref }`
/// remains inside [`KFunction::captured_scope`]; everything else flows through the type
/// system.
///
/// SAFETY: the captured scope is allocated in a `RuntimeArena` that outlives this
/// `KFunction` — they share the arena (FN registers the function in the same scope it
/// captures; builtins are registered in run-root). See `runtime/arena.rs` for the broader
/// lifetime-erasure pattern.
pub struct KFunction<'a> {
    pub signature: ExpressionSignature<'a>,
    pub body: Body<'a>,
    /// Captured definition-scope pointer. **Variance-load-bearing.** `Scope<'a>` is
    /// invariant in `'a` (it contains `RefCell`s), so the paired
    /// `PhantomData<&'a Scope<'a>>` below is required to keep `KFunction<'a>` invariant in
    /// `'a`. Do **not** simplify `_p` to `PhantomData<&'a ()>` — that would make
    /// `KFunction` covariant in `'a` and silently reintroduce the soundness bug the old
    /// `*const Scope<'static>` erasure was working around the wrong way. The constructor
    /// (`with_pre_run`) takes `&'a Scope<'a>` directly and stores it via `NonNull::from`,
    /// so the only `unsafe` site is the `NonNull::as_ref` deref in `captured_scope`.
    captured: NonNull<Scope<'a>>,
    _p: PhantomData<&'a Scope<'a>>,
    /// `Some(_)` for binder builtins (LET, FN, STRUCT, UNION, SIG, MODULE); `None` for
    /// everything else. See [`PreRunFn`].
    pub pre_run: Option<PreRunFn>,
    /// `Some(_)` for binder builtins whose body registers a callable function — `FN`
    /// and `FUNCTOR`. Returns the *inner-call* bucket key (e.g. `(MAKESET _)`) so the
    /// dispatch driver installs an entry in `bindings.pending_overloads` and a
    /// sibling bare-arg call form like `(MAKESET IntOrd)` parks on the binder slot
    /// instead of surfacing `DispatchFailed` before finalize. See [`PreRunBucketFn`]
    /// for the rationale on keying by bucket rather than lead keyword.
    pub pre_run_bucket: Option<PreRunBucketFn>,
    /// Flipped on by the `FUNCTOR` binder (and stays `false` for `FN`). Distinguishes
    /// the same underlying `KFunction` shape into the two type-language families:
    /// `function_value_ktype` projects `is_functor → KType::KFunctor`, else
    /// `KType::KFunction`. The cross-arm wall in `function_compat` refuses both
    /// directions of the `KFunctor`/`KFunction` mismatch silently — see
    /// [design/typing/functors.md](../../../design/typing/functors.md).
    pub is_functor: bool,
}

impl<'a> KFunction<'a> {
    pub fn new(
        signature: ExpressionSignature<'a>,
        body: Body<'a>,
        captured: &'a Scope<'a>,
    ) -> Self {
        Self::with_pre_run(signature, body, captured, None)
    }

    pub fn with_pre_run(
        signature: ExpressionSignature<'a>,
        body: Body<'a>,
        captured: &'a Scope<'a>,
        pre_run: Option<PreRunFn>,
    ) -> Self {
        Self::with_pre_run_and_functor(signature, body, captured, pre_run, None, false)
    }

    /// Like [`Self::with_pre_run`] but lets the caller flip the `is_functor` flag at
    /// construction time and pass a `pre_run_bucket` extractor (for `FN` / `FUNCTOR`,
    /// whose body registers a callable function and so wants a bucket-keyed
    /// pending-overload entry). Used by the `FUNCTOR` binder; everything else routes
    /// through `with_pre_run` and leaves both new fields at their defaults.
    pub fn with_pre_run_and_functor(
        mut signature: ExpressionSignature<'a>,
        body: Body<'a>,
        captured: &'a Scope<'a>,
        pre_run: Option<PreRunFn>,
        pre_run_bucket: Option<PreRunBucketFn>,
        is_functor: bool,
    ) -> Self {
        signature.normalize();
        Self {
            signature,
            body,
            captured: NonNull::from(captured),
            _p: PhantomData,
            pre_run,
            pre_run_bucket,
            is_functor,
        }
    }

    /// Re-borrow the captured scope at `'a`. SAFETY: `captured` was built from
    /// `NonNull::from(&'a Scope<'a>)` in [`Self::with_pre_run`], so the pointer is non-null
    /// and points at a `Scope<'a>` that outlives this `KFunction<'a>` by the broader
    /// runtime-arena SAFETY argument (see `core/arena.rs::RuntimeArena`).
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
                    args.insert(arg.name.clone(), Rc::new(part_value.resolve_for(&arg.ktype)));
                }
            }
        }
        Ok(KFuture {
            parsed: expr,
            function: self,
            bundle: ArgumentBundle { args },
        })
    }

    /// Apply this function to a named-argument list (the inner parts of `f (a: 1, b: 2)`):
    /// parse name-value pairs, consume one per declared argument, and emit a
    /// `BodyResult::Tail` matching the keyword-bucketed signature on re-dispatch.
    ///
    /// Validation precedence (first wins): missing arg → unknown arg. Arity is implicit —
    /// [`NamedPairs`] rejects duplicate names at parse time, so consuming every declared
    /// argument and finding the residual empty witnesses an exact match.
    pub fn apply<'b>(&self, args: Vec<Spanned<ExpressionPart<'b>>>) -> BodyResult<'b> {
        let tmp_expr = KExpression::new(args);
        let mut pairs = match NamedPairs::parse(&tmp_expr, "function call") {
            Ok(p) => p,
            Err(msg) => return BodyResult::Err(KError::new(KErrorKind::ShapeError(msg))),
        };
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
                        return BodyResult::Err(KError::new(KErrorKind::MissingArg(a.name.clone())));
                    }
                },
            }
        }
        if let Some(unknown) = pairs.into_unknown() {
            return BodyResult::Err(KError::new(KErrorKind::ShapeError(format!(
                "unknown name `{unknown}` in function call",
            ))));
        }
        BodyResult::tail(KExpression::new(parts))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::model::ast::{KLiteral, TypeExpr};
    use crate::builtins::test_support::{marker, run_root_bare};
    use crate::builtins::{default_scope, register_builtin};
    use crate::machine::core::{RuntimeArena, Scope};
    use crate::machine::model::types::{Argument, ExpressionSignature, KType, ReturnType};

    fn body_any<'a>(
        s: &'a Scope<'a>,
        _h: &mut dyn SchedulerHandle<'a>,
        _a: ArgumentBundle<'a>,
    ) -> BodyResult<'a> {
        BodyResult::Value(marker(s, "any"))
    }

    /// Walk the scope chain and return the first overload whose strict-or-tentative shape
    /// matches `expr` — the chain-walk half of [`Scope::resolve_dispatch`], factored out
    /// here so the migrated tests can assert on `f.classify_for_pick(&expr)` directly
    /// without re-invoking the full resolution outcome.
    fn find_match<'a>(
        scope: &'a Scope<'a>,
        expr: &KExpression<'a>,
    ) -> Option<&'a KFunction<'a>> {
        let key = expr.untyped_key();
        let mut current: Option<&Scope<'a>> = Some(scope);
        while let Some(s) = current {
            let functions = s.bindings().functions();
            if let Some(bucket) = functions.get(&key) {
                for f in bucket.iter() {
                    if f.signature.matches(expr) {
                        return Some(*f);
                    }
                }
                for f in bucket.iter() {
                    if f.accepts_for_wrap(expr) {
                        return Some(*f);
                    }
                }
            }
            current = s.outer;
        }
        None
    }

    /// A function whose signature is `OP <v:Number>` classified against `OP someName` (where
    /// `someName` is a bare Identifier in a Number-typed slot) returns `wrap_indices = [1]`
    /// and no ref_name_indices — the dispatcher will wrap `someName` as a sub-Dispatch so
    /// it resolves through `value_lookup` (or the bare-name short-circuit, if the name is
    /// bound).
    #[test]
    fn classify_returns_wrap_indices_for_value_slot_identifiers() {
        let arena = RuntimeArena::new();
        let scope = run_root_bare(&arena);
        let sig = ExpressionSignature {
            return_type: ReturnType::Resolved(KType::Any),
            elements: vec![
                SignatureElement::Keyword("OP".into()),
                SignatureElement::Argument(Argument { name: "v".into(), ktype: KType::Number }),
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
        assert!(!pick.picked_has_pre_run);
    }

    /// `call_by_name`'s shape — `<verb:Identifier> <args:KExpression>` — picked against
    /// `myFn (x: 1)` returns ref_name_indices = [0]: the Identifier slot is a literal-name
    /// reference and the function has no pre_run, so replay-park will check whether `myFn`
    /// resolves to a placeholder.
    #[test]
    fn classify_returns_ref_name_indices_for_non_pre_run_function() {
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
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
            .expect("call_by_name should match Identifier-leading expression");
        let pick = f.classify_for_pick(&expr);
        assert!(pick.ref_name_indices.contains(&0));
        assert!(!pick.picked_has_pre_run);
    }

    /// LET's name slot is `Identifier` (or `TypeExprRef`), but LET has `pre_run = Some(_)` —
    /// so `classify_for_pick` should NOT include the name slot in `ref_name_indices`.
    /// Binder literal-name slots are *declarations*, not references; replay-park must skip
    /// them.
    #[test]
    fn classify_skips_ref_name_indices_for_pre_run_function() {
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
        assert!(pick.picked_has_pre_run);
        assert!(
            pick.ref_name_indices.is_empty(),
            "LET's Identifier name slot is a declaration, not a reference; \
             should not be ref_name_index. Got {:?}",
            pick.ref_name_indices,
        );
    }

    /// A non-pre_run function whose slot is `TypeExprRef`, classified against a bare leaf
    /// Type-token, lands the Type slot in `ref_name_indices` the same way an Identifier in
    /// an Identifier slot does — replay-park parks the call on the Type-token's
    /// placeholder. Symmetry pinned by
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
            Spanned::bare(ExpressionPart::Type(TypeExpr::leaf("IntOrd".into()))),
        ]);
        let f = find_match(scope, &expr).expect("OP <TypeExprRef> should match");
        let pick = f.classify_for_pick(&expr);
        assert_eq!(pick.ref_name_indices, vec![1]);
        assert!(pick.wrap_indices.is_empty());
        assert!(!pick.picked_has_pre_run);
    }

    /// Stage 2: a manually-constructed `KFunction` with the `is_functor` flag set
    /// projects through `KObject::ktype()` as `KType::KFunctor`; the unflagged
    /// case stays `KType::KFunction`. Pins the projection split that drives the
    /// cross-arm wall in `function_compat`.
    #[test]
    fn function_value_ktype_projects_kfunctor_when_flagged() {
        use crate::machine::model::types::{ReturnType, ExpressionSignature};
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
        // Plain FN: `is_functor: false`, projects KFunction.
        let plain = KFunction::with_pre_run(make_sig(), Body::Builtin(body_any), scope, None);
        let plain_obj = KObject::KFunction(arena.alloc_function(plain), None);
        assert!(matches!(plain_obj.ktype(), KType::KFunction { .. }));
        // Flagged FN: `is_functor: true`, projects KFunctor.
        let functor = KFunction::with_pre_run_and_functor(
            make_sig(),
            Body::Builtin(body_any),
            scope,
            None,
            None,
            true,
        );
        let functor_obj = KObject::KFunction(arena.alloc_function(functor), None);
        match functor_obj.ktype() {
            KType::KFunctor { params, ret } => {
                assert_eq!(params, vec![KType::Number]);
                assert_eq!(*ret, KType::Number);
            }
            other => panic!("expected KFunctor, got {:?}", other),
        }
    }

    /// Companion to the literal-name slot case: a bare leaf Type-token in an `Any` slot of a
    /// non-binder lands in `wrap_indices` so the auto-wrap pass rewrites it into a
    /// sub-Dispatch that resolves through the TypeExprRef overload of `value_lookup`.
    /// `LET T = Number` walks the same wrap path as `LET y = z`.
    #[test]
    fn classify_type_token_in_any_slot_returns_wrap_indices() {
        let arena = RuntimeArena::new();
        let scope = run_root_bare(&arena);
        let sig = ExpressionSignature {
            return_type: ReturnType::Resolved(KType::Any),
            elements: vec![
                SignatureElement::Keyword("OP".into()),
                SignatureElement::Argument(Argument { name: "v".into(), ktype: KType::Any }),
            ],
        };
        register_builtin(scope, "OP", sig, body_any);
        let expr = KExpression::new(vec![
            Spanned::bare(ExpressionPart::Keyword("OP".into())),
            Spanned::bare(ExpressionPart::Type(TypeExpr::leaf("Number".into()))),
        ]);
        let f = find_match(scope, &expr).expect("OP <Any> should match");
        let pick = f.classify_for_pick(&expr);
        assert_eq!(pick.wrap_indices, vec![1]);
        assert!(pick.ref_name_indices.is_empty());
        assert!(!pick.picked_has_pre_run);
    }
}
