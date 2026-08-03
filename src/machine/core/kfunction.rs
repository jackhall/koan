//! `KFunction` — the callable Koan function value. Carries an `ExpressionSignature`,
//! a `Body` (an action `fn` pointer or captured user-defined `KExpression`), and the
//! lexical scope captured at definition time.

use crate::machine::model::{ExpressionPart, WorkingExpression, WorkingPart};
use crate::source::Spanned;

use crate::machine::core::ref_carriers::ScopeRefFamily;
use crate::machine::core::{KError, KErrorKind, RegionBrand, Scope};
use crate::machine::model::{DeferredReturnSurface, KType, ReturnType, TypeNode, TypeRegistry};
use crate::machine::model::{
    ExpressionSignature, ExpressionSignatureFamily, Record, SignatureElement,
};
use crate::machine::model::{Held, NamedPairs};
use crate::witnessed::{And, SealedExtern};

/// The scheduler-aware `Action` currency: the body shape every builtin returns, interpreted by
/// `machine::execute::runtime::run_action`.
pub mod action;
pub mod bind_by_name;
pub mod block_tail;
pub mod body;
pub mod exec;
pub mod pick;

pub use crate::scheduler::NodeId;
pub use action::ActionFn;
pub use body::{Body, BodyFamily};
use pick::slot_admits;
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
    /// [`Region::alloc_resident`](crate::witnessed::Region)), so the embedded reference re-anchors with it and
    /// [`Self::captured_scope`] is a bare field read. The captured region's owner is read off the
    /// scope itself ([`Scope::region_owner`]); when the closure escapes, the consumer frame retains
    /// that region in its witness set.
    ///
    /// **Variance-load-bearing.** `&'a Scope<'a>` is invariant in `'a` (`Scope<'a>` holds `RefCell`s),
    /// so `captured` keeps `KFunction<'a>` invariant in `'a`.
    captured: &'a Scope<'a>,
    /// True for binder-introducing builtins (LET, VAL, FN, OP, TYPE, MODULE, SIG, UNION, NEWTYPE,
    /// GROUP, RECURSIVE TYPES). The structural detail of *what* a binder declares — the name or the
    /// inner-call bucket key it installs, and which of its slots carry nested binders forward — lives
    /// once in [`crate::machine::model::binder`] (the [`BINDER_SPECS`](crate::machine::model::binder::BINDER_SPECS)
    /// table), keyed by untyped signature shape. This flag is only the classification bit dispatch
    /// reads (a binder's literal-name slots are declarations, not references, so they must not
    /// replay-park on their own placeholder); the spec⟺registration consistency test pins the flag
    /// against the table. User `FN` construction and user `OP` registration are not binder builtins,
    /// so they carry `false`.
    pub binder: bool,
    /// The function *value*'s own type: the `(params) -> ret` handle interned once, here at
    /// definition, from the normalized signature. `KObject::KFunction(f).ktype()` copies it, so
    /// the value layer never rebuilds a parameter record per dispatch check (ruling 4).
    value_ktype: KType,
}

/// [`Reattachable`](crate::witnessed::Reattachable) family for [`KFunction`] — the carrier family a
/// function value travels under when it flows through the three witnessed-carrier states as
/// `Sealed<KFunctionFamily, _>` / `Opened<'step, KFunctionFamily, _>`, the function-table twin of
/// [`CarriedFamily`](crate::machine::model::CarriedFamily). Registered here rather than adding a
/// `Carried::Function` variant because the witnessed library is generic over `Reattachable`
/// families.
///
/// A carried function travels as `&'r KFunction<'r>` — a thin reference whose layout does not depend
/// on `'r`; `KFunction<'r>` itself is generic only in `'r` (its fields are an
/// `ExpressionSignature<'r>`, a `Body<'r>`, a `&'r Scope<'r>`, a `bool`, and a lifetime-free
/// `KType`), so every choice of `'r` is one type up to the lifetime and the shared `reattachable!`
/// macro discharges the layout-invariance obligation once.
// Phase 3 consumes it (the `functions` table entries become `Sealed<KFunctionFamily, _>` and
// dispatch resolves on `Opened<'step, KFunctionFamily>`); Phase 1 only registers the family.
#[allow(dead_code)]
pub struct KFunctionFamily;

crate::witnessed::reattachable! {
    KFunctionFamily => &'r KFunction<'r>,
}

impl<'a> KFunction<'a> {
    /// **Build a function at its captured scope's region and store it there** — the sole door for a
    /// `KFunction`, and the reason the value never exists outside the region that owns its capture.
    ///
    /// The destination is derived from `captured`'s own brand rather than passed alongside it, so
    /// pairing a function with a region other than its captured scope's is unrepresentable. The
    /// signature, the body and the scope all cross into a single `for<'b>` construction brand
    /// ([`RegionHandle::alloc_resident_born_with`](crate::witnessed::RegionHandle)), where the private
    /// `new` assembles them and the store happens in the same act — residence discharged by the
    /// brand, with no runtime check.
    ///
    /// `types` and `binder` carry no region borrow, so they are read straight from the enclosing
    /// frame inside the closure; `types` is consulted during construction and never stored.
    pub fn alloc_captured(
        captured: &'a Scope<'a>,
        signature: ExpressionSignature<'a>,
        body: Body<'a>,
        binder: bool,
        types: &TypeRegistry,
    ) -> &'a KFunction<'a> {
        Self::alloc_captured_sealed(
            captured.brand(),
            SealedExtern::<ScopeRefFamily>::erase(captured),
            captured.region(),
            signature,
            body,
            binder,
            types,
        )
    }

    /// [`Self::alloc_captured`] over a captured scope that arrives **sealed**, with the destination
    /// brand and the pin named separately — the shape a holder takes when the scope is reachable only
    /// through a carrier ([`CallFrame::scope_sealed`](crate::machine::CallFrame::scope_sealed)) rather
    /// than as a live borrow.
    ///
    /// Crate-internal, and [`Self::alloc_captured`] is the only production route through it: there,
    /// all three arguments are read off the one scope, so no pairing is stateable. A caller reaching
    /// this directly owes what the signature no longer says — `brand`'s region must be the one
    /// `captured`'s scope lives in, and `pin` must keep that region alive for the whole of `'a`.
    pub(crate) fn alloc_captured_sealed(
        brand: RegionBrand<'a>,
        captured: SealedExtern<ScopeRefFamily>,
        pin: &'a impl crate::witnessed::Witness,
        signature: ExpressionSignature<'a>,
        body: Body<'a>,
        binder: bool,
        types: &TypeRegistry,
    ) -> &'a KFunction<'a> {
        let operands = captured.zip(
            SealedExtern::<ExpressionSignatureFamily>::erase(signature).zip(SealedExtern::<
                BodyFamily,
            >::erase(
                body
            )),
        );
        brand
            .handle()
            .alloc_resident_born_with::<
                KFunction<'static>,
                And<ScopeRefFamily, And<ExpressionSignatureFamily, BodyFamily>>,
                _,
            >(
                operands,
                pin,
                |_placement, (captured_b, (signature_b, body_b))| {
                    KFunction::new(signature_b, body_b, captured_b, binder, types)
                },
            )
    }

    fn new(
        mut signature: ExpressionSignature<'a>,
        body: Body<'a>,
        captured: &'a Scope<'a>,
        binder: bool,
        types: &TypeRegistry,
    ) -> Self {
        signature.normalize();
        let value_ktype = function_value_ktype(&signature, types);
        Self {
            signature,
            body,
            captured,
            binder,
            value_ktype,
        }
    }

    /// This function value's type handle — a copy of the memo [`Self::new`] interned.
    pub fn value_ktype(&self) -> KType {
        self.value_ktype
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

    /// Validate a positional call's `parts` against this signature: arity, keyword spellings, and
    /// each argument's type ([`slot_admits`]). Shared by [`Self::bind_args`] and the `exec`
    /// executor — the latter binds via `bind_by_name` (a pure rename that trusts the picker), so for
    /// a uniquely-picked call (admitted shape-only by dispatch) this is where a non-satisfying typed
    /// argument becomes a hard `TypeMismatch` rather than slipping through.
    pub(crate) fn validate_call_args(
        &'a self,
        parts: &[Spanned<WorkingPart<'a>>],
        types: &TypeRegistry,
    ) -> Result<(), KError> {
        if self.signature.elements.len() != parts.len() {
            return Err(KError::new(KErrorKind::ArityMismatch {
                expected: self.signature.elements.len(),
                got: parts.len(),
            }));
        }
        for (el, part) in self.signature.elements.iter().zip(parts.iter()) {
            match el {
                SignatureElement::Keyword(s) => match part.value.as_ast() {
                    Some(ExpressionPart::Keyword(t)) if s == t => {}
                    Some(ExpressionPart::Keyword(t)) => {
                        return Err(KError::new(KErrorKind::DispatchFailed {
                            expr: summarize_parts(parts),
                            reason: format!("expected keyword '{s}', got '{t}'"),
                        }));
                    }
                    _ => {
                        return Err(KError::new(KErrorKind::DispatchFailed {
                            expr: summarize_parts(parts),
                            reason: format!("expected keyword '{s}'"),
                        }));
                    }
                },
                SignatureElement::Argument(arg) => {
                    if !slot_admits(arg, &part.value, types) {
                        return Err(KError::new(KErrorKind::TypeMismatch {
                            arg: arg.name.clone(),
                            expected: arg.ktype.name(types),
                            got: part.value.summarize(),
                        }));
                    }
                }
            }
        }
        Ok(())
    }

    /// Bind a builtin call's positional argument `parts` to this signature's parameters, producing
    /// the owned argument record [`Record<Held>`] directly. Each argument is resolved against its
    /// declared parameter type by the slot-aware [`WorkingPart::resolve_for`], which lifts a
    /// resolved sub-result out of its cell and lowers a raw `Type` / `SigiledTypeExpr` /
    /// `RecordType` part into the matching [`Held`] arm.
    ///
    /// This is the builtin counterpart to [`Self::bind_by_name`] (the user-defined-call binder).
    /// The two hold *different currencies for a reason*: this binder produces owned `Held` cells
    /// because a builtin receives raw argument parts that `resolve_for` resolves into fresh values;
    /// `bind_by_name` produces borrowed `Record<Carried>` because a user-defined call arrives with
    /// its value parts already resolved into `Carried` by dispatch, so it is a trusted rename of
    /// existing region values. `scope` is the call scope: `resolve_for` adopts a spliced **cell**
    /// into it before owning the value, so an owned type that still borrows the producer region
    /// stays pinned.
    pub fn bind_args(
        &'a self,
        parts: &[Spanned<WorkingPart<'a>>],
        scope: &'a Scope<'a>,
        types: &TypeRegistry,
    ) -> Result<Record<Held<'a>>, KError> {
        self.validate_call_args(parts, types)?;
        let mut args: Record<Held<'a>> = Record::new();
        for (el, part) in self.signature.elements.iter().zip(parts.iter()) {
            if let SignatureElement::Argument(arg) = el {
                args.insert(
                    arg.name.clone(),
                    part.value.resolve_for(&arg.ktype, scope, types),
                );
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
    ///
    /// The reconstruction is the scheduler's own node: it goes straight to the eager-subs staging
    /// that dispatches the call, so it is built as a [`WorkingExpression`] in `brand`'s region, with
    /// each supplied field riding through as a [`WorkingPart::Ast`] slot and each signature keyword
    /// bumped there as its own text.
    pub fn reconstruct_positional<'b>(
        &self,
        brand: RegionBrand<'b>,
        fields: Vec<(String, ExpressionPart<'b>)>,
    ) -> Result<WorkingExpression<'b>, KError> {
        let mut pairs = NamedPairs::from_fields(fields)
            .map_err(|msg| KError::new(KErrorKind::ShapeError(msg)))?;
        let mut parts: Vec<Spanned<WorkingPart<'b>>> =
            Vec::with_capacity(self.signature.elements.len());
        for el in &self.signature.elements {
            match el {
                SignatureElement::Keyword(s) => parts.push(Spanned::bare(WorkingPart::Ast(
                    ExpressionPart::Keyword(brand.alloc_text(s)),
                ))),
                SignatureElement::Argument(a) => match pairs.take(&a.name) {
                    Some(v) => parts.push(Spanned::bare(WorkingPart::Ast(v))),
                    None => {
                        return Err(KError::new(KErrorKind::MissingArg(a.name.clone())));
                    }
                },
            }
        }
        Ok(WorkingExpression::new(brand, parts))
    }
}

/// Surface rendering of a call's parts for a diagnostic — the same text
/// [`WorkingExpression::summarize`] produces, from the parts run alone.
fn summarize_parts(parts: &[Spanned<WorkingPart<'_>>]) -> String {
    parts
        .iter()
        .map(|part| part.value.summarize())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Intern the function type a `KFunction` value reports. The parameter record keys each
/// `Argument` by its declared name — the names the signature already holds, never the dispatch
/// keywords — so a function value projects the same `(name → type)` record a
/// `:(FN (name :Type) -> _)` slot declares.
///
/// A `Deferred(_)` source return projects into the confined `DeferredReturn` node, holding the
/// hashable surface shadow of the deferred form, so equality and specificity read the deferred
/// shape directly instead of seeing it coarsened to `Any`. See
/// [ktype/records-and-limits.md § Record fields](../../../design/typing/ktype/records-and-limits.md#record-fields-and-ktype-hashing).
fn function_value_ktype(signature: &ExpressionSignature<'_>, types: &TypeRegistry) -> KType {
    let params: Record<KType> = signature
        .elements
        .iter()
        .filter_map(|el| match el {
            SignatureElement::Argument(a) => Some((a.name.clone(), a.ktype)),
            _ => None,
        })
        .collect();
    let ret = match &signature.return_type {
        ReturnType::Resolved(kt) => *kt,
        ReturnType::Deferred(d) => types.intern(TypeNode::DeferredReturn(
            DeferredReturnSurface::from_deferred(d),
        )),
    };
    types.function_type(params, ret)
}

#[cfg(test)]
mod tests;
