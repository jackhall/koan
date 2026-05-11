//! Ascription operators `:|` (opaque) and `:!` (transparent) — bolt a [`Signature`] onto
//! a [`Module`]. Both consume `(Module, Signature)` and produce a `Module`.
//! See [design/module-system.md](../../../design/module-system.md).
//!
//! Stage 1 shape-checking is name-presence only; full type-shape checks are deferred to
//! the inference scheduler.

use crate::dispatch::{
    Argument, ArgumentBundle, BodyResult, ExpressionSignature, KError, KErrorKind, KObject, KType,
    Scope, SchedulerHandle, SignatureElement,
};
use crate::dispatch::values::{resolve_module, resolve_signature, Module};

use super::register_builtin;

/// `<m:Module> :| <s:Signature>` — opaque ascription.
pub fn body_opaque<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let (m, s) = match resolve_module_and_signature(&bundle) {
        Ok(pair) => pair,
        Err(e) => return BodyResult::Err(e),
    };

    let arena = scope.arena;
    let new_scope = arena.alloc_scope(Scope::child_under_named(
        scope,
        format!("MODULE {} :| {}", m.path, s.path),
    ));

    // Mirror the source module's bindings into the new scope by reference (values are
    // arena-allocated and immutable). Direct insert bypasses `bind_value`/`register_function`
    // to avoid double-registering functions via both `data` and the bucket loop below.
    let src = m.child_scope();
    {
        let mut data = new_scope.data.borrow_mut();
        for (name, obj) in src.data.borrow().iter() {
            data.insert(name.clone(), obj);
        }
    }
    for (key, bucket) in src.functions.borrow().iter() {
        new_scope
            .functions
            .borrow_mut()
            .entry(key.clone())
            .or_default()
            .extend(bucket.iter().copied());
    }

    let new_module: &'a Module<'a> = arena.alloc_module(Module::new(m.path.clone(), new_scope));
    // Each minted `ModuleType` carries the new module's `scope_id`, so two opaque ascriptions
    // of the same source yield distinct types — the abstraction-barrier identity property.
    let scope_id = new_module.scope_id();
    let mut minted: Vec<(String, KType)> = Vec::new();
    for name in s.decl_scope().data.borrow().keys() {
        if is_abstract_type_name(name) {
            minted.push((
                name.clone(),
                KType::ModuleType { scope_id, name: name.clone() },
            ));
        }
    }
    if !minted.is_empty() {
        let mut tm = new_module.type_members.borrow_mut();
        for (n, t) in minted {
            tm.insert(n, t);
        }
    }

    if let Err(e) = shape_check(s, src) {
        return BodyResult::Err(e);
    }

    // Record the sig in the new module's compat set so a `KType::SignatureBound { sig_id }`
    // slot accepts this module. Every ascription path must do this — see
    // `Module::mark_satisfies` for the bookkeeping discipline.
    new_module.mark_satisfies(s.sig_id());

    // Ascription paths run on the outer scheduler; the resulting `Module` lives in `arena`
    // (the calling scope's arena), not in any per-call frame. `frame: None` is correct.
    let module_obj: &'a KObject<'a> = arena.alloc_object(KObject::KModule(new_module, None));
    BodyResult::Value(module_obj)
}

/// `<m:Module> :! <s:Signature>` — transparent ascription.
pub fn body_transparent<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let (m, s) = match resolve_module_and_signature(&bundle) {
        Ok(pair) => pair,
        Err(e) => return BodyResult::Err(e),
    };
    if let Err(e) = shape_check(s, m.child_scope()) {
        return BodyResult::Err(e);
    }
    // Reuse the source's child scope; the new Module just retags the path as a view.
    let arena = scope.arena;
    let new_module: &'a Module<'a> = arena.alloc_module(Module::new(
        format!("{} :! {}", m.path, s.path),
        m.child_scope(),
    ));
    // Same compat-set bookkeeping as `body_opaque`. `:!` makes the module appear as the
    // sig at the type level too — sig-typed slots accept it.
    new_module.mark_satisfies(s.sig_id());
    let module_obj: &'a KObject<'a> = arena.alloc_object(KObject::KModule(new_module, None));
    BodyResult::Value(module_obj)
}

/// Verify every non-abstract-type name in `sig` has a binding in `src_scope`.
/// Abstract-type declarations are skipped: they shape the abstraction, not the implementation.
fn shape_check<'a>(
    sig: &crate::dispatch::values::Signature<'a>,
    src_scope: &Scope<'a>,
) -> Result<(), KError> {
    let sig_data = sig.decl_scope().data.borrow();
    let src_data = src_scope.data.borrow();
    for name in sig_data.keys() {
        if is_abstract_type_name(name) {
            continue;
        }
        if !src_data.contains_key(name) {
            return Err(KError::new(KErrorKind::ShapeError(format!(
                "module does not satisfy signature `{}`: missing member `{}`",
                sig.path, name
            ))));
        }
    }
    Ok(())
}

/// True iff `name` classifies as a Type token (first char uppercase + at least one
/// lowercase elsewhere). See [design/type-system.md](../../../design/type-system.md#token-classes--the-parser-level-foundation).
fn is_abstract_type_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else { return false; };
    if !first.is_ascii_uppercase() {
        return false;
    }
    chars.any(|c| c.is_ascii_lowercase())
}

/// Resolve `m` and `s` from the bundle. Both slots are typed `Module` / `Signature`, so
/// the resolver is just a typed `as_module()` / `as_signature()` projection; the
/// `TypeMismatch` arm is a defensive guard against a future caller wiring something else.
fn resolve_module_and_signature<'a>(
    bundle: &ArgumentBundle<'a>,
) -> Result<(&'a crate::dispatch::values::Module<'a>, &'a crate::dispatch::values::Signature<'a>), KError> {
    let m_obj = bundle
        .get("m")
        .ok_or_else(|| KError::new(KErrorKind::MissingArg("m".to_string())))?;
    let s_obj = bundle
        .get("s")
        .ok_or_else(|| KError::new(KErrorKind::MissingArg("s".to_string())))?;
    let m = resolve_module(m_obj, "m")?;
    let s = resolve_signature(s_obj, "s")?;
    Ok((m, s))
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    // Both ascription operators take already-evaluated `Module` / `Signature` values.
    // Bare Type-token operands (`IntOrd :| OrderedSig`) ride the unified §7 / §8 wrap +
    // replay-park rails in `classify_for_pick` — they sub-dispatch through the
    // `value_lookup`-TypeExprRef overload to a `Future(KModule)` / `Future(KSignature)`,
    // which then matches these slots strictly. No parallel Type-Type overload required.
    register_builtin(
        scope,
        ":|",
        ExpressionSignature {
            return_type: KType::Module,
            elements: vec![
                SignatureElement::Argument(Argument { name: "m".into(), ktype: KType::Module }),
                SignatureElement::Keyword(":|".into()),
                SignatureElement::Argument(Argument { name: "s".into(), ktype: KType::Signature }),
            ],
        },
        body_opaque,
    );
    register_builtin(
        scope,
        ":!",
        ExpressionSignature {
            return_type: KType::Module,
            elements: vec![
                SignatureElement::Argument(Argument { name: "m".into(), ktype: KType::Module }),
                SignatureElement::Keyword(":!".into()),
                SignatureElement::Argument(Argument { name: "s".into(), ktype: KType::Signature }),
            ],
        },
        body_transparent,
    );
}

#[cfg(test)]
mod tests {
    use crate::builtins::test_support::{parse_one, run, run_one, run_one_err, run_root_silent};
    use crate::dispatch::{KErrorKind, KObject, KType, RuntimeArena};
    use crate::execute::Scheduler;
    use crate::parse::parse;

    #[test]
    fn opaque_ascription_returns_module() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "MODULE IntOrd = (LET compare = 0)\n\
             SIG OrderedSig = (LET compare = 0)\n\
             LET IntOrdAbstract = (IntOrd :| OrderedSig)",
        );
        let data = scope.data.borrow();
        assert!(matches!(data.get("IntOrdAbstract"), Some(KObject::KModule(_, _))));
    }

    #[test]
    fn transparent_ascription_returns_module() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "MODULE IntOrd = (LET compare = 0)\n\
             SIG OrderedSig = (LET compare = 0)\n\
             LET IntOrdView = (IntOrd :! OrderedSig)",
        );
        let data = scope.data.borrow();
        assert!(matches!(data.get("IntOrdView"), Some(KObject::KModule(_, _))));
    }

    #[test]
    fn ascription_missing_member_errors() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "MODULE Empty = (LET unrelated = 0)\n\
             SIG OrderedSig = (LET compare = 0)",
        );
        let err = run_one_err(scope, parse_one("Empty :| OrderedSig"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg)
                if msg.contains("OrderedSig") && msg.contains("`compare`")),
            "expected ShapeError naming OrderedSig and the missing member, got {err}",
        );
    }

    #[test]
    fn opaque_ascription_mints_distinct_module_type_per_application() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let src = "MODULE IntOrd = (LET compare = 0)\n\
             SIG OrderedSig = ((LET Type = Number) (LET compare = 0))\n\
             LET FirstAbstract = (IntOrd :| OrderedSig)\n\
             LET SecondAbstract = (IntOrd :| OrderedSig)";
        let exprs = parse(src).expect("parse should succeed");
        let mut sched = Scheduler::new();
        let mut ids = Vec::new();
        for expr in exprs {
            ids.push(sched.add_dispatch(expr, scope));
        }
        sched.execute().expect("scheduler should succeed");
        for (i, id) in ids.iter().enumerate() {
            if let Err(e) = sched.read_result(*id) {
                panic!("expr {} errored: {}", i, e);
            }
        }
        let data = scope.data.borrow();
        let a = match data.get("FirstAbstract") {
            Some(KObject::KModule(m, _)) => *m,
            _ => panic!("FirstAbstract should be a module"),
        };
        let b = match data.get("SecondAbstract") {
            Some(KObject::KModule(m, _)) => *m,
            _ => panic!("SecondAbstract should be a module"),
        };
        let a_t = a.type_members.borrow().get("Type").cloned();
        let b_t = b.type_members.borrow().get("Type").cloned();
        assert!(matches!(&a_t, Some(KType::ModuleType { .. })));
        assert!(matches!(&b_t, Some(KType::ModuleType { .. })));
        assert_ne!(a_t, b_t, "two opaque ascriptions must mint distinct ModuleTypes");
    }

    #[test]
    fn transparent_ascription_does_not_mint_module_types() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "MODULE IntOrd = (LET compare = 0)\n\
             SIG OrderedSig = (LET compare = 0)\n\
             LET ViewMod = (IntOrd :! OrderedSig)",
        );
        let data = scope.data.borrow();
        let v = match data.get("ViewMod") {
            Some(KObject::KModule(m, _)) => *m,
            _ => panic!("ViewMod should be a module"),
        };
        assert!(v.type_members.borrow().is_empty());
    }

    #[test]
    fn opaque_ascribed_module_member_access_works() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "MODULE IntOrd = (LET compare = 42)\n\
             SIG OrderedSig = (LET compare = 0)\n\
             LET IntOrdAbstract = (IntOrd :| OrderedSig)",
        );
        let result = run_one(scope, parse_one("IntOrdAbstract.compare"));
        assert!(matches!(result, KObject::Number(n) if *n == 42.0));
    }

    /// End-to-end example from [design/module-system.md](../../../design/module-system.md).
    #[test]
    fn roadmap_example_int_ord_with_ordered_sig() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "MODULE IntOrd = ((LET Type = Number) (LET compare = 7))\n\
             SIG OrderedSig = ((LET Type = Number) (LET compare = 0))\n\
             LET IntOrdAbstract = (IntOrd :| OrderedSig)",
        );

        let data = scope.data.borrow();
        let abstract_mod = match data.get("IntOrdAbstract") {
            Some(KObject::KModule(m, _)) => *m,
            other => panic!("IntOrdAbstract should be a module, got {:?}", other.map(|o| o.ktype())),
        };
        let minted = abstract_mod
            .type_members
            .borrow()
            .get("Type")
            .cloned()
            .expect("opaque ascription should mint a Type member");
        match &minted {
            KType::ModuleType { name, .. } => assert_eq!(name, "Type"),
            other => panic!("minted abstract type must be ModuleType, got {:?}", other),
        }
        assert_ne!(minted, KType::Number, "opaque IntOrdAbstract.Type must not equal Number");
        let compare = abstract_mod
            .child_scope()
            .data
            .borrow()
            .get("compare")
            .copied();
        assert!(matches!(compare, Some(KObject::Number(n)) if *n == 7.0));
    }

    // ---------- Functor integration (module-system stage 2 — functor slice) ----------
    //
    // These tests pin down the end-to-end functor path: a FN with a sig-typed parameter
    // whose body builds a fresh `MODULE Result = (...)`. Construction depends on three
    // pieces wired together:
    //
    //   (a) `KType::SignatureBound` on the parameter slot (Step 3 — resolver lowers SIG
    //       names) so the slot's `accepts_part` does the per-sig admissibility check.
    //   (b) `Module::compatible_sigs` populated by `:|` / `:!` (Step 4) so an ascribed
    //       module flows through the slot.
    //   (c) `lift_kobject`'s `KModule` arm (Step 8) attaching the FN's per-call
    //       `Rc<CallArena>` to the returned module so the body's `child_scope` outlives
    //       the dying frame.
    //
    // Module/sig declarations are dispatched in their own batch ahead of the functor
    // call so the synchronous `ScopeResolver` consultation in FN-def's parameter-list
    // elaboration sees the SIG binding (the same caveat documented on
    // `scope_resolver_lowers_type_expr_value_binding` above).

    /// Test 1 — Functor returns a module. A FN with a sig-typed parameter whose body
    /// declares `MODULE Result = (LET inner = 1)` produces a `KObject::KModule` whose
    /// child scope carries `inner = 1`.
    #[test]
    fn functor_returns_a_module() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "SIG OrderedSig = (LET compare = 0)\n\
             MODULE IntOrd = (LET compare = 7)",
        );
        run(scope, "LET int_ord_a = (IntOrd :! OrderedSig)");
        run(
            scope,
            "FN (MAKESET elem: OrderedSig) -> Module = (MODULE Result = (LET inner = 1))",
        );
        run(scope, "LET set_value = (MAKESET int_ord_a)");

        let data = scope.data.borrow();
        let m = match data.get("set_value") {
            Some(KObject::KModule(m, _)) => *m,
            other => panic!("set_value should be a module, got {:?}", other.map(|o| o.ktype())),
        };
        let inner = m.child_scope().data.borrow().get("inner").copied();
        assert!(matches!(inner, Some(KObject::Number(n)) if *n == 1.0));
    }

    /// Test 2 — Functor body sees the signature-typed parameter. `(elem.compare)` inside
    /// the body resolves through ATTR's KModule arm and reads `7` from the ascribed
    /// IntOrd; that value lands in `S.sample`.
    #[test]
    fn functor_body_reads_signature_typed_parameter() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "SIG OrderedSig = (LET compare = 0)\n\
             MODULE IntOrd = (LET compare = 7)",
        );
        run(scope, "LET int_ord_a = (IntOrd :! OrderedSig)");
        run(
            scope,
            "FN (MAKESET elem: OrderedSig) -> Module = (MODULE Result = (LET sample = (elem.compare)))",
        );
        run(scope, "LET set_value = (MAKESET int_ord_a)");

        let data = scope.data.borrow();
        let m = match data.get("set_value") {
            Some(KObject::KModule(m, _)) => *m,
            other => panic!("set_value should be a module, got {:?}", other.map(|o| o.ktype())),
        };
        let sample = m.child_scope().data.borrow().get("sample").copied();
        assert!(matches!(sample, Some(KObject::Number(n)) if *n == 7.0));
    }

    /// Test 3 — Per-call generative semantics. Two functor invocations produce modules
    /// whose `scope_id` differs, since each call's body runs in a fresh per-call frame
    /// whose arena hands out a fresh scope address. The `Module::scope_id` is the
    /// identity carrier `KType::ModuleType` would mint after `:|` opaque ascription;
    /// asserting on the bare `scope_id`s themselves directly pins the per-call
    /// generativity property without depending on multi-statement-FN-body forward refs
    /// (which fold through `CONS` and don't share lexical bindings between statements).
    #[test]
    fn functor_application_is_generative() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "SIG OrderedSig = (LET compare = 0)\n\
             MODULE IntOrd = (LET compare = 7)",
        );
        run(scope, "LET int_ord_a = (IntOrd :! OrderedSig)");
        run(
            scope,
            "FN (MAKESET elem: OrderedSig) -> Module = (MODULE Result = (LET inner = 1))",
        );
        run(scope, "LET set_one = (MAKESET (int_ord_a))");
        run(scope, "LET set_two = (MAKESET (int_ord_a))");

        let data = scope.data.borrow();
        let m1 = match data.get("set_one") {
            Some(KObject::KModule(m, _)) => *m,
            other => panic!("set_one should be a module, got ktype={:?}", other.map(|o| o.ktype())),
        };
        let m2 = match data.get("set_two") {
            Some(KObject::KModule(m, _)) => *m,
            _ => panic!("set_two should be a module"),
        };
        // Per-call generativity: each invocation allocates a fresh `child_scope` in its
        // own per-call frame's arena, so `scope_id`s differ. After `:|` ascription this
        // would seed two distinct `KType::ModuleType { scope_id, .. }` values; the
        // identity carrier is what makes the abstract types incompatible across calls.
        assert_ne!(
            m1.scope_id(),
            m2.scope_id(),
            "two functor applications must produce modules with distinct scope_id",
        );
    }

    /// Test 4 — Dispatch admissibility filters non-conforming modules. An unascribed
    /// `MODULE Empty` has an empty `compatible_sigs` set, so `accepts_part` for the
    /// `SignatureBound { sig_id }` slot rejects it and dispatch fails. (Also: ascribing
    /// `Empty :! OrderedSig` would itself fail at shape-check time since `Empty` lacks a
    /// `compare` member — verified by `ascription_missing_member_errors` above; the
    /// admissibility-only path is what's pinned here.)
    #[test]
    fn functor_rejects_unascribed_module_argument() {
        use crate::execute::Scheduler;
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "SIG OrderedSig = (LET compare = 0)\n\
             MODULE IntOrd = (LET compare = 7)",
        );
        run(
            scope,
            "FN (MAKESET elem: OrderedSig) -> Module = (MODULE Result = (LET inner = 1))",
        );
        // Bind `IntOrd` (an unascribed module) under a lowercase identifier so the
        // §7 auto-wrap pass triggers when the identifier appears in the SignatureBound
        // slot. The wrapped sub-Dispatch resolves to `Future(KModule(IntOrd, _))`, but
        // IntOrd's `compatible_sigs` is empty — no overload matches. Surfaces as
        // `DispatchFailed` out of `Scheduler::execute`.
        run(scope, "LET unascribed = IntOrd");
        let mut sched = Scheduler::new();
        sched.add_dispatch(parse_one("MAKESET unascribed"), scope);
        let err = sched.execute().expect_err("MAKESET on unascribed module should fail dispatch");
        assert!(
            matches!(&err.kind, KErrorKind::DispatchFailed { .. }),
            "expected DispatchFailed, got {err}",
        );
    }

    /// Test 5 — Sig-typed-parameter overload selection. Two functors share a keyword
    /// (`MAKESET`) but differ on parameter sig (`OrderedSig` vs `HashedSig`); a call
    /// with an OrderedSig-conforming module routes to the first body, a HashedSig one to
    /// the second.
    #[test]
    fn functor_overloads_dispatch_by_signature_bound_param() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "SIG OrderedSig = (LET compare = 0)\n\
             SIG HashedSig = (LET hash = 0)\n\
             MODULE IntOrd = (LET compare = 7)\n\
             MODULE IntHash = (LET hash = 11)",
        );
        run(
            scope,
            "LET int_ord_a = (IntOrd :! OrderedSig)\n\
             LET int_hash_a = (IntHash :! HashedSig)",
        );
        run(
            scope,
            "FN (MAKESET elem: OrderedSig) -> Module = (MODULE Result = (LET tag = 1))",
        );
        run(
            scope,
            "FN (MAKESET elem: HashedSig) -> Module = (MODULE Result = (LET tag = 2))",
        );
        run(scope, "LET ord_set = (MAKESET (int_ord_a))");
        run(scope, "LET hash_set = (MAKESET (int_hash_a))");

        let data = scope.data.borrow();
        let mo = match data.get("ord_set") { Some(KObject::KModule(m, _)) => *m, _ => panic!("ord_set not module") };
        let mh = match data.get("hash_set") { Some(KObject::KModule(m, _)) => *m, _ => panic!("hash_set not module") };
        let to = mo.child_scope().data.borrow().get("tag").copied();
        let th = mh.child_scope().data.borrow().get("tag").copied();
        assert!(matches!(to, Some(KObject::Number(n)) if *n == 1.0),
                "OrderedSig call should pick body with tag=1, got {:?}", to.map(|o| o.ktype()));
        assert!(matches!(th, Some(KObject::Number(n)) if *n == 2.0),
                "HashedSig call should pick body with tag=2, got {:?}", th.map(|o| o.ktype()));
    }

    /// Test 6 — Transparent ascription satisfies `SignatureBound`. Pins that `:!`
    /// (transparent) populates `compatible_sigs` the same way `:|` (opaque) does — the
    /// functor's sig-typed slot accepts a `:!`-ascribed module, and the body still reads
    /// the underlying member through the view.
    #[test]
    fn transparent_ascription_satisfies_signature_bound_slot() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "SIG OrderedSig = (LET compare = 0)\n\
             MODULE IntOrd = (LET compare = 7)",
        );
        run(scope, "LET int_view = (IntOrd :! OrderedSig)");
        run(
            scope,
            "FN (MAKESET elem: OrderedSig) -> Module = (MODULE Result = (LET sample = (elem.compare)))",
        );
        run(scope, "LET set_value = (MAKESET int_view)");

        let data = scope.data.borrow();
        let m = match data.get("set_value") {
            Some(KObject::KModule(m, _)) => *m,
            other => panic!("set_value should be a module, got {:?}", other.map(|o| o.ktype())),
        };
        let sample = m.child_scope().data.borrow().get("sample").copied();
        assert!(matches!(sample, Some(KObject::Number(n)) if *n == 7.0));
    }

    /// Test 7 — bare Type-token argument auto-wraps into a value-lookup. A `LET`-bound
    /// Type-classified name (`IntOrdA`) passed as `MAKESET IntOrdA` should resolve to its
    /// bound `KModule` the same way the lowercase-identifier and parens-wrapped forms do.
    /// Pins the §7 wrap extension to Type-tokens via the `value_lookup`-TypeExprRef overload.
    #[test]
    fn functor_argument_bare_type_token_auto_wraps() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "SIG OrderedSig = (LET compare = 0)\n\
             MODULE IntOrd = (LET compare = 7)",
        );
        run(scope, "LET IntOrdA = (IntOrd :! OrderedSig)");
        run(
            scope,
            "FN (MAKESET elem: OrderedSig) -> Module = \
             (MODULE Result = (LET sample = (elem.compare)))",
        );
        run(scope, "LET set_value = (MAKESET IntOrdA)");

        let data = scope.data.borrow();
        let m = match data.get("set_value") {
            Some(KObject::KModule(m, _)) => *m,
            other => panic!("set_value should be a module, got {:?}", other.map(|o| o.ktype())),
        };
        let sample = m.child_scope().data.borrow().get("sample").copied();
        assert!(matches!(sample, Some(KObject::Number(n)) if *n == 7.0));
    }

    /// Companion: bare Type-token in an ascription operand. `IntOrd :! OrderedSig` already
    /// works via the strict `Type, Type` overload at ascribe.rs:165, so the unified wrap
    /// shouldn't have regressed it. This test pins the path stays green after the wrap
    /// extension lands.
    #[test]
    fn ascription_with_bare_type_tokens_still_works() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "SIG OrderedSig = (LET compare = 0)\n\
             MODULE IntOrd = (LET compare = 7)\n\
             LET IntOrdA = (IntOrd :| OrderedSig)",
        );
        let data = scope.data.borrow();
        assert!(matches!(data.get("IntOrdA"), Some(KObject::KModule(_, _))));
    }
}
