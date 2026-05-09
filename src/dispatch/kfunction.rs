//! `KFunction` and the scheduler-facing types it depends on. A `KFunction` carries an
//! `ExpressionSignature` (its call shape), a `Body` (builtin `fn` pointer or captured
//! user-defined `KExpression`), and a captured scope for lexical lookup of free names.
//! `bind` produces a `KFuture` from a positional call; `apply` rewrites a named-argument
//! call into a tail-form `BodyResult` for the scheduler to run.

use std::collections::HashMap;
use std::rc::Rc;

use crate::parse::kexpression::{ExpressionPart, KExpression};

use super::runtime::{CallArena, KError, KErrorKind, KFuture, Scope};
use super::types::{ExpressionSignature, Parseable, SignatureElement};
use super::values::KObject;

/// Stable handle to a node in the scheduler's DAG. Lives in `dispatch` so `BodyResult` and
/// `SchedulerHandle` can name a node without `dispatch` importing from `execute`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct NodeId(pub usize);

impl NodeId {
    pub fn index(self) -> usize { self.0 }
}

/// Side-channel a builtin body uses to spawn additional `Dispatch` nodes. Defined in
/// `dispatch` so `BuiltinFn` can reference it without importing the scheduler module;
/// `Scheduler` impls it in `execute/scheduler.rs`.
///
/// `current_frame` returns the active slot's `Rc<CallArena>` so a builtin building a new
/// per-call frame whose child scope's `outer` points into the call site can chain that Rc
/// onto the new frame. Without this, MATCH-style builtins (whose new frame's outer is a
/// per-call scope, not a captured lexical scope) would hand out a frame whose `outer`
/// dangles the moment the slot's old frame is dropped on TCO replace.
pub trait SchedulerHandle<'a> {
    fn add_dispatch(&mut self, expr: KExpression<'a>, scope: &'a Scope<'a>) -> NodeId;
    fn current_frame(&self) -> Option<Rc<CallArena>>;
}

/// What a builtin's body returns.
///
/// `Tail { expr, frame: Some(f), .. }` installs the per-call `CallArena` `f` in the slot;
/// the scheduler rewrites the slot's work to `Dispatch(expr)` and re-runs it, so a chain of
/// tail calls reuses one slot. A TCO replace drops the slot's previous frame immediately;
/// for user-fn invokes that's safe (the new child scope's `outer` is the FN's captured
/// scope, not the previous frame's), and for builtins whose new child scope's `outer` IS
/// the call site (MATCH), the new frame holds the previous frame's `Rc` via
/// `CallArena::outer_frame` so the drop doesn't free memory still in use.
///
/// `Tail { frame: None, .. }` keeps the slot's existing frame and scope.
pub enum BodyResult<'a> {
    Value(&'a KObject<'a>),
    Tail {
        expr: KExpression<'a>,
        frame: Option<Rc<CallArena>>,
        /// User-fn reference attached to the slot for two purposes: the slot's Done arm
        /// reads `signature.return_type` to enforce the declared return type at runtime,
        /// and on error `function.summarize()` becomes the appended `Frame`'s function
        /// name. `None` for builtin tails that are deferred-eval continuations, not calls.
        function: Option<&'a KFunction<'a>>,
    },
    Err(KError),
}

impl<'a> BodyResult<'a> {
    pub fn tail(expr: KExpression<'a>) -> Self {
        BodyResult::Tail { expr, frame: None, function: None }
    }

    pub fn tail_with_frame(
        expr: KExpression<'a>,
        frame: Rc<CallArena>,
        function: &'a KFunction<'a>,
    ) -> Self {
        BodyResult::Tail { expr, frame: Some(frame), function: Some(function) }
    }

    pub fn err(e: KError) -> Self {
        BodyResult::Err(e)
    }
}

/// Builtin body. `for<'a>` so a single `fn` works for any caller scope lifetime;
/// `Scope` is `&'a` (not `&mut`) because every node spawned during the body shares it
/// — mutability is interior via `RefCell`.
pub type BuiltinFn = for<'a> fn(
    &'a Scope<'a>,
    &mut dyn SchedulerHandle<'a>,
    ArgumentBundle<'a>,
) -> BodyResult<'a>;

/// Dispatch-time name extractor for a binder builtin. `run_dispatch` calls it on the
/// unresolved expression *before* sub-deps are scheduled; returning `Some(name)` installs
/// `placeholders[name] = NodeId(this_slot)` in the dispatching scope so a sibling looking
/// up `name` while this slot's body is still in flight parks on this slot (see
/// [`crate::dispatch::runtime::Scope::resolve`]). `None` opts out.
pub type PreRunFn = for<'a> fn(&KExpression<'a>) -> Option<String>;

/// An enum (rather than `Box<dyn Fn>`) so the `UserDefined` case stays introspectable —
/// TCO and error-frame attribution both need to walk into the captured expression.
pub enum Body<'a> {
    Builtin(BuiltinFn),
    UserDefined(KExpression<'a>),
}

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
        let pairs = match super::values::parse_named_value_pairs(&tmp_expr, "function call") {
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
}
