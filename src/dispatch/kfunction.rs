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

use std::collections::HashMap;
use std::rc::Rc;

use crate::parse::kexpression::{ExpressionPart, KExpression};

use crate::dispatch::runtime::{KError, KErrorKind, KFuture, Scope};
use crate::dispatch::types::{ExpressionSignature, Parseable, SignatureElement};
use crate::dispatch::values::{parse_named_value_pairs, KObject};

pub mod argument_bundle;
pub mod body;
pub mod scheduler_handle;

pub use argument_bundle::ArgumentBundle;
pub use body::{Body, BodyResult, BuiltinFn, PreRunFn};
pub use scheduler_handle::{CombineFinish, NodeId, SchedulerHandle};

/// A callable Koan function: signature, body, and the lexical environment captured at
/// definition time (the scope that ran the `FN ...` form, or run-root for builtins).
///
/// `captured` is lifetime-erased to `*const Scope<'static>` to keep `KFunction<'a>`
/// covariant in `'a`; storing a real `&'a Scope<'a>` would make `KFunction` invariant
/// (because `Scope<'a>` is invariant via its `RefCell`s) and would break builtin
/// registration's coercion from `'static` to shorter lifetimes.
///
/// SAFETY: the captured scope is allocated in a `RuntimeArena` that outlives this
/// `KFunction` — they share the arena (FN registers the function in the same scope it
/// captures; builtins are registered in run-root). See `runtime/arena.rs` for the broader
/// lifetime-erasure pattern.
pub struct KFunction<'a> {
    pub signature: ExpressionSignature,
    pub body: Body<'a>,
    captured: *const Scope<'static>,
    /// `Some(_)` for binder builtins (LET, FN, STRUCT, UNION, SIG, MODULE); `None` for
    /// everything else. See [`PreRunFn`].
    pub pre_run: Option<PreRunFn>,
}

impl<'a> KFunction<'a> {
    pub fn new(
        signature: ExpressionSignature,
        body: Body<'a>,
        captured: &'a Scope<'a>,
    ) -> Self {
        Self::with_pre_run(signature, body, captured, None)
    }

    pub fn with_pre_run(
        mut signature: ExpressionSignature,
        body: Body<'a>,
        captured: &'a Scope<'a>,
        pre_run: Option<PreRunFn>,
    ) -> Self {
        signature.normalize();
        // Double cast: `Scope` is invariant in `'a`, so a single cast to
        // `*const Scope<'static>` is rejected.
        #[allow(clippy::unnecessary_cast)]
        let captured = captured as *const Scope<'_> as *const Scope<'static>;
        Self { signature, body, captured, pre_run }
    }

    /// Re-attaches the captured scope pointer to a fresh `'a`. Soundness rests on the
    /// SAFETY argument on `KFunction`: the scope's allocation outlives this `KFunction`.
    pub fn captured_scope(&self) -> &'a Scope<'a> {
        unsafe { &*self.captured.cast::<Scope<'a>>() }
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
