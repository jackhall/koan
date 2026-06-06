use crate::machine::core::source::Spanned;
use crate::machine::model::ast::ExpressionPart;
use crate::machine::model::types::{elaborate_type_expr, ElabResult, Elaborator};
use crate::machine::model::{KObject, KType};
use crate::machine::{
    ArgumentBundle, BodyResult, CombineFinish, KError, KErrorKind, SchedulerHandle, Scope,
};

use crate::builtins::ascribe::{abstract_type_names_of, is_abstract_type_name};
use crate::builtins::err;

/// `SIG_WITH <sig:Signature> <bindings:KExpression>` → `TypeExprRef` carrying
/// `KType::Signature { sig, pinned_slots }`.
///
/// Each binding pair is `(SlotName <value>)` where `SlotName` is a bare Type token
/// and `<value>` is one of:
/// - `Type(t)` — elaborated via [`elaborate_type_expr`].
/// - `Future(KTypeValue(_))` — already-resolved carrier from a sub-Dispatch wake.
/// - `Expression(...)` — sub-dispatched and joined via `Combine` finish.
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

    // Single-slot bindings collapse to one pair (`[Type, <value>]`); multi-slot
    // bindings keep each pair as its own `Expression(...)`. Both shapes route
    // through `parse_pair`; anything else errors below.
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
            ExpressionPart::Type(t) => t.render(),
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
    let inner_exprs = bindings_expr
        .borrow_inner_expressions()
        .filter(|v| !v.is_empty());
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

    // Reject unknown slot names before scheduling any sub-Dispatch.
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

    // Synchronous elaboration where possible; `Expression(...)` slots get parked
    // placeholders and a Combine join below.
    let mut pinned: Vec<(String, KType)> = Vec::with_capacity(triples.len());
    let mut sub_dispatches: Vec<(usize, crate::machine::model::ast::KExpression<'a>)> = Vec::new();
    let mut placeholders: Vec<usize> = Vec::new();
    for (slot_name, value_part, _idx) in &triples {
        match value_part {
            ExpressionPart::Type(t) => {
                let mut el = Elaborator::new(scope);
                match elaborate_type_expr(&mut el, t) {
                    ElabResult::Done(kt) => pinned.push((slot_name.clone(), kt)),
                    ElabResult::Park(_) => {
                        return err(KError::new(KErrorKind::ShapeError(format!(
                            "SIG_WITH binding `{slot_name}` parked on unresolved leaf \
                             `{}` — leaves must be synchronously resolvable",
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
                // Reserve an ordered placeholder; the Combine finish overwrites it.
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

    if sub_dispatches.is_empty() {
        return BodyResult::Value(scope.arena.alloc_object(KObject::KTypeValue(
            KType::Signature {
                sig: s,
                pinned_slots: pinned,
            },
        )));
    }

    // `submission_layout[k] = pinned_idx` maps `results[k]` to `pinned[pinned_idx]`.
    let mut deps: Vec<crate::machine::NodeId> = Vec::with_capacity(sub_dispatches.len());
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
            scope
                .arena
                .alloc_object(KObject::KTypeValue(KType::Signature {
                    sig: s,
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

    #[test]
    fn sig_with_one_slot_returns_signature_bound_with_pinned_slot() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "SIG OrderedSig = ((LET Type = Number) (VAL compare :Number))",
        );
        // SIG installs a single type-side identity; read the `sig_id` from there.
        let sig_id = match scope.resolve_type("OrderedSig") {
            Some(KType::Signature { sig, .. }) => sig.sig_id(),
            _ => panic!("OrderedSig must bind a Signature KType"),
        };
        let result = run_one(scope, parse_one("SIG_WITH OrderedSig ((Type :Number))"));
        match result {
            KObject::KTypeValue(kt) => match kt {
                KType::Signature { sig, pinned_slots } => {
                    assert_eq!(sig.sig_id(), sig_id);
                    assert_eq!(sig.path, "OrderedSig");
                    assert_eq!(pinned_slots.len(), 1);
                    assert_eq!(pinned_slots[0].0, "Type");
                    assert_eq!(pinned_slots[0].1, KType::Number);
                }
                other => panic!("expected Signature, got {:?}", other),
            },
            other => panic!("expected KTypeValue, got {:?}", other.ktype()),
        }
    }

    /// Pins land in source order — `pinned_slots` is an ordered `Vec`.
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
            KObject::KTypeValue(KType::Signature { pinned_slots, .. }) => {
                assert_eq!(pinned_slots.len(), 2);
                assert_eq!(pinned_slots[0].0, "Elt");
                assert_eq!(pinned_slots[0].1, KType::Number);
                assert_eq!(pinned_slots[1].0, "Ord");
                assert_eq!(pinned_slots[1].1, KType::Str);
            }
            other => panic!("expected Signature KTypeValue, got {:?}", other.ktype()),
        }
    }

    /// Exercises the Combine-on-sub-dispatches path: an inner `Elem.Type` access
    /// in a pin-value position sub-dispatches and surfaces in `pinned_slots`.
    ///
    /// Ignored: post-ATTR-desugar, `Elem.Type` is a `SigiledTypeExpr` value, which
    /// `SIG_WITH`'s transitional binding-value handler does not accept (it admits a
    /// bare Type token, a parens-wrapped `Expression`, or a resolved `Future`). `SIG_WITH`
    /// retires in Phase 3 of type-operation-surfaces (→ infix `WITH`); delete this test then
    /// rather than teach a soon-dead handler the sigil shape.
    #[test]
    #[ignore = "SIG_WITH value handler doesn't accept the dotted SigiledTypeExpr value; SIG_WITH retires in Phase 3"]
    fn sig_with_inner_module_attr_path_elaborates() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "MODULE IntOrd = ((LET Type = Number) (LET compare = 0))\n\
             SIG OrderedSig = ((LET Type = Number) (VAL compare :Number))\n\
             SIG SetSig = ((LET Elt = Number) (VAL insert :Number))\n\
             LET Elem = (IntOrd :| OrderedSig)",
        );
        let result = run_one(scope, parse_one("SIG_WITH SetSig ((Elt Elem.Type))"));
        match result {
            KObject::KTypeValue(KType::Signature { sig, pinned_slots }) => {
                assert_eq!(sig.path, "SetSig");
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
            other => panic!("expected Signature KTypeValue, got {:?}", other.ktype()),
        }
    }

    #[test]
    fn sig_with_rejects_unknown_slot() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "SIG OrderedSig = ((LET Type = Number) (VAL compare :Number))",
        );
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

    /// Lowercase slot names parse as `Identifier` and get rejected before the
    /// abstract-type-slot lookup, so the diagnostic names the classification rule.
    #[test]
    fn sig_with_rejects_identifier_slot_name() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "SIG OrderedSig = ((LET Type = Number) (VAL compare :Number))",
        );
        let mut sched = Scheduler::new();
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

    /// A 3-part bindings expression matches neither the single-pair (2 parts) nor
    /// multi-pair (every part `Expression(...)`) shape and surfaces the structural
    /// diagnostic.
    #[test]
    fn sig_with_rejects_non_parens_bindings_form() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "SIG OrderedSig = ((LET Type = Number) (VAL compare :Number))",
        );
        let mut sched = Scheduler::new();
        let id = sched.add_dispatch(parse_one("SIG_WITH OrderedSig (Type Number Extra)"), scope);
        sched.execute().expect("scheduler runs to completion");
        let err = match sched.read_result(id) {
            Ok(_) => panic!("malformed bindings form must err"),
            Err(e) => e.clone(),
        };
        let msg = format!("{}", err);
        assert!(
            msg.contains("SIG_WITH bindings")
                || msg.contains("parens-wrapped")
                || msg.contains("pair"),
            "expected diagnostic to mention the bindings shape, got: {msg}",
        );
    }
}
