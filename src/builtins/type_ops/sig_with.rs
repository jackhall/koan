use crate::machine::core::source::Spanned;
use crate::machine::model::ast::ExpressionPart;
use crate::machine::model::types::{elaborate_type_expr, ElabResult, Elaborator};
use crate::machine::model::{KObject, KType};
use crate::machine::{
    ArgumentBundle, BodyResult, CombineFinish, KError, KErrorKind, Scope, SchedulerHandle,
};

use crate::builtins::ascribe::{abstract_type_names_of, is_abstract_type_name};
use crate::builtins::err;

/// `SIG_WITH <sig:Signature> <bindings:KExpression>` → `TypeExprRef` carrying
/// `KType::SatisfiesSignature { sig_id, sig_path, pinned_slots }`. The `bindings` slot is a
/// `KExpression` whose parts are themselves `Expression(...)` groups, one per inner
/// `(slot_name :value)` pair. Each inner expression must match `[Type(slot_name),
/// <value>]` — bare Type-token slot names only (`Type`, `Elt`); lowercase identifiers
/// are rejected because abstract-type slots classify as Type per
/// [`is_abstract_type_name`].
///
/// Value-part shapes:
/// - `Type(t)` — bare type name or `List<Number>`-style; elaborated via
///   [`elaborate_type_expr`] against the current scope.
/// - `Future(KTypeValue(_))` — already-resolved type-value spliced from a prior
///   sub-Dispatch (re-walk after Combine wake).
/// - `Expression(...)` — parens-wrapped sub-expression that must be sub-dispatched;
///   the body schedules it through the scheduler and parks on the result via
///   `Combine` finish.
///
/// Slot-name validation: each `slot_name` must appear in the SIG's `decl_scope` as
/// an abstract-type slot (resolved via [`abstract_type_names_of`]). Unknown slot
/// names error out as `"<SigPath> has no abstract type slot `<name>`"`.
pub fn body<'a>(
    scope: &'a Scope<'a>,
    sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let s = match bundle.require_signature("sig") {
        Ok(s) => s,
        Err(e) => return err(e),
    };
    let bindings_expr = match bundle.require_kexpression("bindings") {
        Ok(e) => e.clone(),
        Err(e) => return err(e),
    };

    // Pre-walk: the bindings_expr's shape comes from the parser's peel-redundant
    // pass. `((Type :Number))` collapses to a single Expression with parts
    // `[Type, Type]`; `((Type :Number) (Elt :IntOrd))` stays as two top-level
    // Expression parts each wrapping a pair. Detect both shapes here:
    // - 2 parts shaped `[Type, <value>]` => the whole bindings IS one pair
    //   (single-slot case after peeling).
    // - Every part is `Expression(...)` => each is its own pair (multi-slot case).
    // Anything else is a user error with a focused message.
    fn parse_pair<'a>(
        parts: &[Spanned<ExpressionPart<'a>>],
        out: &mut Vec<(String, ExpressionPart<'a>, usize)>,
        idx: usize,
    ) -> Result<(), KError> {
        if parts.len() != 2 {
            return Err(KError::new(KErrorKind::ShapeError(format!(
                "SIG_WITH binding must be a `(Name :Type)` pair (2 parts), got {} parts",
                parts.len(),
            ))));
        }
        let slot_name = match &parts[0].value {
            ExpressionPart::Type(t) if matches!(t.params, crate::machine::model::ast::TypeParams::None) => {
                t.name.clone()
            }
            ExpressionPart::Type(t) => {
                return Err(KError::new(KErrorKind::ShapeError(format!(
                    "SIG_WITH binding name must be a bare Type token (e.g. `Type`, `Elt`), got `{}`",
                    t.render(),
                ))));
            }
            ExpressionPart::Identifier(name) => {
                return Err(KError::new(KErrorKind::ShapeError(format!(
                    "SIG_WITH binding name `{name}` must be a Type token (Capital-first, with \
                     a lowercase letter) — abstract-type slots classify as Type, not Identifier",
                ))));
            }
            other => {
                return Err(KError::new(KErrorKind::ShapeError(format!(
                    "SIG_WITH binding name must be a Type token, got `{}`",
                    other.summarize(),
                ))));
            }
        };
        out.push((slot_name, parts[1].value.clone(), idx));
        Ok(())
    }

    let mut triples: Vec<(String, ExpressionPart<'a>, usize)> = Vec::new();
    let parts = &bindings_expr.parts;
    // `borrow_inner_expressions` returns `Some(exprs)` iff every part is `Expression(_)`
    // (and parts is non-empty). The typed view lets the per-element loop iterate
    // `&KExpression` directly — no downstream `unreachable!` arm needed.
    let inner_exprs = bindings_expr.borrow_inner_expressions().filter(|v| !v.is_empty());
    // A 2-part bindings list with non-Expression elements is the single-pair case after
    // peel-redundant. Routes through `parse_pair` so the Type-token / pair-shape error
    // surfaces with its focused diagnostic rather than the structural fallback.
    let is_single_pair = parts.len() == 2 && inner_exprs.is_none();
    if is_single_pair {
        if let Err(e) = parse_pair(parts, &mut triples, 0) {
            return err(e);
        }
    } else if let Some(exprs) = inner_exprs {
        for (idx, inner) in exprs.iter().enumerate() {
            if let Err(e) = parse_pair(&inner.parts, &mut triples, idx) {
                return err(e);
            }
        }
    } else {
        let summary: Vec<String> = parts.iter().map(|p| p.value.summarize()).collect();
        return err(KError::new(KErrorKind::ShapeError(format!(
            "SIG_WITH bindings must be a list of parens-wrapped `(Name :Type)` pairs, \
             got `[{}]`",
            summary.join(" "),
        ))));
    }

    // Validate every slot_name against the SIG's abstract-type slots. The
    // `abstract_type_names_of` helper sweeps both the SIG's `bindings.types` (where
    // `LET Type = Number` lands post-stage-3) and `bindings.data` (lowercase abstract
    // fallback). Reject unknown names here so the sub-Dispatch path doesn't fire on
    // garbage slots.
    let known_slots: std::collections::HashSet<String> =
        abstract_type_names_of(s.decl_scope()).into_iter().collect();
    for (slot_name, _, _) in &triples {
        if !is_abstract_type_name(slot_name) {
            return err(KError::new(KErrorKind::ShapeError(format!(
                "SIG_WITH binding name `{slot_name}` does not classify as an abstract-type slot \
                 (Capital-first with a lowercase letter)",
            ))));
        }
        if !known_slots.contains(slot_name) {
            return err(KError::new(KErrorKind::ShapeError(format!(
                "{} has no abstract type slot `{}`",
                s.path, slot_name,
            ))));
        }
    }

    // First pass: try synchronous elaboration. Bare `Type(t)` parts route through
    // `elaborate_type_expr`; `Future(KTypeValue)` carriers (Combine re-walk) lift
    // directly. Inner `Expression(...)` parts need a sub-Dispatch and force the
    // Combine path.
    let mut pinned: Vec<(String, KType)> = Vec::with_capacity(triples.len());
    let mut sub_dispatches: Vec<(usize, crate::machine::model::ast::KExpression<'a>)> = Vec::new();
    let mut placeholders: Vec<usize> = Vec::new(); // indices in `pinned` for sub-dispatch slots
    for (slot_name, value_part, _idx) in &triples {
        match value_part {
            ExpressionPart::Type(t) => {
                let mut el = Elaborator::new(scope);
                match elaborate_type_expr(&mut el, t) {
                    ElabResult::Done(kt) => pinned.push((slot_name.clone(), kt)),
                    ElabResult::Park(_) => {
                        // Treat a parked leaf as a sub-Dispatch on the bare leaf:
                        // wrap it in a one-part Expression that the `BareTypeLeaf` fast
                        // lane resolves to a `Future(KTypeValue(_))`. Today the
                        // SIG_WITH-at-FN-slot path already runs after the outer
                        // FN-def's Combine wake when leaves have terminalized, so
                        // this is a defensive fallback rather than the hot path.
                        return err(KError::new(KErrorKind::ShapeError(format!(
                            "SIG_WITH binding `{slot_name}` parked on unresolved leaf \
                             `{}` — Phase A1 requires synchronously-resolvable leaves",
                            t.render(),
                        ))));
                    }
                    ElabResult::Unbound(msg) => {
                        return err(KError::new(KErrorKind::ShapeError(format!(
                            "SIG_WITH binding `{slot_name}` value: {msg}",
                        ))));
                    }
                }
            }
            ExpressionPart::Future(KObject::KTypeValue(kt)) => {
                pinned.push((slot_name.clone(), kt.clone()));
            }
            ExpressionPart::Future(other) => {
                return err(KError::new(KErrorKind::ShapeError(format!(
                    "SIG_WITH binding `{slot_name}` value must be a type expression, got a `{}` value",
                    other.ktype().name(),
                ))));
            }
            ExpressionPart::Expression(boxed) => {
                // Sub-Dispatch the inner parens expression on the scheduler. Reserve
                // a placeholder slot in `pinned` so the slot-name order is stable;
                // the Combine finish overwrites it with the resolved `KType`.
                let placeholder_idx = pinned.len();
                pinned.push((slot_name.clone(), KType::Any));
                placeholders.push(placeholder_idx);
                sub_dispatches.push((placeholder_idx, (**boxed).clone()));
            }
            other => {
                return err(KError::new(KErrorKind::ShapeError(format!(
                    "SIG_WITH binding `{slot_name}` value must be a type expression \
                     (Type token, parens-wrapped expression, or already-resolved Future), \
                     got `{}`",
                    other.summarize(),
                ))));
            }
        }
    }

    let sig_id = s.sig_id();
    let sig_path = s.path.clone();

    if sub_dispatches.is_empty() {
        // Fully synchronous path — alloc the SatisfiesSignature carrier directly.
        return BodyResult::Value(
            scope.arena.alloc(KObject::KTypeValue(KType::SatisfiesSignature {
                sig_id,
                sig_path,
                pinned_slots: pinned,
            })),
        );
    }

    // Combine path. Schedule each sub-Dispatch, collect their NodeIds in submission
    // order, then build a Combine whose finish re-reads each result as a
    // `KObject::KTypeValue(kt)` and overwrites the placeholder at `pinned[idx]`.
    let mut deps: Vec<crate::machine::NodeId> = Vec::with_capacity(sub_dispatches.len());
    // `submission_layout[k] = pinned_idx` says "results[k] fills pinned[pinned_idx]".
    let mut submission_layout: Vec<usize> = Vec::with_capacity(sub_dispatches.len());
    for (pinned_idx, sub_expr) in sub_dispatches {
        deps.push(sched.add_dispatch(sub_expr, scope));
        submission_layout.push(pinned_idx);
    }

    let finish: CombineFinish<'a> = Box::new(move |scope, _sched, results| {
        let mut pinned = pinned;
        for (k, &pinned_idx) in submission_layout.iter().enumerate() {
            let obj = results[k];
            match obj {
                KObject::KTypeValue(kt) => {
                    pinned[pinned_idx].1 = kt.clone();
                }
                other => {
                    return BodyResult::Err(KError::new(KErrorKind::ShapeError(format!(
                        "SIG_WITH binding `{}` value resolved to non-type `{}`",
                        pinned[pinned_idx].0,
                        other.ktype().name(),
                    ))));
                }
            }
        }
        BodyResult::Value(
            scope.arena.alloc(KObject::KTypeValue(KType::SatisfiesSignature {
                sig_id,
                sig_path,
                pinned_slots: pinned,
            })),
        )
    });
    let combine_id = sched.add_combine(deps, vec![], scope, finish);
    BodyResult::DeferTo(combine_id)
}

#[cfg(test)]
mod tests {
    use crate::builtins::test_support::{parse_one, run, run_one, run_root_silent};
    use crate::machine::execute::Scheduler;
    use crate::machine::model::{KObject, KType};
    use crate::machine::RuntimeArena;

    /// `(SIG_WITH OrderedSig ((Type: Number)))` dispatches and returns a `KTypeValue`
    /// whose `SatisfiesSignature` carries the matching `sig_id` and a one-entry
    /// `pinned_slots` vec pinning `Type` to `Number`.
    #[test]
    fn sig_with_one_slot_returns_signature_bound_with_pinned_slot() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "SIG OrderedSig = ((LET Type = Number) (VAL compare :Number))");
        // Pull the SIG's sig_id out of the scope so we can compare.
        let sig_id = match scope.bindings().data().get("OrderedSig").map(|(o, _)| *o) {
            Some(KObject::KTypeValue(KType::Signature(s))) => s.sig_id(),
            _ => panic!("OrderedSig must bind a KSignature"),
        };
        let result = run_one(scope, parse_one("SIG_WITH OrderedSig ((Type :Number))"));
        match result {
            KObject::KTypeValue(kt) => match kt {
                KType::SatisfiesSignature { sig_id: id, sig_path, pinned_slots } => {
                    assert_eq!(*id, sig_id);
                    assert_eq!(sig_path, "OrderedSig");
                    assert_eq!(pinned_slots.len(), 1);
                    assert_eq!(pinned_slots[0].0, "Type");
                    assert_eq!(pinned_slots[0].1, KType::Number);
                }
                other => panic!("expected SatisfiesSignature, got {:?}", other),
            },
            other => panic!("expected KTypeValue, got {:?}", other.ktype()),
        }
    }

    /// Multi-slot `(SIG_WITH Set ((Elt: Number) (Ord: Number)))` returns the pins in
    /// source order — `pinned_slots` is an order-preserving `Vec`, not a `HashMap`.
    /// (Using `Number` for both slots since the test only cares about ordering, not
    /// pinning to a custom type.)
    #[test]
    fn sig_with_two_slots_preserves_source_order() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "SIG Set = ((LET Elt = Number) (LET Ord = Number) (VAL tag :Number))",
        );
        let result = run_one(scope, parse_one("SIG_WITH Set ((Elt :Number) (Ord :Str))"));
        match result {
            KObject::KTypeValue(KType::SatisfiesSignature { pinned_slots, .. }) => {
                assert_eq!(pinned_slots.len(), 2);
                assert_eq!(pinned_slots[0].0, "Elt");
                assert_eq!(pinned_slots[0].1, KType::Number);
                assert_eq!(pinned_slots[1].0, "Ord");
                assert_eq!(pinned_slots[1].1, KType::Str);
            }
            other => panic!("expected SatisfiesSignature KTypeValue, got {:?}", other.ktype()),
        }
    }

    /// Inner `MODULE_TYPE_OF` reference in a pin-value position sub-dispatches and the
    /// resulting `pinned_slots` carries the `KType::UserType { kind: Module, .. }` minted
    /// by ascription. Exercises the body's Combine-on-sub-dispatches path.
    #[test]
    fn sig_with_inner_module_attr_path_elaborates() {
        // UserTypeKind import retired with the type-language collapse — abstract-type
        // members are now `KType::AbstractType { source_module, name }` directly.
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        // Use a multi-letter Type-token name for the ascribed module so it classifies as
        // a Type (single-letter `E` is reserved by koan's token-classification rules —
        // see `tokens.rs`).
        run(
            scope,
            "MODULE IntOrd = ((LET Type = Number) (LET compare = 0))\n\
             SIG OrderedSig = ((LET Type = Number) (VAL compare :Number))\n\
             SIG SetSig = ((LET Elt = Number) (VAL insert :Number))\n\
             LET Elem = (IntOrd :| OrderedSig)",
        );
        let result = run_one(
            scope,
            parse_one("SIG_WITH SetSig ((Elt (MODULE_TYPE_OF Elem Type)))"),
        );
        match result {
            KObject::KTypeValue(KType::SatisfiesSignature { sig_path, pinned_slots, .. }) => {
                assert_eq!(sig_path, "SetSig");
                assert_eq!(pinned_slots.len(), 1);
                assert_eq!(pinned_slots[0].0, "Elt");
                match &pinned_slots[0].1 {
                    KType::AbstractType { name, .. } => {
                        assert_eq!(name, "Type");
                    }
                    other => panic!(
                        "expected pinned Elt to be AbstractType(Type), got {:?}",
                        other,
                    ),
                }
            }
            other => panic!("expected SatisfiesSignature KTypeValue, got {:?}", other.ktype()),
        }
    }

    /// Unknown abstract-type slot names error with a focused diagnostic naming the SIG
    /// and the offending name.
    #[test]
    fn sig_with_rejects_unknown_slot() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "SIG OrderedSig = ((LET Type = Number) (VAL compare :Number))");
        let mut sched = Scheduler::new();
        let id = sched.add_dispatch(parse_one("SIG_WITH OrderedSig ((Bogus :Number))"), scope);
        sched.execute().expect("scheduler runs to completion");
        let err = match sched.read_result(id) {
            Ok(_) => panic!("SIG_WITH on unknown slot must err"),
            Err(e) => e.clone(),
        };
        let msg = format!("{}", err);
        assert!(
            msg.contains("OrderedSig") && msg.contains("Bogus"),
            "expected error to name both SIG and slot, got: {msg}",
        );
    }

    /// Lowercase slot names (`type` instead of `Type`) parse as `Identifier`, not
    /// `Type`, and SIG_WITH rejects them with the abstract-type classification rule.
    #[test]
    fn sig_with_rejects_identifier_slot_name() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "SIG OrderedSig = ((LET Type = Number) (VAL compare :Number))");
        let mut sched = Scheduler::new();
        // `type` is an Identifier (lowercase first letter). The body rejects this
        // before the abstract-type-slot lookup, so the error names the classification
        // rule.
        let id = sched.add_dispatch(parse_one("SIG_WITH OrderedSig ((type :Number))"), scope);
        sched.execute().expect("scheduler runs to completion");
        let err = match sched.read_result(id) {
            Ok(_) => panic!("SIG_WITH with lowercase slot must err"),
            Err(e) => e.clone(),
        };
        let msg = format!("{}", err);
        assert!(
            msg.contains("Type token") || msg.contains("Capital-first"),
            "expected diagnostic to mention the Type-classification rule, got: {msg}",
        );
    }

    /// `(SIG_WITH OrderedSig Type Number)` — without parens around the binding triples
    /// — errors before validating slot names. The `bindings` slot is a KExpression
    /// whose parts must each be `Expression(...)`; bare `Type(...)` tokens at the top
    /// level get a focused rejection.
    ///
    /// Important caveat: the dispatcher will only reach `body` if the slot signature
    /// actually matches. `SIG_WITH OrderedSig Type Number` is a 4-part expression where
    /// `Type` is a Type-token at part index 2. The `bindings` slot is at index 2 and
    /// accepts `KExpression`, which `Type` is not — so the dispatcher fails before the
    /// body fires. Wrap the malformed bindings in a single parens to route through the
    /// body and exercise its rejection path.
    #[test]
    fn sig_with_rejects_non_parens_bindings_form() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "SIG OrderedSig = ((LET Type = Number) (VAL compare :Number))");
        // `(Type Number Extra)` is a single Expression at the bindings slot — three
        // parts, more than the 2-part pair shape SIG_WITH bindings require. The body
        // should reject because the single-pair fallback requires exactly 2 parts and
        // the multi-pair fallback requires all parts to be `Expression(...)`.
        let mut sched = Scheduler::new();
        let id = sched.add_dispatch(parse_one("SIG_WITH OrderedSig (Type Number Extra)"), scope);
        sched.execute().expect("scheduler runs to completion");
        let err = match sched.read_result(id) {
            Ok(_) => panic!("malformed bindings form must err"),
            Err(e) => e.clone(),
        };
        let msg = format!("{}", err);
        assert!(
            msg.contains("SIG_WITH bindings") || msg.contains("parens-wrapped") || msg.contains("pair"),
            "expected diagnostic to mention the bindings shape, got: {msg}",
        );
    }
}
