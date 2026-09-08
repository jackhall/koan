//! Post-classification side of FN-def: turn the (return-type, parameter-list)
//! pair into either a synchronous `finalize_fn_with_kind` call or a deferred
//! schedule, and own the dep-finish closure.
//!
//! [`classify`] collapses the 8-combinatoric `(ReturnTypeState × ParamListResult)`
//! decision tree to an [`FnPlan`] with two terminal shapes, so the caller in
//! `super::fn_def` reduces to a two-arm match.
//!
//! The keyworded and anonymous FN binders ride the same path, selected by the
//! [`FnKind`] threaded through `finalize_fn_with_kind` / `defer`.

use crate::machine::Action;
use crate::machine::KFunction;
use crate::machine::ProducerId;
use crate::machine::StepCarried;
use crate::machine::core::bindings::WriteOp;
use crate::machine::execute::deps_on;
use crate::machine::model::Carried;
use crate::machine::model::CarriedFamily;
use crate::machine::model::KExpression;
use crate::machine::model::KType;
use crate::machine::model::labels::TypeSymbol;
use crate::machine::model::{Elaborator, ReturnType};
use crate::machine::model::{SignatureElement, shape_type_of};
use crate::machine::{BindingIndex, Body, CarrierWitness, KError, KErrorKind, Scope};
use crate::witnessed::{BumpAllocator, BumpVec, Witnessed};

use super::return_type::{
    ReturnTypeCapture, ReturnTypeState, make_capture, resolve_capture_at_finish,
};
use super::signature::{ParamListOutcome, Wildcards, parse_fn_param_list};
use crate::machine::OverloadSeal;
use crate::machine::model::RunRegistries;
use crate::machine::model::{display_label, render_label};

/// What a finalize hands back: the callable's own witnessed carrier, and the at-most-two binding
/// writes its [`FnKind`] calls for — a keyworded FN's overload registration, and the combined
/// form's value binding.
type FinalizedFn<'a> = (
    Witnessed<CarriedFamily, CarrierWitness>,
    [Option<WriteOp<'a>>; 2],
);

/// How a finalized FN-def is wired into the scope:
///
/// - `Function` — a keyworded FN registers under its lead keyword. `bound_name` is `Some` for the
///   combined `LET <name> = FN …` statement, which additionally binds the callable under that
///   value name; the two writes describe one `KFunction` at one `BindingIndex`.
/// - `Anonymous` — a record-schema binder (`FN :{…}`) has no keyword, so it
///   registers nothing; the value it evaluates to is its only handle.
/// - `Declaration` — a SIG body's bodyless `EXPR (<head>) -> <Return>`, which builds no callable at
///   all: it records the head's bucket key and `(params) -> ret` type as a keyworded member of the
///   signature under construction. It rides this path so a declaration and a definition derive
///   their key and slot types through one parse.
/// - `Shape` — the bodyless `EXPR (<head>) -> <Return>`, whose carrier is the head's expression
///   shape as a type value. `declare` is set where that value is also a declaration: bare inside a
///   SIG body, where the shape is recorded as a keyworded member of the signature under
///   construction. Under `:(…)` — or anywhere outside a SIG body — the value is the whole answer.
///
/// `bound_name` is the symbol the `name` slot captured — the one the parse minted — so the kind
/// carries no borrow and stays `Copy`.
#[derive(Clone, Copy)]
pub(crate) enum FnKind {
    Function {
        bound_name: Option<crate::machine::model::ValueSymbol>,
    },
    Anonymous,
    Declaration,
    Shape {
        declare: bool,
    },
}

impl FnKind {
    /// Whether this surface reads a `_ :<Type>` position as a positional slot. Only a bodyless
    /// head does: its type drops the slot names, so there is nothing for `_` to lose.
    pub(crate) fn wildcards(self) -> Wildcards {
        match self {
            FnKind::Declaration | FnKind::Shape { .. } => Wildcards::Slots,
            FnKind::Function { .. } | FnKind::Anonymous => Wildcards::Refused,
        }
    }
}

/// The `FOR ALL (<names>)` group a head declares, threaded through the whole elaboration: the
/// names in `Quantified(index)` order, and the child scope they are bound in. That scope is what
/// the head's slot types and its return elaborate against, so a quantifier reaches them through
/// the ordinary type-name lookup however it is spelled.
///
/// An unquantified head is the empty group standing in its own defining scope
/// ([`Quantification::none`]), so every surface takes one path.
#[derive(Clone, Copy)]
pub(crate) struct Quantification<'a> {
    pub(crate) names: &'a [TypeSymbol],
    pub(crate) scope: &'a Scope<'a>,
}

impl<'a> Quantification<'a> {
    /// The empty group: an unquantified head, elaborated in its own defining scope.
    pub(crate) fn none(scope: &'a Scope<'a>) -> Self {
        Quantification { names: &[], scope }
    }

    /// Where a slot's sub-dispatch runs. A quantified head's slots must see the group, so they
    /// enter its scope; an unquantified head's take the slot's own node scope, as every other
    /// binder's type sub-dispatch does.
    fn placement(self) -> crate::machine::DepPlacement<'a> {
        if self.names.is_empty() {
            crate::machine::DepPlacement::OwnScope
        } else {
            crate::machine::DepPlacement::InScope(self.scope)
        }
    }
}

/// What a finalize is finalizing: the surface's [`FnKind`] and the [`Quantification`] its head
/// stood under. The two travel together everywhere — the kind decides what is installed, the group
/// decides what the head elaborated against — so they cross as one operand.
#[derive(Clone, Copy)]
pub(crate) struct FnSurface<'a> {
    pub(crate) kind: FnKind,
    pub(crate) quantification: Quantification<'a>,
}

/// Local mirror of [`ParamListOutcome`] minus the structural-error variant
/// (short-circuited before [`classify`] runs) and with `Pending`'s payload
/// kept by-value so the planning match stays readable.
pub(crate) enum ParamListResult<'a> {
    Done(BumpVec<'a, SignatureElement>),
    Pending {
        awaited_producers: Vec<ProducerId>,
        sub_dispatches: BumpVec<'a, (usize, KExpression<'a>)>,
    },
}

/// Terminal shape of FN-def's planning step.
pub(crate) enum FnPlan<'a> {
    Synchronous {
        elements: BumpVec<'a, SignatureElement>,
        return_type: ReturnType<'a>,
    },
    Deferred(DeferredInputs<'a>),
}

/// Inputs to [`defer`]: carrier that survives the dep-finish boundary
/// plus the two parking lists.
pub(crate) struct DeferredInputs<'a> {
    pub capture: ReturnTypeCapture<'a>,
    /// The binder claim edges this dep-finish waits on at finish-time but does NOT
    /// own: it names each by the source it already holds, so the door mints this
    /// slot's own edge off that source and nothing here retires the producer behind it.
    pub awaited_producers: Vec<ProducerId>,
    /// `Some` only when the return-type slot is an `Expression(_)` carrier that
    /// doesn't reference any FN parameter (resolves once at FN-def time, not
    /// per call). Appended ahead of `sub_dispatches`.
    pub return_type_sub: Option<KExpression<'a>>,
    /// `(slot_idx, sub_expr)` — `slot_idx` tells the finish closure which
    /// `signature_expr.parts` slot to splice the result into.
    pub sub_dispatches: BumpVec<'a, (usize, KExpression<'a>)>,
    /// `Some` for the anonymous (`FN :{…}`) path: the parameter list is already
    /// built from the resolved record schema, so the finish closure uses it
    /// verbatim instead of re-parsing `signature_expr` (which the anonymous path
    /// has no keyword/arg form of). `None` for the keyworded FN path, which
    /// re-elaborates the spliced signature. Unlike `sub_dispatches` above — read back by the
    /// `defer` that schedules them, inside the step that built it — this one is carried across the
    /// wake, so it is bumped in the frame region rather than on the step scratch that pop resets.
    pub prebuilt_elements: Option<BumpVec<'a, SignatureElement>>,
}

/// Decide between the synchronous build path and the deferred path.
///
/// Arms differ only in how they shape the [`ReturnTypeCapture`] and merge the
/// two parking lists. All eight `(ReturnTypeState × ParamListResult)` combos
/// route to exactly one [`FnPlan`] outcome — no further routing downstream.
pub(crate) fn classify<'a>(
    rt: ReturnTypeState<'a>,
    params: ParamListResult<'a>,
    scratch: BumpAllocator<'a>,
) -> FnPlan<'a> {
    match (rt, params) {
        (ReturnTypeState::Done(kt), ParamListResult::Done(elements)) => FnPlan::Synchronous {
            elements,
            return_type: ReturnType::Resolved(kt),
        },
        (ReturnTypeState::Deferred(d), ParamListResult::Done(elements)) => FnPlan::Synchronous {
            elements,
            return_type: ReturnType::Deferred(d),
        },
        (ReturnTypeState::ExprToSubDispatch(e), ParamListResult::Done(_)) => {
            FnPlan::Deferred(DeferredInputs {
                capture: ReturnTypeCapture::ReturnTypeExpr,
                awaited_producers: Vec::new(),
                return_type_sub: Some(e),
                sub_dispatches: BumpVec::new_in(scratch),
                prebuilt_elements: None,
            })
        }
        (
            ReturnTypeState::Done(kt),
            ParamListResult::Pending {
                awaited_producers,
                sub_dispatches,
            },
        ) => FnPlan::Deferred(DeferredInputs {
            capture: ReturnTypeCapture::Resolved(kt),
            awaited_producers,
            return_type_sub: None,
            sub_dispatches,
            prebuilt_elements: None,
        }),
        (
            ReturnTypeState::Deferred(d),
            ParamListResult::Pending {
                awaited_producers,
                sub_dispatches,
            },
        ) => FnPlan::Deferred(DeferredInputs {
            // Return type is per-call-deferred: carry the carrier verbatim
            // through to `finalize_fn_with_kind` once params land.
            capture: ReturnTypeCapture::Deferred(d),
            awaited_producers,
            return_type_sub: None,
            sub_dispatches,
            prebuilt_elements: None,
        }),
        (
            ReturnTypeState::ExprToSubDispatch(e),
            ParamListResult::Pending {
                awaited_producers,
                sub_dispatches,
            },
        ) => FnPlan::Deferred(DeferredInputs {
            capture: ReturnTypeCapture::ReturnTypeExpr,
            awaited_producers,
            return_type_sub: Some(e),
            sub_dispatches,
            prebuilt_elements: None,
        }),
        (ReturnTypeState::Pending { te, producers }, ParamListResult::Done(_)) => {
            // Synchronously elaborated `elements` are discarded; the wake
            // re-elaborates the param list against the spliced signature.
            FnPlan::Deferred(DeferredInputs {
                capture: make_capture(te),
                awaited_producers: producers,
                return_type_sub: None,
                sub_dispatches: BumpVec::new_in(scratch),
                prebuilt_elements: None,
            })
        }
        (
            ReturnTypeState::Pending {
                te,
                producers: rt_producers,
            },
            ParamListResult::Pending {
                mut awaited_producers,
                sub_dispatches,
            },
        ) => {
            awaited_producers.extend(rt_producers);
            FnPlan::Deferred(DeferredInputs {
                capture: make_capture(te),
                awaited_producers,
                return_type_sub: None,
                sub_dispatches,
                prebuilt_elements: None,
            })
        }
    }
}

/// Reject a bare type constructor in either of a function's value type positions. A parameter
/// annotation and a resolved return type each name the type of a value, so each must be a proper
/// type; a constructor of kind `* -> *` standing unapplied is a kind error. The single gate for
/// every FN surface — keyworded and anonymous, synchronous and dep-finished — since all of them
/// reach [`finalize_fn_with_kind`]. A [`ReturnType::Deferred`] carrier names a parameter and
/// elaborates per call, so it is checked at that boundary, not here.
fn check_value_type_kinds(
    elements: &[SignatureElement],
    return_type: &ReturnType<'_>,
    registries: &RunRegistries,
) -> Result<(), KError> {
    use crate::machine::model::unsaturated_constructor_message;
    for element in elements {
        if let SignatureElement::Argument(argument) = element
            && let Some(message) = unsaturated_constructor_message(
                argument.ktype,
                format_args!(
                    "the type of FN parameter `{}`",
                    display_label(argument.name.symbol(), registries)
                ),
                registries,
            )
        {
            return Err(KError::new(KErrorKind::ShapeError(message)));
        }
    }
    if let ReturnType::Resolved(kt) = return_type
        && let Some(message) =
            unsaturated_constructor_message(*kt, "the FN return type", registries)
    {
        return Err(KError::new(KErrorKind::ShapeError(message)));
    }
    Ok(())
}

/// Reject a signature that names the same parameter twice.
///
/// A repeated name has no reading that works. Positionally the second slot's binding overwrites
/// the first, so one of the two arguments the caller passed is silently unreachable in the body;
/// by name it is worse, because a field record carries one value per name, so no call can fill
/// both slots and the call site is told it is missing an argument it did in fact supply. Refusing
/// the definition puts the diagnostic on the signature that is actually wrong.
///
/// Quadratic in the parameter names, which a signature has a handful of, and this runs once per
/// definition.
fn check_distinct_parameter_names(
    elements: &[SignatureElement],
    registries: &RunRegistries,
) -> Result<(), KError> {
    let names = || {
        elements.iter().filter_map(|element| match element {
            SignatureElement::Argument(argument) => Some(argument.name),
            SignatureElement::Keyword(_) => None,
        })
    };
    for (slot, name) in names().enumerate() {
        if names().take(slot).any(|earlier| earlier == name) {
            return Err(KError::new(KErrorKind::ShapeError(format!(
                "FN parameter `{}` is declared more than once; each parameter of a \
                 signature must have its own name",
                render_label(name.symbol(), registries),
            ))));
        }
    }
    Ok(())
}

/// Build the `KFunction` and, for a keyworded `Function`, register it under its lead
/// keyword — plus, for the combined form, bind it under the statement's value name.
/// `Anonymous` skips registration entirely — the value it returns is the
/// function's only handle.
pub(crate) fn finalize_fn_with_kind<'a>(
    scope: &'a Scope<'a>,
    elements: &[SignatureElement],
    return_type: ReturnType<'a>,
    body_expr: KExpression<'a>,
    surface: FnSurface<'a>,
    bind_index: BindingIndex,
    registries: &RunRegistries,
) -> Result<FinalizedFn<'a>, KError> {
    let FnSurface {
        kind,
        quantification,
    } = surface;
    check_value_type_kinds(elements, &return_type, registries)?;

    match kind {
        // A bodyless head's slots are positional and its type drops their names, so two slots
        // sharing a name collide over nothing. Every other surface binds its arguments by name in
        // a body, where a repeat has no reading that works.
        FnKind::Declaration | FnKind::Shape { .. } => {
            return finalize_bodyless_head(
                scope,
                elements,
                return_type,
                kind,
                quantification,
                registries,
            );
        }
        FnKind::Function { .. } | FnKind::Anonymous => {
            check_distinct_parameter_names(elements, registries)?;
        }
    }

    // First Keyword keys the data table. Dispatch is by full signature via
    // `Bindings::functions`; `Bindings::data` is for discoverability /
    // shadow-by-name, neither of which has a single right answer for a
    // multi-token signature like `(a ADD b)`. Presence is all that is read: the bucket key the
    // registration lands under is derived from the whole signature at seal time, so no spelling is
    // resolved here.
    let has_dispatch_keyword = elements
        .iter()
        .any(|e| matches!(e, SignatureElement::Keyword(_)));
    let birth = KFunction::alloc_captured(
        scope,
        return_type,
        elements,
        quantification.names,
        Body::UserDefined(body_expr),
        registries,
    );
    // `frame: None` — the scheduler's lift-on-return populates the Rc if this
    // KFunction value escapes a per-call body; top-level FNs have no frame. The birth envelope
    // carries the description the callable's own construction composed — hosted in `scope`'s region
    // with that region its one member — and both doors below compose from it, so the wrapper's
    // reach and the bucket's are the same derived fact rather than two independent claims.
    // A keyworded FN's overload registration rides the step outcome: the seal is built here, from
    // the envelope's own open, and the bucket write lands at the run loop's apply.
    // A finalize emits at most two writes — this registration, and the combined form's value
    // binding below — so the pair rides out as a fixed array. The action's own bump is where they
    // land, so nothing between here and there needs a buffer of its own.
    let mut overload_write: Option<WriteOp<'a>> = None;
    let bound_name = match kind {
        // A bodyless head never reaches here: it returns above, before any callable is built.
        FnKind::Anonymous | FnKind::Declaration | FnKind::Shape { .. } => None,
        FnKind::Function { bound_name } => {
            if !has_dispatch_keyword {
                return Err(KError::new(KErrorKind::ShapeError(
                    "FN signature must contain at least one Keyword (a fixed token to dispatch on)"
                        .to_string(),
                )));
            }
            overload_write = Some(WriteOp::Overload {
                index: bind_index,
                seal: OverloadSeal::of_delivered(scope, &birth),
                builtin_shadow_guard: true,
            });
            bound_name
        }
    };
    // The FN value is co-located in its defining scope's region (owned signature / body, a `&Scope`
    // capture), and the captured scope — region-resident under that frame — transitively keeps every
    // foreign region its bindings reach alive through the scope's sealed reach-set. So a fresh FN
    // reaches nothing foreign: the wrapper's merge takes the birth envelope as its source operand,
    // so its composed reach names that home region and nothing else.
    let cell = scope.store_function_cell(&birth);
    // The combined form's value write duplicates the very cell the terminal carries, at the same
    // `BindingIndex` the overload write and the submission-time placeholder both stamp — so the
    // bound name and the registered overload are the one `KFunction` allocated above, not two
    // builds of the same source.
    let value_write = bound_name.map(|bound_name| WriteOp::Value {
        name: bound_name,
        index: bind_index,
        sealed: cell.duplicate(),
    });
    Ok((cell.unseal(), [overload_write, value_write]))
}

/// The bodyless-head leg of [`finalize_fn_with_kind`]: derive the head's expression shape. No
/// callable is built — a bodyless head has no body — so the step's own carrier is that shape as a
/// type value, uniform with what a `VAL` slot hands back. Where the head also declares — every
/// `FnKind::Declaration`, and a `FnKind::Shape` standing bare in a SIG body — it additionally
/// records that shape as a keyworded member of the SIG under construction.
///
/// Two shapes are rejected here rather than at the ascription that would meet them. A head with no
/// fixed token has no bucket to declare (the same rule a definition's registration applies), and a
/// return type that names a parameter (`-> er.Carrier`) is a *per-call* elaboration: a declaration
/// has no call to elaborate it at, so the member would have no type. Everything else — the
/// proper-type checks over parameters and return — already ran on the shared path above.
fn finalize_bodyless_head<'a>(
    scope: &'a Scope<'a>,
    elements: &[SignatureElement],
    return_type: ReturnType<'a>,
    kind: FnKind,
    quantification: Quantification<'a>,
    registries: &RunRegistries,
) -> Result<FinalizedFn<'a>, KError> {
    let declare = match kind {
        FnKind::Declaration => true,
        FnKind::Shape { declare } => declare,
        // The caller routes only the two bodyless kinds here.
        FnKind::Function { .. } | FnKind::Anonymous => false,
    };
    if !elements
        .iter()
        .any(|e| matches!(e, SignatureElement::Keyword(_)))
    {
        return Err(KError::new(KErrorKind::ShapeError(match kind {
            FnKind::Shape { .. } => "a shape has at least one keyword — a keyword-free callable \
                 is a lambda, written `:(FN :{…} -> <Return>)`"
                .to_string(),
            _ => "a SIG keyworded member must contain at least one Keyword (a fixed token to \
                 dispatch on) — write `(VAL <name>: <FnType>)` to declare a function value member"
                .to_string(),
        })));
    }
    let ReturnType::Resolved(ret) = return_type else {
        return Err(KError::new(KErrorKind::ShapeError(format!(
            "the return type of a SIG keyworded member must be a type, but `{}` names a \
             parameter and resolves per call — a declaration has no call to resolve it at",
            return_type.name(registries),
        ))));
    };
    let shape = shape_type_of(elements, quantification.names, ret, registries);
    Ok((
        scope.resident(Carried::Type(shape)),
        [declare.then_some(WriteOp::SigKeyworded { shape }), None],
    ))
}

/// Wrap a [`finalize_fn_with_kind`] result in the action currency. The FN value is built witnessed
/// (it names its captured scope's frame), so success seals as `Done(Ok)` carrying the overload
/// registration as the step's effect.
pub(crate) fn fn_action<'a>(
    scratch: crate::witnessed::BumpAllocator<'a>,
    result: Result<FinalizedFn<'a>, KError>,
) -> Action<'a> {
    match result {
        Ok((witnessed, writes)) => Action::done(Ok(StepCarried::born(witnessed)))
            .with_effects(scratch, writes.into_iter().flatten()),
        Err(e) => Action::done(Err(e)),
    }
}

/// Schedule an `AwaitDeps` over `awaited_producers` plus any newly scheduled
/// sub-Dispatches for parens-wrapped parameter types, then re-run the signature
/// elaboration in the finish closure.
///
/// Dep order is `[forward refs ++ rt? ++ subs]` and results come back in it, so each request's
/// index is recorded as it is appended rather than derived from a layout rule.
pub(crate) fn defer<'a>(
    scope: &'a Scope<'a>,
    signature_expr: KExpression<'a>,
    inputs: DeferredInputs<'a>,
    body_expr: KExpression<'a>,
    surface: FnSurface<'a>,
    bind_index: BindingIndex,
) -> crate::machine::Action<'a> {
    use crate::machine::model::WorkingExpression;
    use crate::machine::{Action, AwaitContinue, SubDispatch};
    let FnSurface {
        kind,
        quantification,
    } = surface;
    let DeferredInputs {
        capture,
        awaited_producers,
        return_type_sub,
        sub_dispatches,
        prebuilt_elements,
    } = inputs;
    let brand = scope.brand();
    // The forward-ref producers this finalize merely waits on come first, then the return-type sub
    // and the signature subs in declaration order. Each `request` hands back its dep index, which is
    // the position its result comes back at; `splice_layout` pairs that with the signature
    // part-index for the finish.
    let mut deps = deps_on(awaited_producers.iter().copied());
    let return_type_dep = return_type_sub.map(|rt_expr| {
        deps.request(SubDispatch {
            expr: WorkingExpression::from_ast(brand, rt_expr),
            placement: quantification.placement(),
        })
    });
    // `splice_layout` is read by the finish closure below, which runs at a later drain pop than
    // this one — so its home is the frame region the continuation's own `'a` already outlives,
    // not the step scratch that pop resets. One push per sub-dispatch makes the reservation exact.
    let mut splice_layout: BumpVec<'a, (usize, usize)> =
        BumpVec::with_capacity_in(sub_dispatches.len(), brand.allocator());
    for (slot_idx, sub_expr) in sub_dispatches {
        let dep_index = deps.request(SubDispatch {
            expr: WorkingExpression::from_ast(brand, sub_expr),
            placement: quantification.placement(),
        });
        splice_layout.push((slot_idx, dep_index));
    }
    let finish: AwaitContinue<'a> = Box::new(move |fctx, results| {
        // Extract each signature slot's resolved type: each dep is resident in a region this step
        // already covers, read at the step's own brand. A `KType` is an interned handle, so it
        // escapes the open guard's borrow and the re-walk below feeds on owned data alone.
        let mut resolved: BumpVec<'a, (usize, KType)> =
            BumpVec::with_capacity_in(splice_layout.len(), fctx.scratch);
        for &(slot_idx, dep_index) in &splice_layout {
            let terminal = results[dep_index];
            let opened = terminal.cell.open_at();
            match opened.value() {
                Carried::Type(ktype) => resolved.push((slot_idx, ktype)),
                other => {
                    return Action::done(Err(KError::new(KErrorKind::ShapeError(format!(
                        "FN signature slot at part-index {slot_idx} expected a type expression, \
                         got a {} value",
                        other.ktype(fctx.types()).name(fctx.registries),
                    )))));
                }
            }
        }
        let return_type: ReturnType<'a> = crate::try_action!(resolve_capture_at_finish(
            capture,
            quantification.scope,
            results,
            return_type_dep,
            fctx.registries
        ));
        let elements = match prebuilt_elements {
            Some(es) => es,
            None => {
                let mut elaborator = Elaborator::new(quantification.scope);
                match parse_fn_param_list(
                    &signature_expr,
                    &mut elaborator,
                    fctx.registries,
                    Some(&resolved),
                    kind.wildcards(),
                    fctx.scratch,
                ) {
                    ParamListOutcome::Done(es) => es,
                    ParamListOutcome::Err(error) => return Action::done(Err(error)),
                    ParamListOutcome::Pending { .. } => {
                        return Action::done(Err(KError::new(KErrorKind::ShapeError(
                            "FN signature elaboration still pending after dep-finish wake"
                                .to_string(),
                        ))));
                    }
                }
            }
        };
        fn_action(
            fctx.scratch,
            finalize_fn_with_kind(
                fctx.scope,
                &elements,
                return_type,
                body_expr,
                surface,
                bind_index,
                fctx.registries,
            ),
        )
    });
    crate::machine::Action::await_deps(deps, finish)
}
