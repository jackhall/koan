//! `KFunction` — the callable Koan function value. Carries an `ExpressionSignature`,
//! a `Body` (an action `fn` pointer or captured user-defined `KExpression`), and the
//! lexical scope captured at definition time.

use crate::machine::model::{ExpressionPart, WorkingExpression, WorkingPart};
use crate::source::{SourceRef, Spanned};

use crate::machine::core::kfunction::action::BoundArg;
use crate::machine::core::{KError, KErrorKind, Scope};
use crate::machine::model::DeliveredCarried;
use crate::machine::model::NamedPairs;
#[cfg(test)]
use crate::machine::model::SignatureDraft;
use crate::machine::model::{DeferredReturnSurface, KType, ReturnType, TypeNode};
use crate::machine::model::{ExpressionSignature, Record, SignatureElement, shape_type_of};
use crate::machine::model::{Unifier, UnifyFailure, Variance, admits_with};
use crate::memory::BumpVec;
use crate::memory::{Delivered, Opened, RegionHandleFamily, Sealed};
use crate::memory::{FoldingBrand, KoanStorageProfile, RegionBrand};

/// The scheduler-aware `Action` currency: the body shape every builtin returns, interpreted by
/// `machine::execute`'s `run_action`.
pub mod action;
pub mod block_tail;
pub mod body;
pub mod exec;
pub mod pick;

use crate::machine::model::RunRegistries;
use crate::machine::model::{Symbol, render_label};
pub use action::ActionFn;
pub use body::Body;
pub use pick::WrapIndices;
use pick::{carried_slot_ktype, slot_admits};

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
    ///
    /// A lambda type carries no binder, so every quantified position is erased to `Any` here: a
    /// `VAL` slot over a quantified callable keeps working, and a call by name solves the
    /// quantifiers at argument validation exactly as a dispatched call does.
    value_ktype: KType,
    /// The callable's **shape** type: the interleaved keyword / argument-position run its bucket
    /// key is read off, the quantifier group, and the return. Interned once beside `value_ktype`,
    /// from the same signature — the two are the callable's two identities, one per lane.
    shape_ktype: KType,
}

/// [`Reattachable`](crate::memory::Reattachable) family for [`KFunction`] — the carrier family a
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

crate::memory::reattachable! {
    KFunctionFamily => &'r KFunction<'r>,
}

impl<'a> FoldingBrand<'a> {
    /// Store a [`KFunction`] built at this fold's own brand — the door [`KFunction::alloc_captured`]
    /// is born through. One line over [`FoldingBrand::alloc_folded`], which carries the rank-2
    /// soundness argument: the callable is typed at the brand lifetime, so its captured-scope borrow
    /// is the fold's own operand view and an ambient-lifetime capture is a compile error here. A
    /// `KFunction` is `Copy`, its signature text already re-homed at this same region by
    /// [`ExpressionSignature::mint`](crate::machine::model::ExpressionSignature). Assembling the
    /// struct literal stays with this file, which owns the private fields; this door only stores.
    pub(crate) fn alloc_function_folded(self, f: KFunction<'a>) -> &'a KFunction<'a> {
        self.alloc_folded(f)
    }
}

/// A callable **in transit from its birth**: the merge-born `KFunction` carrier paired with the home
/// pin its birth composed. What [`KFunction::alloc_captured`] hands back and what every registration
/// door composes from — the seal ([`OverloadSeal::of_delivered`](crate::machine::core::OverloadSeal))
/// rests it, the `KObject` wrapper ([`Scope::store_function_cell`](Scope::store_function_cell))
/// merges it — so no door re-states the callable's reach on its own authority.
pub type DeliveredFunction = Delivered<KFunctionFamily>;

/// A callable's **dormant** carrier: the `KFunction` fused to the exact reach description its birth
/// composed for it, over the [`KFunctionFamily`] the library dispatches on. This is what a
/// `functions` dispatch bucket stores and what a
/// [`ReturnContract`](crate::machine::core::ReturnContract) carries across a tail chain: the seal
/// fuses the callable with its reach claim, where a bare `&KFunction` would state no reach at all.
pub type SealedFunction<'home> = Sealed<'home, KFunctionFamily>;

/// A callable **in use**: re-anchored at a region's own lifetime, paired with the reach witness it
/// was opened under. Dispatch resolves on one of these and carries it across argument evaluation
/// (`Resolved<'step>`); the escape into the call chain
/// [`reseal`](crate::memory::Opened::reseal)s it back to a [`SealedFunction`].
pub type OpenedFunction<'a> = Opened<'a, KFunctionFamily>;

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

/// [`Reattachable`](crate::memory::Reattachable) family for [`FunctionBirth`] — the seed operand
/// [`KFunction::alloc_captured`]'s merge folds, delivered resident at the captured scope so the
/// composition's source pins name exactly that region.
struct FunctionBirthFamily;

crate::memory::reattachable! {
    FunctionBirthFamily => FunctionBirth<'r>,
}

impl<'a> KFunction<'a> {
    /// **Build a function at its captured scope's region and store it there** — the privacy-gated
    /// door for a `KFunction`: `captured` and `value_ktype` are private fields, so a struct literal
    /// is unstateable outside this module, and the value never exists outside the region that owns
    /// its capture.
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
    /// [`merge_into`](crate::memory::Delivered::merge_into) whose destination is that same
    /// region's own handle. The fold's rank-2 brand is the residence proof — an ambient region
    /// borrow cannot inhabit `KFunction<'b>`, so the finished callable can borrow nothing but the
    /// fold's declared operands — and the merge *composes* the product's description from the seed's
    /// coverage: host the home region, home its one member. "A function borrows only the scope it
    /// captures" is therefore a fact the composition derived, not a claim a seal asserts, and a
    /// registration door composes from the envelope handed back rather than re-stating that reach.
    ///
    /// The signature is minted **before** the merge, at the captured scope's own brand: its text is
    /// re-homed into the very region the merge targets, so it enters as a region-resident operand
    /// alongside the scope, and the `Copy` [`ExpressionSignature`] rides in the seed where the
    /// growable elements buffer it was minted from could not.
    ///
    /// The store inside the fold is the plain bump verb
    /// ([`FoldingBrand::alloc_function_folded`]): a `KFunction` is `Copy`, so it lands in the region
    /// bump and region death frees it as a chunk with no destructor pass
    /// ([value-substrates.md § Untyped arenas](../../../design/value-substrates.md#untyped-arenas-the-drop-free-end-state)).
    pub fn alloc_captured(
        captured: &'a Scope<'a>,
        return_type: ReturnType<'a>,
        elements: &[SignatureElement],
        quantifiers: &[crate::machine::model::TypeSymbol],
        body: Body<'a>,
        registries: &RunRegistries,
    ) -> DeliveredFunction {
        let signature =
            ExpressionSignature::mint(captured.brand(), return_type, elements, quantifiers);
        let shape_ktype = function_shape_ktype(&signature, registries);
        let value_ktype = registries.types.erase_quantified(
            function_value_ktype(&signature, registries),
            quantifiers.len(),
        );
        Self::birth(captured, signature, body, value_ktype, shape_ktype)
    }

    /// **Assemble a copy of `source` captured at `captured`, at a relocation fold's own brand** —
    /// the environment copy's in-fold door. Where [`Self::alloc_captured_copy`] performs its own
    /// witnessed birth, this hands the struct back for the enclosing fold to store through
    /// [`FoldingBrand::alloc_function_folded`], whose rank-2 brand is the same residence proof: the
    /// callable is typed at the brand lifetime, so `captured` is the fold's own destination operand
    /// and an ambient-lifetime capture is a compile error at this signature.
    ///
    /// The signature is re-minted at `captured`'s brand for [`Self::alloc_captured_copy`]'s reason —
    /// a carried-over signature would leave the copy borrowing the source region — and
    /// `value_ktype` is copied, being a lifetime-free handle on the same `(params) -> ret` type.
    /// Assembling the struct stays here, where the private fields live; the fold stores.
    pub(crate) fn copy_at_fold(captured: &'a Scope<'a>, source: &KFunction<'a>) -> KFunction<'a> {
        KFunction {
            signature: ExpressionSignature::mint(
                captured.brand(),
                source.signature.return_type(),
                source.signature.elements(),
                source.signature.quantifiers(),
            ),
            body: source.body,
            captured,
            value_ktype: source.value_ktype,
            shape_ktype: source.shape_ktype,
        }
    }

    /// The witnessed birth both callable doors share: the three ingredients ride in as one resident
    /// seed and the callable is assembled inside a merge whose destination is the captured scope's
    /// own region handle. `signature` must already be minted at `captured`'s brand — that is what
    /// makes it a region-resident operand alongside the scope.
    fn birth(
        captured: &'a Scope<'a>,
        signature: ExpressionSignature<'a>,
        body: Body<'a>,
        value_ktype: KType,
        shape_ktype: KType,
    ) -> DeliveredFunction {
        let seed = FunctionBirth {
            captured,
            signature,
            body,
        };
        captured
            .deliver_resident::<FunctionBirthFamily>(seed)
            .merge_into::<RegionHandleFamily, KFunctionFamily, KoanStorageProfile>(
                captured.dest_operand(),
                |birth, _handle, placement| {
                    let door = FoldingBrand::in_fold_closure(placement);
                    door.alloc_function_folded(KFunction {
                        signature: birth.signature,
                        body: birth.body,
                        captured: birth.captured,
                        value_ktype,
                        shape_ktype,
                    })
                },
            )
    }

    /// Test door for [`Self::alloc_captured`] over a bundled [`SignatureDraft`]. A fixture spells its
    /// signature as a draft — the same `vec![]`-of-elements shape builtin registration uses — so the
    /// unbundling happens once here rather than at every call.
    #[cfg(test)]
    pub(crate) fn alloc_captured_draft(
        captured: &'a Scope<'a>,
        draft: SignatureDraft<'a>,
        body: Body<'a>,
        registries: &RunRegistries,
    ) -> DeliveredFunction {
        Self::alloc_captured(
            captured,
            draft.return_type,
            &draft.elements,
            &[],
            body,
            registries,
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
        registries: &RunRegistries,
    ) -> &'a KFunction<'a> {
        let cell = Self::alloc_captured_draft(captured, draft, body, registries);
        let sealed = cell.rest_into(captured.brand().handle());
        captured.open_function(&sealed).value()
    }

    /// This function value's type handle — a copy of the memo [`Self::alloc_captured`] interned.
    pub fn value_ktype(&self) -> KType {
        self.value_ktype
    }

    /// This callable's shape type — its dispatch-lane identity, the peer of
    /// [`value_ktype`](Self::value_ktype).
    pub fn shape_ktype(&self) -> KType {
        self.shape_ktype
    }

    /// The captured definition scope. Bare field read — the stored reference is already at `'a`.
    pub fn captured_scope(&self) -> &'a Scope<'a> {
        self.captured
    }

    /// Validate a positional call's `parts` against this signature: arity, keyword spellings, and
    /// each argument's type ([`slot_admits`]). Shared by [`Self::bind_args_into`] and the `exec`
    /// executor — the latter selects the call's delivery envelopes by `part_slots`, trusting the
    /// picker, so for a uniquely-picked call (admitted shape-only by dispatch) this is where a
    /// non-satisfying typed argument becomes a hard `TypeMismatch` rather than slipping through.
    /// It is also what makes that selection a 1:1 slot walk: parts and signature elements have
    /// equal length and matching shapes.
    ///
    /// This is also where a quantified signature is **solved**. One [`Unifier`] runs the whole
    /// argument walk in order, so the first position reaching a quantifier binds it from the type
    /// that argument carries and every later one must agree; the returned solution is what the
    /// call registers into its own scope and substitutes into its return. A signature quantifying
    /// over nothing walks the ordinary boolean path and returns an empty solution.
    pub(crate) fn validate_call_args(
        &'a self,
        parts: &[Spanned<WorkingPart<'a>>],
        registries: &RunRegistries,
    ) -> Result<Unifier, KError> {
        let quantifiers = self.signature.quantifiers();
        let mut unifier = Unifier::new(quantifiers.len());
        if self.signature.elements().len() != parts.len() {
            return Err(KError::new(KErrorKind::ArityMismatch {
                expected: self.signature.elements().len(),
                got: parts.len(),
            }));
        }
        for (el, part) in self.signature.elements().iter().zip(parts.iter()) {
            match el {
                SignatureElement::Keyword(s) => match part.value.as_ast() {
                    Some(ExpressionPart::Keyword(t)) if *s == t => {}
                    Some(ExpressionPart::Keyword(t)) => {
                        let (s, t) = (
                            registries.labels.display(s.symbol()),
                            registries.labels.display(t.symbol()),
                        );
                        return Err(KError::new(KErrorKind::DispatchFailed {
                            expr: summarize_parts(parts, registries),
                            reason: format!("expected keyword '{s}', got '{t}'"),
                            // A parts run carries spans but no file, and validation runs inside a
                            // call already being bound, so the enclosing frame locates this.
                            location: None,
                        }));
                    }
                    _ => {
                        let s = registries.labels.display(s.symbol());
                        return Err(KError::new(KErrorKind::DispatchFailed {
                            expr: summarize_parts(parts, registries),
                            reason: format!("expected keyword '{s}'"),
                            location: None,
                        }));
                    }
                },
                SignatureElement::Argument(arg) => {
                    let mismatch = || {
                        KError::new(KErrorKind::TypeMismatch {
                            arg: render_label(arg.name.symbol(), registries),
                            expected: arg.ktype.name_under(quantifiers, registries),
                            got: part.value.summarize(registries),
                        })
                    };
                    // A slot with nothing to solve answers by the ordinary admission. Only a slot
                    // whose declared type reads a quantifier needs the argument's carried type,
                    // and only then does the walk cost a unification.
                    if quantifiers.is_empty() || !registries.types.contains_quantified(arg.ktype) {
                        if !slot_admits(arg, &part.value, registries) {
                            return Err(mismatch());
                        }
                        continue;
                    }
                    let Some(carried) = carried_slot_ktype(&part.value, arg.ktype, registries)
                    else {
                        return Err(mismatch());
                    };
                    match admits_with(arg.ktype, carried, Variance::Co, &mut unifier, registries) {
                        Ok(()) => {}
                        Err(UnifyFailure::Mismatch) => return Err(mismatch()),
                        Err(UnifyFailure::Disagree { index, bound, got }) => {
                            return Err(KError::new(KErrorKind::TypeMismatch {
                                arg: render_label(arg.name.symbol(), registries),
                                expected: format!(
                                    "{}, already solved as `{}` by an earlier argument",
                                    render_label(quantifiers[index].symbol(), registries),
                                    bound.name(registries),
                                ),
                                got: got.name(registries),
                            }));
                        }
                    }
                }
            }
        }
        // Every quantifier the definition lists is read by some slot — the definition refuses one
        // that is not — so an unsolved cell means the argument that would have solved it carries
        // no type the call can read yet.
        for (index, name) in quantifiers.iter().enumerate() {
            if unifier.get(index).is_none() {
                return Err(KError::new(KErrorKind::TypeMismatch {
                    arg: render_label(name.symbol(), registries),
                    expected: "a type solved from the arguments".to_string(),
                    got: "nothing — no argument position at this call determines it".to_string(),
                }));
            }
        }
        Ok(unifier)
    }

    /// Bind a builtin call's positional argument `parts` into `slots`, one entry per declared
    /// parameter in declaration order — the values half of the argument view, aligned with the
    /// signature's own [`params`](crate::machine::model::ExpressionSignature::params) schema.
    ///
    /// Each argument is resolved against its declared parameter type by the slot-aware
    /// [`WorkingPart::resolve_for`], which lifts a resolved sub-result out of its cell and lowers a
    /// raw `Type` / `SigiledTypeExpr` / `RecordType` part into the matching [`Held`](crate::machine::model::Held) arm; its
    /// delivery envelope is read off `carriers` at the same part index. `scope` is the call scope:
    /// `resolve_for` adopts a spliced **cell** into it before owning the value, so an owned type
    /// that still borrows the producer region stays pinned.
    ///
    /// Nothing is keyed here, on either lane. A committed call's parts line up 1:1 with the
    /// signature's elements ([`Self::validate_call_args`] enforces it), so
    /// [`part_slots`](crate::machine::model::ExpressionSignature::part_slots) addresses each
    /// parameter's part positionally and the view's named reads resolve against the
    /// definition-time schema.
    pub fn bind_args_into<'c>(
        &'a self,
        parts: &[Spanned<WorkingPart<'a>>],
        scope: &'a Scope<'a>,
        registries: &RunRegistries,
        carriers: &'c [Option<DeliveredCarried>],
        slots: &mut BumpVec<'_, BoundArg<'a, 'c>>,
    ) -> Result<(), KError> {
        // A builtin quantifies over nothing, so the solution the walk returns is empty and there
        // is nothing to register: the validation is all this door wants from it.
        self.validate_call_args(parts, registries)?;
        for slot in self.signature.part_slots() {
            let at = *slot as usize;
            let SignatureElement::Argument(arg) = self.signature.elements()[at] else {
                unreachable!("part_slots indexes exactly the signature's argument elements");
            };
            slots.push(BoundArg {
                value: parts[at]
                    .value
                    .resolve_for(&arg.ktype, scope, &registries.types),
                carrier: carriers.get(at).and_then(Option::as_ref),
                surface: match parts[at].value {
                    WorkingPart::Spliced { from_name, .. } => from_name,
                    _ => None,
                },
            });
        }
        Ok(())
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
    /// bumped there as its own text. `site` is the invoked expression's own extent, carried onto
    /// the reconstruction so it reports the source it stands in for rather than as location-free.
    pub fn reconstruct_positional<'b>(
        &self,
        brand: RegionBrand<'b>,
        fields: Vec<(Symbol, ExpressionPart<'b>)>,
        site: Option<SourceRef>,
        registries: &RunRegistries,
    ) -> Result<WorkingExpression<'b>, KError> {
        let mut pairs = NamedPairs::from_fields(fields, registries)
            .map_err(|msg| KError::new(KErrorKind::ShapeError(msg)))?;
        // Every named slot is checked present before the run is reserved, so the fill below cannot
        // fail partway and the reconstruction builds straight into the region's bytes. Presence is
        // the whole of it: `take` consumes, but no two slots ever ask for the same name — a
        // signature declaring one twice is refused at its definition
        // (`fn_def::finalize::check_distinct_parameter_names`).
        if let Some(missing) = self.signature.elements().iter().find_map(|el| match el {
            SignatureElement::Argument(a) if !pairs.contains(a.name.symbol()) => Some(a.name),
            _ => None,
        }) {
            return Err(KError::new(KErrorKind::MissingArg(render_label(
                missing.symbol(),
                registries,
            ))));
        }
        Ok(WorkingExpression::build_from_iter(
            brand,
            self.signature.elements().iter().map(|el| {
                Spanned::bare(WorkingPart::Ast(match el {
                    SignatureElement::Keyword(symbol) => ExpressionPart::Keyword(*symbol),
                    SignatureElement::Argument(a) => pairs
                        .take(a.name.symbol())
                        .expect("every named slot checked satisfiable above"),
                }))
            }),
            site.map(|s| s.span),
            site.map(|s| s.file),
        ))
    }
}

/// Surface rendering of a call's parts for a diagnostic — the same text
/// [`WorkingExpression::summarize`] produces, from the parts run alone.
fn summarize_parts(parts: &[Spanned<WorkingPart<'_>>], registries: &RunRegistries) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    for (index, part) in parts.iter().enumerate() {
        if index > 0 {
            out.push(' ');
        }
        let _ = write!(out, "{}", part.value.summary(registries));
    }
    out
}

/// Intern the function type a `KFunction` value reports. The parameter record keys each
/// `Argument` by its declared name — the names the signature already holds, never the dispatch
/// keywords — so a function value projects the same `(name → type)` record a
/// `:(FN :{name :Type} -> _)` slot declares.
///
/// A `Deferred(_)` source return projects into the confined `DeferredReturn` node, holding the
/// hashable surface shadow of the deferred form, so equality and specificity read the deferred
/// shape directly instead of seeing it coarsened to `Any`. See
/// [ktype/records-and-limits.md § Record fields](../../../design/typing/ktype/records-and-limits.md#record-fields-and-ktype-hashing).
/// Intern the **shape** type a `KFunction` registers under: its signature read through the one
/// shape derivation [`shape_type_of`] owns, with the same return projection
/// [`function_value_ktype`] takes.
///
/// A keyword-free signature (the anonymous `FN :{…}` lambda) yields a keyword-free shape, which
/// no declared member can equal: a declared shape always spells at least one keyword, and shape
/// subtyping pairs keywords positionally. So an anonymous lambda fills no shape slot without this
/// door needing a second answer for it.
fn function_shape_ktype(signature: &ExpressionSignature<'_>, registries: &RunRegistries) -> KType {
    shape_type_of(
        signature.elements(),
        signature.quantifiers(),
        projected_return(signature, registries),
        registries,
    )
}

/// The return type both of a callable's two type identities carry: a resolved return verbatim, a
/// per-call-deferred one as the confined `DeferredReturn` node holding its surface shadow, so
/// equality and specificity read the deferred shape rather than seeing it coarsened to `Any`.
fn projected_return(signature: &ExpressionSignature<'_>, registries: &RunRegistries) -> KType {
    match signature.return_type() {
        ReturnType::Resolved(kt) => kt,
        ReturnType::Deferred(d) => registries.types.intern(TypeNode::DeferredReturn(
            DeferredReturnSurface::from_deferred(&d, &registries.labels),
        )),
    }
}

fn function_value_ktype(signature: &ExpressionSignature<'_>, registries: &RunRegistries) -> KType {
    let types = &registries.types;
    // The signature already owns its parameter schema; the function type shares it rather than
    // re-deriving one — one intern-boundary copy per definition, never per call.
    let params = Record::from_pairs(signature.params().iter().copied());
    let ret = projected_return(signature, registries);
    types.function_type(params, ret)
}

#[cfg(test)]
mod tests;
