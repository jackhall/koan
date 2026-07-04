//! `KFunction` — the callable Koan function value. Carries an `ExpressionSignature`,
//! a `Body` (an action `fn` pointer or captured user-defined `KExpression`), and the
//! lexical scope captured at definition time.

use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::source::Spanned;

use crate::machine::core::{BindKind, KError, KErrorKind, Scope};
use crate::machine::model::types::{ExpressionSignature, Parseable, Record, SignatureElement};
use crate::machine::model::values::{Held, NamedPairs};

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
    /// The captured definition scope, held as a plain `&'a Scope<'a>`. The holder re-anchors to `'a`
    /// as a whole when read out of its region (the substrate retype in
    /// [`Region::alloc`](crate::witnessed::Region)), so the embedded reference re-anchors with it and
    /// [`Self::captured_scope`] is a bare field read. The captured region's owner is read off the
    /// scope itself ([`Scope::region_owner`]); when the closure escapes, the consumer frame retains
    /// that region in its witness set.
    ///
    /// **Variance-load-bearing.** `&'a Scope<'a>` is invariant in `'a` (`Scope<'a>` holds `RefCell`s),
    /// so `captured` keeps `KFunction<'a>` invariant in `'a`.
    captured: &'a Scope<'a>,
    /// `Some((extractor, kind))` for name-binding declarators (LET, VAL, UNION, SIG,
    /// MODULE, NEWTYPE, RECURSIVE). `extractor` pulls the bound name out of the binder
    /// expression; `kind` records whether the binding lands in the value or the type
    /// language, so the forward-reference placeholder the dispatch driver installs is
    /// tagged and a value bind never satisfies a type placeholder (or the reverse). FN /
    /// FUNCTOR carry `binder_bucket` instead and install no name placeholder.
    pub binder_name: Option<(BinderNameFn, BindKind)>,
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

impl<'a> KFunction<'a> {
    pub fn new(
        mut signature: ExpressionSignature<'a>,
        body: Body<'a>,
        captured: &'a Scope<'a>,
        binder_name: Option<(BinderNameFn, BindKind)>,
        binder_bucket: Option<BinderBucketFn>,
        is_functor: bool,
    ) -> Self {
        signature.normalize();
        Self {
            signature,
            body,
            captured,
            binder_name,
            binder_bucket,
            is_functor,
        }
    }

    /// The captured definition scope. Bare field read — the holder was already re-anchored to `'a`
    /// when read out of its region.
    pub fn captured_scope(&self) -> &'a Scope<'a> {
        self.captured
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
    /// argument's type ([`Argument::matches`]). Shared by [`Self::bind_args`] and the `exec` executor —
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

    /// Bind a builtin call's positional arguments to this signature's parameters, producing the
    /// owned argument record [`Record<Held>`] directly. Each argument is resolved against its
    /// declared parameter type by the slot-aware [`ExpressionPart::resolve_for`], which lowers a raw
    /// `Type` / `SigiledTypeExpr` / `RecordType` part into the matching [`Held`] arm.
    ///
    /// This is the builtin counterpart to [`Self::bind_by_name`] (the user-defined-call binder).
    /// The two hold *different currencies for a reason*: this binder produces owned `Held` cells
    /// because a builtin receives raw un-`Spliced` argument parts that `resolve_for` resolves into
    /// fresh values; `bind_by_name` produces borrowed `Record<Carried>` because a user-defined call
    /// arrives with its value parts already resolved into `Carried` by dispatch, so it is a trusted
    /// rename of existing region values. `scope` is the call scope: `resolve_for` adopts a spliced
    /// **cell** into it before owning the value, so an owned type that still borrows the producer
    /// region stays pinned.
    pub fn bind_args(
        &'a self,
        expr: &KExpression<'a>,
        scope: &'a Scope<'a>,
    ) -> Result<Record<Held<'a>>, KError> {
        self.validate_call_args(expr)?;
        let mut args: Record<Held<'a>> = Record::new();
        for (el, part) in self.signature.elements.iter().zip(expr.parts.iter()) {
            if let SignatureElement::Argument(arg) = el {
                args.insert(arg.name.clone(), part.value.resolve_for(&arg.ktype, scope));
            }
        }
        Ok(args)
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
        Ok(KExpression::new(parts))
    }
}

#[cfg(test)]
mod tests;
