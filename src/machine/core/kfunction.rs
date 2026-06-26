//! `KFunction` — the callable Koan function value. Carries an `ExpressionSignature`,
//! a `Body` (an action `fn` pointer or captured user-defined `KExpression`), and the
//! lexical scope captured at definition time.

use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::source::Spanned;

use crate::machine::core::scope_ptr::BoundedScopePtr;
use crate::machine::core::{KError, KErrorKind, KFuture, Scope};
use crate::machine::model::types::{ExpressionSignature, Parseable, Record, SignatureElement};
use crate::machine::model::values::{ArgValue, NamedPairs};
use crate::witnessed::reattachable;

/// The scheduler-aware `Action` currency: the body shape every builtin returns, interpreted by
/// `machine::execute::runtime::run_action`.
pub mod action;
pub mod bind_by_name;
pub mod body;
pub mod exec;
pub mod pick;

pub use crate::scheduler::NodeId;
pub use action::ActionFn;
pub use body::{BinderBucketFn, BinderNameFn, Body};
pub use pick::ClassifiedSlots;

/// SAFETY: the captured scope is allocated in a `KoanRegion` that outlives this
/// `KFunction` — they share the region (FN registers the function in the same scope it
/// captures; builtins are registered in run-root). See `core/arena.rs` for the broader
/// lifetime-erasure pattern.
pub struct KFunction<'a> {
    pub signature: ExpressionSignature<'a>,
    pub body: Body<'a>,
    /// The captured definition scope, a content-branded [`BoundedScopePtr<'a>`] (the same
    /// reader-bounded handle [`Scope::outer`] uses): the FN may be defined inside a per-call frame,
    /// so the capture borrow is frame-bounded while the scope *content* stays `'a`.
    /// [`Self::captured_scope`] re-hands it; the captured region's owner — needed at dispatch for the
    /// cycle-gate escape — is read off the scope itself ([`Scope::region_owner`]), so no separate
    /// handle rides here. Liveness past the defining frame rides the `Rc<CallFrame>` that lift
    /// attaches to an escaping closure.
    ///
    /// **Variance-load-bearing.** `BoundedScopePtr<'a>` carries `'a` structurally (`Scope<'a>` is
    /// invariant — it holds `RefCell`s), so `captured` keeps `KFunction<'a>` invariant in `'a`. Do
    /// **not** weaken the brand to a covariant carrier.
    captured: BoundedScopePtr<'a>,
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
}

/// `Reattachable` family for a `&KFunction` reference — re-anchors a resolved dispatch function from
/// a threaded scope's `'b` brand back to the cart `'step`. A reference is a thin pointer, layout
/// identical for every `'r`, so the shared `reattachable!` macro discharges the obligation once.
pub struct KFunctionRefFamily;
reattachable!(KFunctionRefFamily => &'r KFunction<'r>);

impl<'a> KFunction<'a> {
    pub fn new(signature: ExpressionSignature<'a>, body: Body<'a>, captured: &Scope<'a>) -> Self {
        Self::with_binder_name(signature, body, captured, None)
    }

    pub fn with_binder_name(
        signature: ExpressionSignature<'a>,
        body: Body<'a>,
        captured: &Scope<'a>,
        binder_name: Option<BinderNameFn>,
    ) -> Self {
        Self::with_binder_and_functor(signature, body, captured, binder_name, None, false)
    }

    pub fn with_binder_and_functor(
        mut signature: ExpressionSignature<'a>,
        body: Body<'a>,
        captured: &Scope<'a>,
        binder_name: Option<BinderNameFn>,
        binder_bucket: Option<BinderBucketFn>,
        is_functor: bool,
    ) -> Self {
        signature.normalize();
        Self {
            signature,
            body,
            captured: BoundedScopePtr::erase(captured),
            binder_name,
            binder_bucket,
            is_functor,
        }
    }

    /// Re-attach `'a` to the captured scope. The branded `captured` makes this a safe re-attach: it
    /// was erased from a `&'a Scope<'a>` in [`Self::with_binder_and_functor`], and points at a scope
    /// that outlives this `KFunction<'a>` by the broader runtime-region argument.
    pub fn captured_scope(&self) -> &Scope<'a> {
        self.captured.get()
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

    /// Validate a positional call `expr` against this signature: arity, keyword spellings, and each
    /// argument's type ([`Argument::matches`]). Shared by [`Self::bind`] and the `exec` executor —
    /// the latter binds via `bind_by_name` (a pure rename that trusts the picker), so for a
    /// uniquely-picked call (admitted shape-only by dispatch) this is where a non-satisfying typed
    /// argument becomes a hard `TypeMismatch` rather than slipping through.
    pub(crate) fn validate_call_args(&'a self, expr: &KExpression<'a>) -> Result<(), KError> {
        if self.signature.elements.len() != expr.parts.len() {
            return Err(KError::new(KErrorKind::ArityMismatch {
                expected: self.signature.elements.len(),
                got: expr.parts.len(),
            }));
        }
        for (el, part) in self.signature.elements.iter().zip(expr.parts.iter()) {
            match el {
                SignatureElement::Keyword(s) => match &part.value {
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
                    if !arg.matches(&part.value) {
                        return Err(KError::new(KErrorKind::TypeMismatch {
                            arg: arg.name.clone(),
                            expected: arg.ktype.name(),
                            got: part.value.summarize(),
                        }));
                    }
                }
            }
        }
        Ok(())
    }

    pub fn bind(&'a self, expr: KExpression<'a>) -> Result<KFuture<'a>, KError> {
        self.validate_call_args(&expr)?;
        let mut args: Record<ArgValue<'a>> = Record::new();
        for (el, part) in self.signature.elements.iter().zip(expr.parts.iter()) {
            if let SignatureElement::Argument(arg) = el {
                args.insert(arg.name.clone(), part.value.resolve_for(&arg.ktype));
            }
        }
        Ok(KFuture {
            parsed: expr,
            function: self,
            args,
        })
    }

    /// Reorder a call's named arguments (the `{name = value}` record literal's fields)
    /// into this signature's positional element order. Validation precedence (first
    /// wins): duplicate name (`ShapeError` from `NamedPairs::from_fields`) → missing arg
    /// (`MissingArg`). Width-drop semantics: a named arg with no matching declared
    /// parameter is ignored, not an error — this is the value side of function-subtyping
    /// width drop, where a value fills a slot that promised extra parameters and the
    /// surplus named args simply go unbound on the reconstructed exact-arity expression.
    /// `NamedPairs` rejects duplicate names, so consuming every declared argument
    /// witnesses an exact-arity reconstruction regardless of leftover (now-dropped) names.
    pub fn reconstruct_positional<'b>(
        &self,
        fields: Vec<(String, ExpressionPart<'b>)>,
    ) -> Result<KExpression<'b>, KError> {
        let mut pairs = NamedPairs::from_fields(fields)
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
        // Leftover named args (no matching declared param) are dropped, not rejected:
        // call-by-name width drop.
        Ok(KExpression::new(parts))
    }
}

#[cfg(test)]
mod tests;
