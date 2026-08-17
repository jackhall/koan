//! `KFunction` — the callable Koan function value. Carries an `ExpressionSignature`,
//! a `Body` (an action `fn` pointer or captured user-defined `KExpression`), and the
//! lexical scope captured at definition time.

use crate::machine::model::{ExpressionPart, WorkingExpression, WorkingPart};
use crate::source::Spanned;

use crate::machine::core::carrier_witness::DeliveredFunction;
use crate::machine::core::{
    FoldingBrand, KError, KErrorKind, KoanStorageProfile, RegionBrand, Scope,
};
use crate::machine::model::{DeferredReturnSurface, KType, ReturnType, TypeNode, TypeRegistry};
use crate::machine::model::{ExpressionSignature, Record, SignatureDraft, SignatureElement};
use crate::machine::model::{Held, NamedPairs};
use crate::witnessed::RegionHandleFamily;

/// The scheduler-aware `Action` currency: the body shape every builtin returns, interpreted by
/// `machine::execute`'s `run_action`.
pub mod action;
pub mod block_tail;
pub mod body;
pub mod exec;
pub mod pick;

pub use action::ActionFn;
pub use body::Body;
pub use pick::ClassifiedSlots;
use pick::slot_admits;

/// The captured scope is allocated in the same `KoanRegion` this `KFunction` lives in —
/// [`Self::alloc_captured`] derives the destination brand from the scope, so the two cannot come
/// apart. Every field is `Copy` and `Drop`-free, which is what puts the value in the region bump
/// rather than a lifetime-typed cell.
#[derive(Clone, Copy)]
pub struct KFunction<'a> {
    pub signature: ExpressionSignature<'a>,
    pub body: Body<'a>,
    /// The captured definition scope, held as a plain `&'a Scope<'a>` into the very region this
    /// function lives in, so [`Self::captured_scope`] is a bare field read and nothing is retyped on
    /// the way out. The captured region's owner is read off the scope itself
    /// ([`Scope::region_owner`]); when the closure escapes, the consumer frame retains that region in
    /// its witness set.
    ///
    /// **Variance-load-bearing.** `&'a Scope<'a>` is invariant in `'a` (`Scope<'a>` holds `RefCell`s),
    /// so `captured` keeps `KFunction<'a>` invariant in `'a`.
    captured: &'a Scope<'a>,
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
pub struct KFunctionFamily;

crate::witnessed::reattachable! {
    KFunctionFamily => &'r KFunction<'r>,
}

/// The birth operands that cross the merge brand together: the captured scope — the callable's one
/// region borrow — the signature already minted into that same region, and the body. All `Copy`, and
/// the layout is a thin reference plus two `Copy` handles, independent of `'r`, which is what the
/// shared `reattachable!` macro's layout-invariance obligation names.
#[derive(Clone, Copy)]
struct FunctionBirth<'r> {
    captured: &'r Scope<'r>,
    signature: ExpressionSignature<'r>,
    body: Body<'r>,
}

/// [`Reattachable`](crate::witnessed::Reattachable) family for [`FunctionBirth`] — the seed operand
/// [`KFunction::alloc_captured`]'s merge folds, delivered resident at the captured scope so the
/// composition's source pins name exactly that region.
struct FunctionBirthFamily;

crate::witnessed::reattachable! {
    FunctionBirthFamily => FunctionBirth<'r>,
}

impl<'a> KFunction<'a> {
    /// **Build a function at its captured scope's region and store it there** — the sole door for a
    /// `KFunction`, and the reason the value never exists outside the region that owns its capture.
    ///
    /// The destination is derived from `captured`'s own brand rather than passed alongside it, so
    /// pairing a function with a region other than its captured scope's is unstateable. The `draft`'s
    /// signature text may borrow from anywhere at `'a` — a builtin's `&'static` literal, a
    /// program-storage AST part — because [`ExpressionSignature::mint`] re-homes every name at that
    /// same brand before the value is assembled.
    ///
    /// **A witnessed birth.** The three ingredients ride in as one resident seed
    /// ([`FunctionBirth`], delivered at the captured scope, so the source operand's pins are exactly
    /// that region) and the callable is assembled *inside* a
    /// [`merge_into`](crate::witnessed::Delivered::merge_into) whose destination is that same
    /// region's own handle. The fold's rank-2 brand is the residence proof — an ambient region
    /// borrow cannot inhabit `KFunction<'b>`, so the finished callable can borrow nothing but the
    /// fold's declared operands — and the merge *composes* the product's description from the seed's
    /// coverage: host the home region, home its one member. "A function borrows only the scope it
    /// captures" is therefore a fact the composition derived, not a claim a seal asserts, and the
    /// envelope handed back is what every registration door composes from
    /// ([`OverloadSeal::of_delivered`](crate::machine::core::carrier_witness::OverloadSeal),
    /// [`Scope::store_function_cell`]).
    ///
    /// The signature is minted **before** the merge, at the captured scope's own brand: its text is
    /// re-homed into the very region the merge targets, so it enters as a region-resident operand
    /// alongside the scope, and the `Copy` [`ExpressionSignature`] rides in the seed where the
    /// `Vec`-holding [`SignatureDraft`] could not.
    ///
    /// The store inside the fold is the plain bump verb
    /// ([`FoldingBrand::alloc_function_folded`]): a `KFunction` is `Copy`, so it lands in the region
    /// bump and region death frees it as a chunk with no destructor pass
    /// ([value-substrates.md § Untyped arenas](../../../design/value-substrates.md#untyped-arenas-the-drop-free-end-state)).
    pub fn alloc_captured(
        captured: &'a Scope<'a>,
        draft: SignatureDraft<'a>,
        body: Body<'a>,
        types: &TypeRegistry,
    ) -> DeliveredFunction {
        let brand = captured.brand();
        let signature = ExpressionSignature::mint(brand, draft);
        let value_ktype = function_value_ktype(&signature, types);
        let seed = FunctionBirth {
            captured,
            signature,
            body,
        };
        captured
            .deliver_resident::<FunctionBirthFamily>(seed)
            .merge_into::<RegionHandleFamily<KoanStorageProfile>, KFunctionFamily, KoanStorageProfile>(
                captured.dest_operand(),
                |birth, _handle, placement| {
                    let door = FoldingBrand::in_fold_closure(placement);
                    door.alloc_function_folded(KFunction {
                        signature: birth.signature,
                        body: birth.body,
                        captured: birth.captured,
                        value_ktype,
                    })
                },
            )
    }

    /// Test fixture: the witnessed birth [`Self::alloc_captured`] performs, rested into the captured
    /// scope and re-opened there, so a suite can hold the callable at `'a` directly. A resident
    /// value rests for free — the library's self rule strips its own region from what is retained —
    /// so this adds no coverage the birth did not already compose. Production keeps the envelope and
    /// feeds both registration doors from it.
    #[cfg(test)]
    pub(crate) fn alloc_captured_for_test(
        captured: &'a Scope<'a>,
        draft: SignatureDraft<'a>,
        body: Body<'a>,
        types: &TypeRegistry,
    ) -> &'a KFunction<'a> {
        let cell = Self::alloc_captured(captured, draft, body, types);
        let sealed = cell.rest_into(captured.brand().handle());
        captured.open_function(&sealed).value()
    }

    /// This function value's type handle — a copy of the memo [`Self::alloc_captured`] interned.
    pub fn value_ktype(&self) -> KType {
        self.value_ktype
    }

    /// The captured definition scope. Bare field read — the stored reference is already at `'a`.
    pub fn captured_scope(&self) -> &'a Scope<'a> {
        self.captured
    }

    pub fn summarize(&self) -> String {
        let parts: Vec<String> = self
            .signature
            .elements()
            .iter()
            .map(|el| match el {
                SignatureElement::Keyword(s) => (*s).to_string(),
                SignatureElement::Argument(arg) => format!("<{}>", arg.name),
            })
            .collect();
        format!("fn({})", parts.join(" "))
    }

    /// Validate a positional call's `parts` against this signature: arity, keyword spellings, and
    /// each argument's type ([`slot_admits`]). Shared by [`Self::bind_args`] and the `exec`
    /// executor — the latter re-keys the call's delivery envelopes onto parameter names by slot
    /// (`map_arg_carriers`, a pure rename that trusts the picker), so for a uniquely-picked call
    /// (admitted shape-only by dispatch) this is where a non-satisfying typed argument becomes a
    /// hard `TypeMismatch` rather than slipping through. It is also what makes that re-key a
    /// 1:1 slot walk: parts and signature elements have equal length and matching shapes.
    pub(crate) fn validate_call_args(
        &'a self,
        parts: &[Spanned<WorkingPart<'a>>],
        types: &TypeRegistry,
    ) -> Result<(), KError> {
        if self.signature.elements().len() != parts.len() {
            return Err(KError::new(KErrorKind::ArityMismatch {
                expected: self.signature.elements().len(),
                got: parts.len(),
            }));
        }
        for (el, part) in self.signature.elements().iter().zip(parts.iter()) {
            match el {
                SignatureElement::Keyword(s) => match part.value.as_ast() {
                    Some(ExpressionPart::Keyword(t)) if s == &t => {}
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
                            arg: arg.name.to_string(),
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
    /// This is the builtin counterpart to the user-defined-call binder (`map_arg_carriers`, in the
    /// dispatcher's `exec` lane). The two hold *different currencies for a reason*: this binder
    /// produces owned `Held` cells because a builtin receives raw argument parts that `resolve_for`
    /// resolves into fresh values; the user-defined lane re-keys the call's **delivery envelopes**
    /// onto parameter names, because a user-defined call arrives with its value parts already
    /// delivered by dispatch and the frame bind relocates each one through its own envelope. `scope`
    /// is the call scope: `resolve_for` adopts a spliced **cell** into it before owning the value,
    /// so an owned type that still borrows the producer region stays pinned.
    pub fn bind_args(
        &'a self,
        parts: &[Spanned<WorkingPart<'a>>],
        scope: &'a Scope<'a>,
        types: &TypeRegistry,
    ) -> Result<Record<Held<'a>>, KError> {
        self.validate_call_args(parts, types)?;
        let mut args: Record<Held<'a>> = Record::new();
        for (el, part) in self.signature.elements().iter().zip(parts.iter()) {
            if let SignatureElement::Argument(arg) = el {
                args.insert(
                    arg.name.to_string(),
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
            Vec::with_capacity(self.signature.elements().len());
        for el in self.signature.elements() {
            match el {
                SignatureElement::Keyword(s) => parts.push(Spanned::bare(WorkingPart::Ast(
                    ExpressionPart::Keyword(brand.allocator().text(s)),
                ))),
                SignatureElement::Argument(a) => match pairs.take(a.name) {
                    Some(v) => parts.push(Spanned::bare(WorkingPart::Ast(v))),
                    None => {
                        return Err(KError::new(KErrorKind::MissingArg(a.name.to_string())));
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
        .elements()
        .iter()
        .filter_map(|el| match el {
            SignatureElement::Argument(a) => Some((a.name.to_string(), a.ktype)),
            _ => None,
        })
        .collect();
    let ret = match signature.return_type() {
        ReturnType::Resolved(kt) => kt,
        ReturnType::Deferred(d) => types.intern(TypeNode::DeferredReturn(
            DeferredReturnSurface::from_deferred(&d),
        )),
    };
    types.function_type(params, ret)
}

#[cfg(test)]
mod tests;
