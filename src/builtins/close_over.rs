//! `CLOSE OVER (<captures>) (<block>)` — explicit severance. See
//! [design/lazy-closures.md](../../design/lazy-closures.md).
//!
//! The block runs over a dedicated **per-call-tier region with no `outer` storage link**, so a value
//! homed there pins that one region and whatever its captures still borrow — never the chain of
//! frames the block was written inside. The region comes from `CallFrame::new` handed the innermost
//! eternal-homed enclosing scope ([`Scope::innermost_eternal_home`]): `parent_frame_pin` declines to
//! chain an eternal owner, so the fresh storage's `outer` is `None` and the frame's child scope's
//! *lexical* outer is that eternal scope. Builtins and top-level definitions stay visible through
//! the link and contribute no reach; every per-call binding must arrive as a capture. That same
//! eternal region is where the block's statements are frozen as working copies: they are read from
//! the block frame's cart, which severance leaves holding nothing of the caller, so a copy homed at
//! the call site would dangle the moment the calling frame retires.
//!
//! Three kinds of capture reach the block scope, seeded before its first statement dispatches:
//!
//! - An **identifier** (either channel) is copied in at the severing adoption seam
//!   ([`Scope::bind_delivered_severed`]), which forces the `Copy` verb: data is rebuilt
//!   transitively at the block's own door and strings are re-bumped, so the copy's release-exact
//!   reach names nothing in the producer's region and that region is free to die. A `KType` is a
//!   lifetime-free handle and copies by value. A capture that resolves to a *callable or module*
//!   rides that same copy as a borrow leaf — i.e. pinned — which is the deferred half of the design
//!   ([lazy close](../../roadmap/foundation/lazy-close.md) owns the transitive callable copy).
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
//! `EVAL` inside the block is permitted and needs no arm here: a name resolves against the block
//! scope, its captures and the eternal chain, and the ancestor walk simply ends at the eternal
//! scope's own outer, landing `UnboundName`.

use std::rc::Rc;

use crate::machine::execute::deps_on;
use crate::machine::model::RunRegistries;
use crate::machine::model::TypeResolution;
use crate::machine::model::{
    ExpressionPart, KExpression, KObject, KType, KeyElement, KeywordSymbol, TypeSymbol, UntypedKey,
    ValueSymbol, WILDCARD, render_label, render_untyped_key,
};
use crate::machine::{Action, AwaitContinue, CallFrame, DeliveredCarried, WriteGate};
use crate::machine::{BindingIndex, DeclarationSite};
use crate::machine::{BlockBody, BlockScope, block_tail, seed};
use crate::machine::{DeliveredFunction, DeliveredOperatorGroup, Scope};
use crate::machine::{FramePlacement, KError, KErrorKind, LexicalFrame, NameLookup, ProducerId};

use super::{arg, kw, sig};

// This builtin's slot spellings, minted once and read back by symbol.
crate::slots! { SLOTS { body, captures } }

/// One entry of the capture list, as read off the slot before anything is resolved. Owned and
/// lifetime-free so the list survives a park and is re-read verbatim at wake.
enum Capture {
    /// A bare name on the value channel — `x`.
    Value(ValueSymbol),
    /// A bare name on the type channel — `Meters`.
    Type(TypeSymbol),
    /// A signature-shaped group naming one full untyped bucket key — `(HELPER _)`.
    Pattern(UntypedKey),
}

/// Everything the block scope is seeded with, resolved and lifted. Lifetime-free for the same
/// reason [`Capture`] is, and because the seed writes it inside the block frame's `for<'b>` open,
/// where nothing at the caller's lifetime could be admitted.
#[derive(Default)]
struct CapturePlan {
    /// Explicit identifier captures on the value channel — bound through the **severing** seam.
    values: Vec<(ValueSymbol, DeliveredCarried)>,
    /// Explicit identifier captures on the type channel — `Copy` handles, nothing to relocate.
    types: Vec<(TypeSymbol, KType)>,
    /// Pattern captures and implicit close's registrations — bound **pinned**.
    functions: Vec<DeliveredFunction>,
    /// Implicit close's operator-registry entries — bound pinned, by probe key.
    operators: Vec<(KeywordSymbol, DeliveredOperatorGroup)>,
    /// Implicit close's module bindings — bound pinned.
    modules: Vec<(ValueSymbol, DeliveredCarried)>,
}

/// What one resolution pass produced: a finished plan, or the set of in-flight producers the form
/// has to wait on first. Errors take the `Result` channel around this.
enum Pass {
    Ready(Box<CapturePlan>),
    Park(Vec<ProducerId>),
}

pub fn body<'a>(ctx: &crate::machine::BodyCtx<'_, 'a, '_>) -> Action<'a> {
    use crate::machine::require_kexpression;

    let captures = crate::try_action!(require_kexpression(ctx.args, "CLOSE OVER", &SLOTS.captures));
    let block = crate::try_action!(require_kexpression(ctx.args, "CLOSE OVER", &SLOTS.body));
    let list = crate::try_action!(read_capture_list(captures, ctx.registries));
    build(ctx.scope, ctx.chain.clone(), list, block, ctx.registries)
}

/// Resolve the capture list against `scope`, then hand the block its seeded region — or park and
/// re-enter here when a capture or a visible registration is still in flight. Re-entrant by
/// construction: `list` and `block` are the same values the synchronous pass held, and the wake-side
/// scope is the slot's own, so the second pass resolves against exactly the chain the first did.
fn build<'a>(
    scope: &'a Scope<'a>,
    chain: Option<Rc<LexicalFrame>>,
    list: Vec<Capture>,
    block: KExpression<'a>,
    registries: &RunRegistries,
) -> Action<'a> {
    let plan = match resolve(scope, chain.as_ref(), &list, registries) {
        Ok(Pass::Ready(plan)) => plan,
        Ok(Pass::Park(sources)) => {
            let finish: AwaitContinue<'a> = Box::new(move |fctx, _results| {
                build(fctx.scope, chain, list, block, fctx.registries)
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

    // The body's working copies are frozen into the **eternal** region, not the call site's: the
    // block's statements are read from the block frame's own cart, which severance leaves with no
    // link to the caller, so a copy homed at the call site would dangle the moment that frame
    // retires. The eternal tier is the one region the block scope already names — its lexical outer
    // — and it contributes no reach, so nothing the block escapes with is retained by the choice.
    //
    // The seed's failure travels out here rather than through the block: a capture that cannot bind
    // is this statement's own error, and the block has not dispatched anything yet.
    let mut failure: Option<KError> = None;
    let action = block_tail(
        eternal.brand(),
        FramePlacement::FreshChild {
            frame: Rc::clone(&frame),
        },
        BlockScope::FrameScope(frame),
        Some(seed(
            |block_scope, registries: &RunRegistries, gate: &mut WriteGate| {
                failure = plan.seed_into(block_scope, registries, gate).err();
            },
        )),
        BlockBody::Block(block),
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
fn read_capture_list(
    slot: KExpression<'_>,
    registries: &RunRegistries,
) -> Result<Vec<Capture>, KError> {
    if slot
        .parts
        .iter()
        .any(|part| matches!(part.value, ExpressionPart::Keyword(_)))
    {
        return Ok(vec![Capture::Pattern(read_pattern(slot, registries)?)]);
    }
    slot.parts
        .iter()
        .map(|part| match part.value {
            ExpressionPart::Identifier(name) => Ok(Capture::Value(name)),
            ExpressionPart::Type(name) => Ok(Capture::Type(name)),
            ExpressionPart::Expression(group) => {
                Ok(Capture::Pattern(read_pattern(*group, registries)?))
            }
            _ => Err(shape_error(
                "a capture is a name or a signature-shaped group like `(HELPER _)`",
            )),
        })
        .collect()
}

/// Read one signature-shaped group into the untyped bucket key it names. `_` — already keyword-class
/// as a pure-symbol token, so no lexer arm is involved — maps to a slot; every other keyword maps to
/// itself. At least one non-wildcard keyword is required, since an all-slot key names no
/// registration.
fn read_pattern(group: KExpression<'_>, registries: &RunRegistries) -> Result<UntypedKey, KError> {
    let key: UntypedKey = group
        .parts
        .iter()
        .map(|part| match part.value {
            ExpressionPart::Keyword(symbol) if symbol == WILDCARD.symbol() => Ok(KeyElement::Slot),
            ExpressionPart::Keyword(symbol) => Ok(KeyElement::Keyword(symbol)),
            _ => Err(shape_error(
                "a capture pattern holds keywords and `_` holes only",
            )),
        })
        .collect::<Result<_, _>>()?;
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

/// One resolution pass: the explicit captures, then implicit close over the per-call chain.
/// Every in-flight producer either pass meets is collected into a single park set, so N captures
/// and every visible claim cost one wake between them.
fn resolve(
    scope: &Scope<'_>,
    chain: Option<&Rc<LexicalFrame>>,
    list: &[Capture],
    registries: &RunRegistries,
) -> Result<Pass, KError> {
    let mut plan = CapturePlan::default();
    let mut parked: Vec<ProducerId> = Vec::new();
    let frame = chain.map(|c| &**c);

    for capture in list {
        match capture {
            Capture::Value(name) => match scope.resolve_value_delivered(*name, frame) {
                Some(NameLookup::Bound(delivered)) => plan.values.push((*name, delivered)),
                Some(NameLookup::Parked(producer)) => park(&mut parked, producer),
                None => {
                    return Err(KError::new(KErrorKind::UnboundName(render_label(
                        name.symbol(),
                        registries,
                    ))));
                }
            },
            Capture::Type(name) => {
                match scope.resolve_type_identifier(*name, chain.cloned(), registries) {
                    TypeResolution::Done(kt) => plan.types.push((*name, kt)),
                    TypeResolution::Park(sources) => {
                        for producer in sources {
                            park(&mut parked, producer);
                        }
                    }
                    TypeResolution::Unbound(missing) => {
                        return Err(KError::new(KErrorKind::UnboundName(render_label(
                            missing.symbol(),
                            registries,
                        ))));
                    }
                }
            }
            Capture::Pattern(key) => {
                resolve_pattern(scope, frame, key, registries, &mut plan, &mut parked)?
            }
        }
    }

    close_implicitly(scope, frame, &mut plan, &mut parked);

    Ok(if parked.is_empty() {
        Pass::Ready(Box::new(plan))
    } else {
        Pass::Park(parked)
    })
}

fn park(parked: &mut Vec<ProducerId>, producer: ProducerId) {
    if !parked.contains(&producer) {
        parked.push(producer);
    }
}

/// Capture every visible overload registered under `key`, anywhere on the chain. A bucket key names
/// a *registration set*, not one overload, and dispatch's own walk keeps going past a scope whose
/// overloads do not match — so taking the innermost scope's bucket alone would drop overloads the
/// call site could still have reached. A visible pending claim on the key joins the park set.
fn resolve_pattern(
    scope: &Scope<'_>,
    frame: Option<&LexicalFrame>,
    key: &UntypedKey,
    registries: &RunRegistries,
    plan: &mut CapturePlan,
    parked: &mut Vec<ProducerId>,
) -> Result<(), KError> {
    let mut found = false;
    for ancestor in scope.ancestors() {
        let (overloads, pending) = ancestor
            .bindings()
            .lifted_overloads_for(key, ancestor.binding_cutoff(frame));
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
/// token, an already-registered operator probe and a standing name all mean an inner scope's entry
/// won.
fn close_implicitly(
    scope: &Scope<'_>,
    frame: Option<&LexicalFrame>,
    plan: &mut CapturePlan,
    parked: &mut Vec<ProducerId>,
) {
    for ancestor in scope
        .ancestors()
        .take_while(|s| s.parent_frame_pin().is_some())
    {
        let visible = ancestor
            .bindings()
            .visible_for_capture(ancestor.binding_cutoff(frame));
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

impl CapturePlan {
    /// Write the whole plan into the freshly minted block scope, under the construction gate
    /// `block_tail` mints for the call. Explicit captures land first, so they shadow anything
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
    let signature = sig(
        KType::ANY,
        vec![
            kw(registries, "CLOSE"),
            kw(registries, "OVER"),
            arg(registries, &SLOTS.captures, KType::KEXPRESSION),
            arg(registries, &SLOTS.body, KType::KEXPRESSION),
        ],
    );
    crate::builtins::register_builtin(scope, "CLOSE OVER", signature, body, registries, gate);
}

#[cfg(test)]
mod tests;
