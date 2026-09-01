//! `CLOSE OVER (<captures>) (<block>)` and `CLOSE (<block>)` — severance with the captures written
//! out, and severance with them inferred. See
//! [design/lazy-closures.md](../../design/lazy-closures.md).
//!
//! The two forms differ only in where the capture list comes from ([`Captures`]); everything below
//! that — resolution, parking, the severed frame, the seed — is one spine.
//!
//! The block runs over a dedicated **per-call-tier region with no `outer` storage link**, so a value
//! homed there pins that one region and whatever its captures still borrow — never the chain of
//! frames the block was written inside. The region comes from `CallFrame::new` handed the innermost
//! eternal-homed enclosing scope ([`Scope::innermost_eternal_home`]): `parent_frame_pin` declines to
//! chain an eternal owner, so the fresh storage's `outer` is `None` and the frame's child scope's
//! *lexical* outer is that eternal scope. Builtins and top-level definitions stay visible through
//! the link and contribute no reach; every per-call binding must arrive as a capture.
//!
//! The block's statements are frozen as working copies into that same per-call region: the body
//! crosses the cart install as raw AST and the reinstalled step, which runs with the block frame as
//! its own cart, freezes it at that cart's brand. The copies are read from the block frame's cart,
//! which severance leaves holding nothing of the caller, so a copy homed at the call site would
//! dangle the moment the calling frame retires — and one homed in the eternal chain would outlive
//! every evaluation, so a form re-entered many times would grow the run-root arena without bound.
//! Homing them in the cart is what makes both false: the copies die with the region the block runs
//! in.
//!
//! Three kinds of capture reach the block scope, seeded before its first statement dispatches:
//!
//! - An **identifier** (either channel) is copied in at the severing adoption seam
//!   ([`Scope::bind_delivered_severed`]), which forces the `Copy` verb: data is rebuilt
//!   transitively at the block's own door and strings are re-bumped, so the copy's release-exact
//!   reach names nothing in the producer's region and that region is free to die. A `KType` is a
//!   lifetime-free handle and copies by value. A capture that resolves to a **callable** takes the
//!   `Consolidate` verb instead: its own captured environment is rebuilt at the block's region too,
//!   so the severance is transitive ([lazy-closures.md § Lazy
//!   close](../../design/lazy-closures.md)). A **module** capture still rides that copy as a
//!   borrow leaf — i.e. pinned — the remaining deferred half
//!   ([module-scope-consolidation.md](../../roadmap/foundation/module-scope-consolidation.md)).
//! - A **signature-shaped pattern** `(HELPER _)` names one full untyped bucket key and captures
//!   every visible overload registered under it, pinned.
//! - **Implicit close** copies every dispatch registration, operator-registry entry and module
//!   binding visible in the *per-call* portion of the enclosing chain, pinned. This is a build-time
//!   act: an escaped closure's body dispatches after every one of its ancestors is dead, so nothing
//!   may resolve outward later.
//!
//! Pins are transitive through the existing protocol — resting a lifted envelope here lodges its
//! whole coverage in this region's bundle, and a pinned region's own binding entries hold what
//! *they* reach — so a closed-over callable still runs correctly once every non-pinned producer
//! frame has died.
//!
//! The build parks on every visible in-flight claim in that per-call chain, name claims and bucket
//! claims alike, so what the block closes over never depends on drain order. The wait is acyclic:
//! a claim names a strictly earlier statement (the binding tables' exclusive cutoff keeps a
//! statement's own claim out of its own subtree), and `CLOSE OVER` installs no binder plan of its
//! own, so nothing can wait on it.
//!
//! `EVAL` inside an explicitly-captured block is permitted and needs no arm here: a name resolves
//! against the block scope, its captures and the eternal chain, and the ancestor walk simply ends at
//! the eternal scope's own outer, landing `UnboundName`. Under `CLOSE` it is a
//! [`DynamicNamesUnderInferredClose`](crate::machine::KErrorKind::DynamicNamesUnderInferredClose)
//! error instead: what a form resolves at evaluation cannot be read off the text beforehand, and
//! `CLOSE OVER` is the spelling that admits one.
//!
//! ## Which names an inferred capture list holds
//!
//! [`infer_close_captures`] hands back every name the block would resolve outward for; this file
//! decides which of them to *copy*. A name that resolves in the **per-call** portion of the chain
//! must be captured — its home dies while the block's value lives. One that resolves in the
//! **eternal** portion is skipped: it stays visible through the severed block's own outer link, so a
//! copy buys nothing. [`HitTier`] is that split, read off the same resolution that found the
//! binding, so `CLOSE` and `CLOSE OVER (<the same names>)` capture the identical bindings.

use std::rc::Rc;

use crate::machine::execute::deps_on;
use crate::machine::model::RunRegistries;
use crate::machine::model::TypeResolution;
use crate::machine::model::infer_close_captures;
use crate::machine::model::{
    ExpressionPart, KExpression, KObject, KType, KeyElement, KeywordSymbol, TypeSymbol,
    ValueSymbol, WILDCARD, render_label, render_untyped_key,
};
use crate::machine::{Action, AwaitContinue, CallFrame, DeliveredCarried, WriteGate};
use crate::machine::{BindingIndex, DeclarationSite};
use crate::machine::{DeliveredFunction, DeliveredOperatorGroup, Scope};
use crate::machine::{HitTier, KError, KErrorKind, LexicalFrame, NameLookup, ProducerId};
use crate::machine::{fresh_cart_tail, seed};
use crate::witnessed::{BumpAllocator, BumpVec};

use super::{arg, kw, sig};

// This builtin's slot spellings, minted once and read back by symbol.
crate::slots! { SLOTS { body, captures } }

/// Where a form's capture list comes from. The one thing the two `CLOSE` overloads differ in — a
/// `Copy` handle either way, so a park's continuation carries it back into the same spine.
#[derive(Clone, Copy)]
enum Captures<'a> {
    /// `CLOSE OVER (<captures>) (<block>)`: the slot the form spells its captures in, still raw.
    Listed(KExpression<'a>),
    /// `CLOSE (<block>)`: derived from the block's free identifiers, per evaluation.
    Inferred,
}

/// One entry of the capture list, as read off the slot before anything is resolved. Staged on the
/// step scratch, like every other transient here: a pass reads the list out of the `captures` slot
/// it still holds, so nothing has to survive the park — the wake re-reads the same slot and gets
/// the same list.
enum Capture<'a> {
    /// A bare name on the value channel — `x`.
    Value(ValueSymbol),
    /// A bare name on the type channel — `Meters`.
    Type(TypeSymbol),
    /// A signature-shaped group naming one full untyped bucket key — `(HELPER _)`. The key is a
    /// scratch-staged run rather than an owned `UntypedKey`: [`KeyElement`] is `Copy`, so the run
    /// probes the bucket tables directly through the same slice door a node's own bumped key uses.
    Pattern(BumpVec<'a, KeyElement>),
}

/// Everything the block scope is seeded with, resolved and lifted. Every carrier is a lifetime-free
/// delivery envelope — the seed writes them inside the block frame's `for<'b>` open, where nothing
/// at the caller's lifetime could be admitted — and the five buffers holding them are step scratch,
/// which outlives the seed by construction: the seed runs before this builtin's body returns.
struct CapturePlan<'a> {
    /// Explicit identifier captures on the value channel — bound through the **severing** seam.
    values: BumpVec<'a, (ValueSymbol, DeliveredCarried)>,
    /// Explicit identifier captures on the type channel — `Copy` handles, nothing to relocate.
    types: BumpVec<'a, (TypeSymbol, KType)>,
    /// Pattern captures and implicit close's registrations — bound **pinned**.
    functions: BumpVec<'a, DeliveredFunction>,
    /// Implicit close's operator-registry entries — bound pinned, by probe key.
    operators: BumpVec<'a, (KeywordSymbol, DeliveredOperatorGroup)>,
    /// Implicit close's module bindings — bound pinned.
    modules: BumpVec<'a, (ValueSymbol, DeliveredCarried)>,
}

impl<'a> CapturePlan<'a> {
    fn new(scratch: BumpAllocator<'a>) -> Self {
        Self {
            values: BumpVec::new_in(scratch),
            types: BumpVec::new_in(scratch),
            functions: BumpVec::new_in(scratch),
            operators: BumpVec::new_in(scratch),
            modules: BumpVec::new_in(scratch),
        }
    }
}

/// What one resolution pass produced: a finished plan, or the set of in-flight producers the form
/// has to wait on first. Errors take the `Result` channel around this.
enum Pass<'a> {
    Ready(CapturePlan<'a>),
    Park(BumpVec<'a, ProducerId>),
}

pub fn body<'a>(ctx: &crate::machine::BodyCtx<'_, 'a, '_>) -> Action<'a> {
    use crate::machine::require_kexpression;

    let captures = crate::try_action!(require_kexpression(ctx.args, "CLOSE OVER", &SLOTS.captures));
    let block = crate::try_action!(require_kexpression(ctx.args, "CLOSE OVER", &SLOTS.body));
    build(
        ctx.scope,
        ctx.chain.clone(),
        Captures::Listed(captures),
        block,
        ctx.scratch,
        ctx.registries,
    )
}

/// `CLOSE (<block>)` — the same severance with the capture list derived from the block instead of
/// spelled. Shares [`build`] whole; only the [`Captures`] source differs.
pub fn body_inferred<'a>(ctx: &crate::machine::BodyCtx<'_, 'a, '_>) -> Action<'a> {
    use crate::machine::require_kexpression;

    let block = crate::try_action!(require_kexpression(ctx.args, "CLOSE", &SLOTS.body));
    build(
        ctx.scope,
        ctx.chain.clone(),
        Captures::Inferred,
        block,
        ctx.scratch,
        ctx.registries,
    )
}

/// Resolve the capture list against `scope`, then hand the block its seeded region — or park and
/// re-enter here when a capture or a visible registration is still in flight. Re-entrant by
/// construction: `captures` and `block` are the same `Copy` slot handles the synchronous pass held,
/// and the wake-side scope is the slot's own, so the second pass reads the same list and resolves
/// against exactly the chain the first did.
///
/// `scratch` is the *running* step's arena, never a captured one: a park's continuation carries only
/// the two slot handles and the chain, and the wake passes its own `FinishCtx`'s scratch back in.
/// That is what lets every transient below live on the arena — the list (or the inference that
/// stands in for it), the plan and the park set all die with the pop that produced them, and the
/// wake rebuilds them on its own.
fn build<'a>(
    scope: &'a Scope<'a>,
    chain: Option<Rc<LexicalFrame>>,
    captures: Captures<'a>,
    block: KExpression<'a>,
    scratch: BumpAllocator<'a>,
    registries: &RunRegistries,
) -> Action<'a> {
    let plan = match resolve(scope, chain.as_ref(), captures, block, scratch, registries) {
        Ok(Pass::Ready(plan)) => plan,
        Ok(Pass::Park(sources)) => {
            let finish: AwaitContinue<'a> = Box::new(move |fctx, _results| {
                build(
                    fctx.scope,
                    chain,
                    captures,
                    block,
                    fctx.scratch,
                    fctx.registries,
                )
            });
            return Action::await_deps(deps_on(sources), finish);
        }
        Err(error) => return Action::done(Err(error)),
    };

    // The fresh frame's storage takes no `outer` chain, which is what makes the block region a root
    // of its own rather than a link in the producer's. `parent_frame_pin`'s eternal-tier policy is
    // the only reason it holds, so the premise is asserted where it is relied on.
    let eternal = scope.innermost_eternal_home();
    debug_assert!(
        eternal.parent_frame_pin().is_none(),
        "the block frame's lexical outer must be eternal-homed: an unpinned `&Scope` outer is \
         sound only because its region outlives the run",
    );
    let frame: Rc<CallFrame> = CallFrame::new(eternal);

    // The body crosses to the scheduler raw and is frozen by the reinstalled step, so the working
    // copies land in the block frame's own region and are released with it. The seed's failure
    // travels out here rather than through the block: a capture that cannot bind is this
    // statement's own error, and the block has not dispatched anything yet.
    let mut failure: Option<KError> = None;
    let action = fresh_cart_tail(
        frame,
        Some(seed(
            |block_scope, registries: &RunRegistries, gate: &mut WriteGate| {
                failure = plan.seed_into(block_scope, registries, gate).err();
            },
        )),
        block,
        None,
        registries,
    );
    match failure {
        Some(error) => Action::done(Err(error)),
        None => action,
    }
}

// ---------- reading the capture list ----------

/// Read the `captures` slot into the list the form will resolve.
///
/// The slot's own shape decides how it is read: a top-level `Keyword` part means the parser peeled a
/// redundant group and handed back the pattern itself (`CLOSE OVER ((HELPER _))` arrives as
/// `HELPER _`), so the whole slot is **one** pattern capture. Otherwise its parts are the list —
/// names on either channel, and parenthesized groups as patterns. A bare keyword inside a list is an
/// error: a dispatch registration is named by its full bucket key, never by a lead keyword.
fn read_capture_list<'a>(
    slot: KExpression<'a>,
    scratch: BumpAllocator<'a>,
    registries: &RunRegistries,
) -> Result<BumpVec<'a, Capture<'a>>, KError> {
    if slot
        .parts
        .iter()
        .any(|part| matches!(part.value, ExpressionPart::Keyword(_)))
    {
        let mut list = BumpVec::with_capacity_in(1, scratch);
        list.push(Capture::Pattern(read_pattern(slot, scratch, registries)?));
        return Ok(list);
    }
    // One entry per part, so the buffer takes its capacity up front and never grows — a grown bump
    // buffer abandons its old bytes until the pop.
    let mut list = BumpVec::with_capacity_in(slot.parts.len(), scratch);
    for part in slot.parts.iter() {
        list.push(match part.value {
            ExpressionPart::Identifier(name) => Capture::Value(name),
            ExpressionPart::Type(name) => Capture::Type(name),
            ExpressionPart::Expression(group) => {
                Capture::Pattern(read_pattern(*group, scratch, registries)?)
            }
            _ => {
                return Err(shape_error(
                    "a capture is a name or a signature-shaped group like `(HELPER _)`",
                ));
            }
        });
    }
    Ok(list)
}

/// Read one signature-shaped group into the untyped bucket key it names. `_` — already keyword-class
/// as a pure-symbol token, so no lexer arm is involved — maps to a slot; every other keyword maps to
/// itself. At least one non-wildcard keyword is required, since an all-slot key names no
/// registration.
fn read_pattern<'a>(
    group: KExpression<'a>,
    scratch: BumpAllocator<'a>,
    registries: &RunRegistries,
) -> Result<BumpVec<'a, KeyElement>, KError> {
    let mut key: BumpVec<'a, KeyElement> = BumpVec::with_capacity_in(group.parts.len(), scratch);
    for part in group.parts.iter() {
        key.push(match part.value {
            ExpressionPart::Keyword(symbol) if symbol == WILDCARD.symbol() => KeyElement::Slot,
            ExpressionPart::Keyword(symbol) => KeyElement::Keyword(symbol),
            _ => {
                return Err(shape_error(
                    "a capture pattern holds keywords and `_` holes only",
                ));
            }
        });
    }
    if !key.iter().any(|el| matches!(el, KeyElement::Keyword(_))) {
        return Err(shape_error(&format!(
            "capture pattern {} names no registration: a bucket key needs at least one keyword",
            render_untyped_key(&key, registries),
        )));
    }
    Ok(key)
}

fn shape_error(detail: &str) -> KError {
    KError::new(KErrorKind::ShapeError(format!("CLOSE OVER: {detail}")))
}

// ---------- resolving ----------

/// One resolution pass: the form's captures, then implicit close over the per-call chain.
/// Every in-flight producer either half meets is collected into a single park set, so N captures
/// and every visible claim cost one wake between them.
fn resolve<'a>(
    scope: &Scope<'_>,
    chain: Option<&Rc<LexicalFrame>>,
    captures: Captures<'a>,
    block: KExpression<'a>,
    scratch: BumpAllocator<'a>,
    registries: &RunRegistries,
) -> Result<Pass<'a>, KError> {
    let mut plan = CapturePlan::new(scratch);
    let mut parked: BumpVec<'a, ProducerId> = BumpVec::new_in(scratch);
    let frame = chain.map(|c| &**c);

    match captures {
        Captures::Listed(slot) => resolve_listed(
            scope,
            chain,
            &read_capture_list(slot, scratch, registries)?,
            scratch,
            registries,
            &mut plan,
            &mut parked,
        )?,
        Captures::Inferred => resolve_inferred(
            scope,
            chain,
            block,
            scratch,
            registries,
            &mut plan,
            &mut parked,
        )?,
    }

    close_implicitly(scope, frame, scratch, &mut plan, &mut parked);

    Ok(if parked.is_empty() {
        Pass::Ready(plan)
    } else {
        Pass::Park(parked)
    })
}

/// The written-out list: every entry resolves, and a name that resolves nowhere is this statement's
/// error. No tier test — naming a capture explicitly says to copy it whatever it resolves to.
#[allow(clippy::too_many_arguments)]
fn resolve_listed<'a>(
    scope: &Scope<'_>,
    chain: Option<&Rc<LexicalFrame>>,
    list: &[Capture<'a>],
    scratch: BumpAllocator<'a>,
    registries: &RunRegistries,
    plan: &mut CapturePlan<'a>,
    parked: &mut BumpVec<'a, ProducerId>,
) -> Result<(), KError> {
    let frame = chain.map(|c| &**c);
    for capture in list {
        match capture {
            Capture::Value(name) => match scope.resolve_value_delivered(*name, frame) {
                Some(NameLookup::Bound(delivered)) => plan.values.push((*name, delivered)),
                Some(NameLookup::Parked(producer)) => park(parked, producer),
                None => return Err(unbound(*name, registries)),
            },
            Capture::Type(name) => {
                resolve_type_capture(scope, chain, *name, registries, plan, parked)?;
            }
            Capture::Pattern(key) => {
                resolve_pattern(scope, frame, key, scratch, registries, plan, parked)?;
            }
        }
    }
    Ok(())
}

/// The derived list: walk `block` for its free identifiers, then keep the ones whose home will not
/// survive the block's value.
///
/// A dynamic-name form anywhere in the inference domain fails the whole form before any resolution,
/// so the error is the same whichever order the names would have resolved in.
fn resolve_inferred<'a>(
    scope: &Scope<'_>,
    chain: Option<&Rc<LexicalFrame>>,
    block: KExpression<'a>,
    scratch: BumpAllocator<'a>,
    registries: &RunRegistries,
    plan: &mut CapturePlan<'a>,
    parked: &mut BumpVec<'a, ProducerId>,
) -> Result<(), KError> {
    let inference = infer_close_captures(&block, scratch, registries);
    if let Some(conflict) = inference.conflict {
        return Err(KError::new(KErrorKind::DynamicNamesUnderInferredClose {
            form: conflict.form,
            location: match (conflict.span, conflict.file) {
                (Some(span), Some(file)) => Some(crate::machine::core::resolve_location(
                    crate::source::SourceRef { span, file },
                )),
                _ => None,
            },
        }));
    }
    let frame = chain.map(|c| &**c);
    for name in inference.values.iter().copied() {
        match scope.resolve_value_tiered(name, frame) {
            Some((HitTier::PerCall, NameLookup::Bound(delivered))) => {
                plan.values.push((name, delivered));
            }
            Some((HitTier::PerCall, NameLookup::Parked(producer))) => park(parked, producer),
            // Eternal-homed: still visible through the block's own outer link, so a copy buys
            // nothing and the block reads it where it lives.
            Some((HitTier::Eternal, _)) => {}
            None => return Err(unbound(name, registries)),
        }
    }
    for name in inference.types.iter().copied() {
        // The tier decides whether to capture; a per-call hit then resolves through the same ladder
        // the written-out list uses, so both forms capture the identical handle under the identical
        // finalize gate. A no-hit is unbound at the form, exactly as in the value loop: every name
        // the walk reports is a genuine type-channel use, because the surfaces that spell a Type
        // token which is *not* a scope name — a union member as a `MATCH … OVER` arm head, an error
        // kind as a `TRY` arm head, an `ATTR` field — are the ones the walk skips.
        match scope.resolve_type_tiered(name, frame) {
            Some((HitTier::PerCall, _)) => {
                resolve_type_capture(scope, chain, name, registries, plan, parked)?;
            }
            // Eternal-homed (every builtin type): visible through the block's own outer link.
            Some((HitTier::Eternal, _)) => {}
            None => return Err(unbound(name, registries)),
        }
    }
    Ok(())
}

/// One type-channel capture, resolved through the elaborator and its finalize gate.
fn resolve_type_capture<'a>(
    scope: &Scope<'_>,
    chain: Option<&Rc<LexicalFrame>>,
    name: TypeSymbol,
    registries: &RunRegistries,
    plan: &mut CapturePlan<'a>,
    parked: &mut BumpVec<'a, ProducerId>,
) -> Result<(), KError> {
    match scope.resolve_type_identifier(name, chain.cloned(), registries) {
        TypeResolution::Done(kt) => plan.types.push((name, kt)),
        TypeResolution::Park(sources) => {
            for producer in sources {
                park(parked, producer);
            }
        }
        TypeResolution::Unbound(missing) => return Err(unbound(missing, registries)),
    }
    Ok(())
}

/// The one `UnboundName` spelling both channels and both forms report through.
fn unbound(
    name: impl crate::machine::model::ClassifiedSymbol,
    registries: &RunRegistries,
) -> KError {
    KError::new(KErrorKind::UnboundName(render_label(
        name.symbol(),
        registries,
    )))
}

fn park(parked: &mut BumpVec<'_, ProducerId>, producer: ProducerId) {
    if !parked.contains(&producer) {
        parked.push(producer);
    }
}

/// Capture every visible overload registered under `key`, anywhere on the chain. A bucket key names
/// a *registration set*, not one overload, and dispatch's own walk keeps going past a scope whose
/// overloads do not match — so taking the innermost scope's bucket alone would drop overloads the
/// call site could still have reached. A visible pending claim on the key joins the park set.
fn resolve_pattern<'a>(
    scope: &Scope<'_>,
    frame: Option<&LexicalFrame>,
    key: &[KeyElement],
    scratch: BumpAllocator<'a>,
    registries: &RunRegistries,
    plan: &mut CapturePlan<'a>,
    parked: &mut BumpVec<'a, ProducerId>,
) -> Result<(), KError> {
    let mut found = false;
    for ancestor in scope.ancestors() {
        let (overloads, pending) =
            ancestor
                .bindings()
                .lifted_overloads_for(key, ancestor.binding_cutoff(frame), scratch);
        if let Some(producer) = pending {
            park(parked, producer);
            found = true;
        }
        found |= !overloads.is_empty();
        plan.functions.extend(overloads);
    }
    if found {
        return Ok(());
    }
    Err(shape_error(&format!(
        "capture pattern {} names no registration in scope",
        render_untyped_key(key, registries),
    )))
}

/// Implicit close: walk the **per-call** portion of the chain — every scope up to but not including
/// the first eternal-homed one, which is exactly the run of frames that will die while the block's
/// value lives — and take each scope's visible registrations, operator entries and module bindings.
/// A `USING … SCOPE` window is part of that walk, so a module opened around the form contributes its
/// surfaced members and its own region is pinned by them.
///
/// Innermost-first, and the write doors settle the shadow rule from there: a duplicate dispatch
/// token, a probe the operator registry already holds, and a standing name all mean an inner
/// scope's entry won.
fn close_implicitly<'a>(
    scope: &Scope<'_>,
    frame: Option<&LexicalFrame>,
    scratch: BumpAllocator<'a>,
    plan: &mut CapturePlan<'a>,
    parked: &mut BumpVec<'a, ProducerId>,
) {
    for ancestor in scope
        .ancestors()
        .take_while(|s| s.parent_frame_pin().is_some())
    {
        let visible = ancestor
            .bindings()
            .visible_for_capture(ancestor.binding_cutoff(frame), scratch);
        for producer in visible.claims {
            park(parked, producer);
        }
        plan.functions.extend(visible.functions);
        plan.operators.extend(visible.operators);
        plan.modules
            .extend(visible.data.into_iter().filter(|(_, delivered)| {
                delivered.open(|carried| matches!(carried.object(), KObject::Module(_)))
            }));
    }
}

// ---------- seeding ----------

impl CapturePlan<'_> {
    /// Write the whole plan into the freshly minted block scope, under the construction gate
    /// `fresh_cart_tail` mints for the call. Explicit captures land first, so they shadow anything
    /// implicit close would otherwise have brought in under the same name or token.
    fn seed_into<'b>(
        self,
        block: &'b Scope<'b>,
        registries: &RunRegistries,
        gate: &mut WriteGate,
    ) -> Result<(), KError> {
        for (name, delivered) in &self.values {
            block.bind_delivered_severed(
                *name,
                delivered,
                BindingIndex::value(0),
                registries,
                gate,
            )?;
        }
        for (name, kt) in &self.types {
            block.register_type_direct(
                *name,
                *kt,
                DeclarationSite::AT_CONSTRUCTION,
                registries,
                gate,
            )?;
        }
        for delivered in &self.functions {
            block.adopt_registration(delivered, registries, gate)?;
        }
        for (probe, delivered) in &self.operators {
            block.adopt_operator_registration(*probe, delivered, registries, gate)?;
        }
        for (name, delivered) in &self.modules {
            // An explicit capture of the same name already stands, and a module surfaced twice by
            // two window scopes is the outer one shadowed; both read as "already bound" here.
            if block.bindings().lookup_value(*name, None).is_none() {
                block.adopt_binding_pinned(*name, delivered, registries, gate)?;
            }
        }
        Ok(())
    }
}

pub fn register<'a>(scope: &'a Scope<'a>, registries: &RunRegistries, gate: &mut WriteGate) {
    let listed = sig(
        KType::ANY,
        vec![
            kw(registries, "CLOSE"),
            kw(registries, "OVER"),
            arg(registries, &SLOTS.captures, KType::KEXPRESSION),
            arg(registries, &SLOTS.body, KType::KEXPRESSION),
        ],
    );
    crate::builtins::register_builtin(scope, listed, body, registries, gate);
    // A distinct bucket key, so `CLOSE OVER () (<block>)` stays the explicit "capture nothing" form
    // rather than an inferred one.
    let inferred = sig(
        KType::ANY,
        vec![
            kw(registries, "CLOSE"),
            arg(registries, &SLOTS.body, KType::KEXPRESSION),
        ],
    );
    crate::builtins::register_builtin(scope, inferred, body_inferred, registries, gate);
}

#[cfg(test)]
mod tests;
