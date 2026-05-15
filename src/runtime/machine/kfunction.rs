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
//! - [`invoke`] — `KFunction::invoke` (the body-runner) and `substitute_params` (the
//!   parameter-Identifier rewriter user-fn bodies use on entry).

use std::collections::HashMap;
use std::marker::PhantomData;
use std::ptr::NonNull;
use std::rc::Rc;

use crate::ast::{ExpressionPart, KExpression};

use crate::runtime::machine::core::{KError, KErrorKind, KFuture, Scope};
use crate::runtime::model::types::{Argument, ExpressionSignature, KType, Parseable, SignatureElement};
use crate::runtime::model::values::{parse_named_value_pairs, KObject};

pub mod argument_bundle;
pub mod body;
pub mod invoke;
pub mod scheduler_handle;

pub use argument_bundle::ArgumentBundle;
pub use body::{Body, BodyResult, BuiltinFn, PreRunFn};
pub(crate) use invoke::substitute_params;
pub use scheduler_handle::{CombineFinish, NodeId, SchedulerHandle};

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
}

/// Per-slot classification produced by [`KFunction::classify_for_pick`]:
/// - `eager_indices`: `Some(indices)` iff the picked function is a *lazy candidate* (has at
///   least one `KType::KExpression` slot bound by an `ExpressionPart::Expression`); the
///   carried indices are the `Expression` parts in *non*-`KExpression` slots that must
///   evaluate eagerly. `None` when the function isn't a lazy candidate — the scheduler
///   then schedules every eager-shaped part (`Expression` / `ListLiteral` / `DictLiteral`)
///   as a sub-Dispatch.
/// - `wrap_indices`: bare-Identifier / bare-Type parts in non-literal-name slots to
///   auto-wrap as sub-Dispatches.
/// - `ref_name_indices`: bare-Identifier / bare-Type parts in literal-name slots
///   (`KType::Identifier` / `KType::TypeExprRef`) of a non-`pre_run` function; candidates
///   for replay-park.
///
/// `picked_has_pre_run` distinguishes binder-shaped expressions (literal-name slots are
/// declarations) from call-shaped expressions (literal-name slots are references that may
/// need to park). The three index vectors are disjoint by construction over disjoint
/// `(SignatureElement, ExpressionPart)` shapes — `classify_for_pick` is the sole producer.
pub struct ClassifiedSlots {
    pub eager_indices: Option<Vec<usize>>,
    pub wrap_indices: Vec<usize>,
    pub ref_name_indices: Vec<usize>,
    pub picked_has_pre_run: bool,
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
        mut signature: ExpressionSignature<'a>,
        body: Body<'a>,
        captured: &'a Scope<'a>,
        pre_run: Option<PreRunFn>,
    ) -> Self {
        signature.normalize();
        Self {
            signature,
            body,
            captured: NonNull::from(captured),
            _p: PhantomData,
            pre_run,
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

    /// Lazy-candidate shape check for this function: is `self` a viable lazy match for
    /// `expr`, and if so what are the indices of its eager-Expression parts? Returns `None`
    /// when this function isn't a lazy candidate (length mismatch, fixed-token mismatch, no
    /// `KExpression` slot binding an `Expression` part, or any other arg-type mismatch).
    /// Lazy means at least one `KType::KExpression` slot is bound by an
    /// `ExpressionPart::Expression`; the caller schedules the eager indices as deps and
    /// leaves the lazy ones in place for the receiving builtin to dispatch itself.
    pub fn lazy_eager_indices(&self, expr: &KExpression<'_>) -> Option<Vec<usize>> {
        let sig = &self.signature;
        if sig.elements.len() != expr.parts.len() {
            return None;
        }
        let mut eager_indices: Vec<usize> = Vec::new();
        let mut has_lazy_slot = false;
        for (i, (el, part)) in sig.elements.iter().zip(expr.parts.iter()).enumerate() {
            match (el, part) {
                (SignatureElement::Keyword(s), ExpressionPart::Keyword(t)) if s == t => {}
                (SignatureElement::Keyword(_), _) => return None,
                (SignatureElement::Argument(arg), part) => match (&arg.ktype, part) {
                    (KType::KExpression, ExpressionPart::Expression(_)) => {
                        has_lazy_slot = true;
                    }
                    (KType::KExpression, _) => return None,
                    (_, ExpressionPart::Expression(_)) => {
                        // Speculative: assume the eager-evaluated result will type-match at
                        // late dispatch. If not, dispatch will fail at that point.
                        eager_indices.push(i);
                    }
                    (_, other) => {
                        // Mirror `accepts_for_wrap`'s bare-name relaxation: a bare
                        // Identifier or bare leaf-Type part in any slot whose declared
                        // type isn't `Identifier` / `TypeExprRef` is auto-wrap-eligible.
                        // The auto-wrap pass (`apply_auto_wrap`) rewrites the part into
                        // a single-name sub-Dispatch that re-enters via the bare-name
                        // short-circuit before late dispatch matches the lifted value.
                        // Admitting the part here keeps the function's lazy candidacy
                        // intact when a sibling `KExpression+Expression` slot is the
                        // one driving laziness — without this, `SIG_WITH OrderedSig (...)`
                        // would lose its lazy candidacy on the `sig: Signature` /
                        // `Type(OrderedSig)` pairing and the `schedule_deps` None-arm
                        // would sub-Dispatch the bindings group, defeating the lazy
                        // contract for the `KExpression` slot.
                        let is_bare_name = matches!(
                            other,
                            ExpressionPart::Identifier(_)
                                | ExpressionPart::Type(crate::ast::TypeExpr {
                                    params: crate::ast::TypeParams::None,
                                    ..
                                })
                        );
                        if is_bare_name
                            && !matches!(arg.ktype, KType::Identifier | KType::TypeExprRef)
                        {
                            continue;
                        }
                        if !arg.matches(other) {
                            return None;
                        }
                    }
                },
            }
        }
        if has_lazy_slot { Some(eager_indices) } else { None }
    }

    /// Auto-wrap-permissive shape check. Speculatively admits two relaxations beyond the
    /// strict matcher:
    ///
    /// - Bare-Identifier and bare leaf-Type parts in any slot whose declared type isn't
    ///   `Identifier` or `TypeExprRef`. The auto-wrap pass rewrites these into single-name
    ///   sub-Dispatches that re-enter via the bare-name short-circuit and route through
    ///   the Identifier / TypeExprRef overload of `value_lookup`. Covers both
    ///   `MAKESET some_var` (Identifier) and `MAKESET IntOrd` (Type-token).
    /// - Parens-wrapped `Expression` parts in non-`KExpression` slots — *but only when*
    ///   the signature also has at least one `KExpression` slot bound by an `Expression`
    ///   part (i.e. the function is a [`Self::lazy_eager_indices`] candidate). The
    ///   post-pick scheduler then routes the non-`KExpression` slot's `Expression`
    ///   through `eager_indices` for sub-Dispatch while leaving the lazy
    ///   `KExpression+Expression` pair untouched, and splices the resulting `Future(_)`
    ///   back for strict re-matching. Covers shapes like `FN (...) -> Mo.Ty = (...)`
    ///   where the return-type slot is `Expression([ATTR Mo Ty])` and FN's `signature`/
    ///   `body` slots are also `Expression` parts. Functions without a `KExpression`
    ///   slot (e.g. `LIST_OF Mo.Ty`, `PLUS (deep_call) OP 1`) ride the
    ///   `resolve_dispatch::Deferred` path instead, where `schedule_eager_fallthrough`
    ///   sub-Dispatches every `Expression` part uniformly — equivalent end state without
    ///   the false-tentative-match noise that would otherwise show up here.
    ///
    /// All other slot/part pairings reuse the normal `Argument::matches` check.
    pub fn accepts_for_wrap(&self, expr: &KExpression<'_>) -> bool {
        let sig = &self.signature;
        if sig.elements.len() != expr.parts.len() {
            return false;
        }
        // Pre-compute whether this function has a `KExpression+Expression` lazy slot — gates
        // the Expression-in-non-KExpression-slot relaxation below so non-lazy candidates
        // keep their existing `Deferred` path.
        let has_lazy_kexpr_slot = sig.elements.iter().zip(expr.parts.iter()).any(|(el, part)| {
            matches!(
                (el, part),
                (
                    SignatureElement::Argument(Argument { ktype: KType::KExpression, .. }),
                    ExpressionPart::Expression(_),
                )
            )
        });
        for (el, part) in sig.elements.iter().zip(expr.parts.iter()) {
            match (el, part) {
                (SignatureElement::Keyword(s), ExpressionPart::Keyword(t)) if s == t => {}
                (SignatureElement::Keyword(_), _) => return false,
                (SignatureElement::Argument(arg), part) => {
                    let is_bare_name = matches!(
                        part,
                        ExpressionPart::Identifier(_)
                            | ExpressionPart::Type(crate::ast::TypeExpr {
                                params: crate::ast::TypeParams::None,
                                ..
                            })
                    );
                    if is_bare_name
                        && !matches!(arg.ktype, KType::Identifier | KType::TypeExprRef)
                    {
                        continue;
                    }
                    if has_lazy_kexpr_slot
                        && matches!(part, ExpressionPart::Expression(_))
                        && !matches!(arg.ktype, KType::KExpression)
                    {
                        continue;
                    }
                    if !arg.matches(part) {
                        return false;
                    }
                }
            }
        }
        true
    }

    /// Per-slot classification: classify `expr`'s slots against `self`'s signature into
    /// three disjoint index buckets — `eager_indices`, `wrap_indices`, `ref_name_indices` —
    /// plus a `picked_has_pre_run` flag. Identifier and bare leaf Type-token
    /// (`TypeParams::None`) parts are treated symmetrically — both name-shaped parts ride
    /// the same wrap-or-park rails, so `LET T = Number` and `LET y = z` (and their
    /// forward-reference variants) walk identical scheduler paths.
    ///
    /// Disjointness is guaranteed by construction: each slot's `(SignatureElement,
    /// ExpressionPart)` shape lands in at most one bucket. The classifier is the sole
    /// producer of these vectors; the downstream scheduler may rely on the invariant.
    pub fn classify_for_pick(&self, expr: &KExpression<'_>) -> ClassifiedSlots {
        let eager_indices = self.lazy_eager_indices(expr);
        let mut wrap_indices: Vec<usize> = Vec::new();
        let mut ref_name_indices: Vec<usize> = Vec::new();
        let picked_has_pre_run = self.pre_run.is_some();
        for (i, (el, part)) in self.signature.elements.iter().zip(expr.parts.iter()).enumerate() {
            let SignatureElement::Argument(arg) = el else { continue };
            let is_bare_name = matches!(
                part,
                ExpressionPart::Identifier(_)
                    | ExpressionPart::Type(crate::ast::TypeExpr {
                        params: crate::ast::TypeParams::None,
                        ..
                    })
            );
            if !is_bare_name {
                continue;
            }
            match &arg.ktype {
                // Bare name in literal-name slot: replay-park iff the picked function isn't
                // a binder. Binders' literal-name slots are *declarations*; the slot already
                // owns the name and must not park on its own placeholder.
                KType::Identifier | KType::TypeExprRef => {
                    if !picked_has_pre_run {
                        ref_name_indices.push(i);
                    }
                }
                // Bare name in any other slot (including `Any`): auto-wrap. The wrap
                // rewrites the part into a sub-Dispatch that re-enters via the bare-name
                // short-circuit and routes through the Identifier / TypeExprRef overload of
                // `value_lookup`. Covers both `LET y = z` and `LET T = Number` /
                // `MAKESET IntOrd` symmetrically.
                _ => wrap_indices.push(i),
            }
        }
        ClassifiedSlots {
            eager_indices,
            wrap_indices,
            ref_name_indices,
            picked_has_pre_run,
        }
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
            match el {
                SignatureElement::Keyword(s) => match part {
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
                    if !arg.matches(part) {
                        return Err(KError::new(KErrorKind::TypeMismatch {
                            arg: arg.name.clone(),
                            expected: arg.ktype.name(),
                            got: part.summarize(),
                        }));
                    }
                    args.insert(arg.name.clone(), Rc::new(part.resolve_for(&arg.ktype)));
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
    /// parse name-value pairs, reorder values into signature order, and emit a
    /// `BodyResult::Tail` matching the keyword-bucketed signature on re-dispatch.
    ///
    /// Validation precedence (first wins): missing arg → unknown arg → arity. Missing-first
    /// because "you forgot `b`" is more actionable than "you have a stray `c`".
    pub fn apply<'b>(&self, args: Vec<ExpressionPart<'b>>) -> BodyResult<'b> {
        let tmp_expr = KExpression { parts: args };
        let pairs = match parse_named_value_pairs(&tmp_expr, "function call") {
            Ok(p) => p,
            Err(msg) => return BodyResult::Err(KError::new(KErrorKind::ShapeError(msg))),
        };
        let arg_names: Vec<&str> = self
            .signature
            .elements
            .iter()
            .filter_map(|el| match el {
                SignatureElement::Argument(a) => Some(a.name.as_str()),
                _ => None,
            })
            .collect();
        for name in &arg_names {
            if !pairs.iter().any(|(n, _)| n == name) {
                return BodyResult::Err(KError::new(KErrorKind::MissingArg((*name).to_string())));
            }
        }
        for (pair_name, _) in &pairs {
            if !arg_names.iter().any(|n| n == pair_name) {
                return BodyResult::Err(KError::new(KErrorKind::ShapeError(format!(
                    "unknown name `{}` in function call",
                    pair_name
                ))));
            }
        }
        if pairs.len() != arg_names.len() {
            return BodyResult::Err(KError::new(KErrorKind::ArityMismatch {
                expected: arg_names.len(),
                got: pairs.len(),
            }));
        }
        let mut parts = Vec::with_capacity(self.signature.elements.len());
        for el in &self.signature.elements {
            match el {
                SignatureElement::Keyword(s) => parts.push(ExpressionPart::Keyword(s.clone())),
                SignatureElement::Argument(a) => {
                    let value_part = pairs
                        .iter()
                        .find(|(n, _)| n == &a.name)
                        .map(|(_, v)| v.clone())
                        .expect("missing-arg check above guarantees presence");
                    parts.push(value_part);
                }
            }
        }
        BodyResult::tail(KExpression { parts })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{KLiteral, TypeExpr};
    use crate::runtime::builtins::test_support::{marker, run_root_bare};
    use crate::runtime::builtins::{default_scope, register_builtin};
    use crate::runtime::machine::core::{RuntimeArena, Scope};
    use crate::runtime::model::types::{Argument, ExpressionSignature, KType, ReturnType};

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
        expr: &KExpression<'_>,
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
        let expr = KExpression {
            parts: vec![
                ExpressionPart::Keyword("OP".into()),
                ExpressionPart::Identifier("someName".into()),
            ],
        };
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
        let inner = KExpression {
            parts: vec![
                ExpressionPart::Identifier("x".into()),
                ExpressionPart::Keyword(":".into()),
                ExpressionPart::Literal(KLiteral::Number(1.0)),
            ],
        };
        let expr = KExpression {
            parts: vec![
                ExpressionPart::Identifier("myFn".into()),
                ExpressionPart::Expression(Box::new(inner)),
            ],
        };
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
        let expr = KExpression {
            parts: vec![
                ExpressionPart::Keyword("LET".into()),
                ExpressionPart::Identifier("x".into()),
                ExpressionPart::Keyword("=".into()),
                ExpressionPart::Literal(KLiteral::Number(1.0)),
            ],
        };
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
        let expr = KExpression {
            parts: vec![
                ExpressionPart::Keyword("OP".into()),
                ExpressionPart::Type(TypeExpr::leaf("IntOrd".into())),
            ],
        };
        let f = find_match(scope, &expr).expect("OP <TypeExprRef> should match");
        let pick = f.classify_for_pick(&expr);
        assert_eq!(pick.ref_name_indices, vec![1]);
        assert!(pick.wrap_indices.is_empty());
        assert!(!pick.picked_has_pre_run);
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
        let expr = KExpression {
            parts: vec![
                ExpressionPart::Keyword("OP".into()),
                ExpressionPart::Type(TypeExpr::leaf("Number".into())),
            ],
        };
        let f = find_match(scope, &expr).expect("OP <Any> should match");
        let pick = f.classify_for_pick(&expr);
        assert_eq!(pick.wrap_indices, vec![1]);
        assert!(pick.ref_name_indices.is_empty());
        assert!(!pick.picked_has_pre_run);
    }
}
