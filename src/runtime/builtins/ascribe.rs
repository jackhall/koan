//! Ascription operators `:|` (opaque) and `:!` (transparent) — bolt a [`Signature`] onto
//! a [`Module`]. Both consume `(Module, Signature)` and produce a `Module`.
//! See [design/module-system.md](../../../design/module-system.md).
//!
//! Stage 1 shape-checking is name-presence only; full type-shape checks are deferred to
//! the inference scheduler.

use crate::runtime::machine::model::{KObject, KType};
use crate::runtime::machine::model::types::UserTypeKind;
use crate::runtime::machine::{ArgumentBundle, BodyResult, KError, KErrorKind, Scope, SchedulerHandle};
use crate::runtime::machine::model::values::{resolve_module, resolve_signature, Module};

use super::{arg, kw, register_builtin, sig};

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
    let new_scope = arena.alloc_scope(Scope::child_under_module(
        scope,
        format!("{} :| {}", m.path, s.path),
    ));

    // Mirror the source module's bindings into the new scope by reference (values are
    // arena-allocated and immutable). The `try_bulk_install_from` helper snapshots
    // `src.data`, releases the source guard, and replays each entry through the shared
    // `try_apply` so the `KFunction → functions` dual-map mirror happens exactly once per
    // entry — no separate functions-loop needed.
    let src = m.child_scope();
    if let Err(e) = new_scope.bindings().try_bulk_install_from(src.bindings()) {
        return BodyResult::Err(e);
    }

    let new_module: &'a Module<'a> = arena.alloc_module(Module::new(m.path.clone(), new_scope));
    // Each minted abstract type carries the new module's `scope_id`, so two opaque
    // ascriptions of the same source yield distinct types — the abstraction-barrier
    // identity property. `kind: Module` reuses the user-declared-module family; the
    // distinction from a first-class module value is by `name` (the abstract type
    // name, typically `"Type"`, vs. the module's full path).
    //
    // Module-system stage 2: per-slot kind selection. A SIG slot declared with
    // `LET Wrap = (TYPE_CONSTRUCTOR T)` lives in the SIG's decl_scope as a
    // `KType::UserType { kind: TypeConstructor { param_names }, .. }` template; we
    // mint a fresh per-call `TypeConstructor` rather than the default `Module` arm.
    // The lookup inspects `bindings.types` (where Type-class LET aliases land via
    // `register_type`) and falls back to the default `Module` mint for plain
    // abstract-type slots (`LET Type = Number`).
    let scope_id = new_module.scope_id();
    let mut minted: Vec<(String, KType)> = Vec::new();
    {
        let sig_types = s.decl_scope().bindings().types();
        for name in abstract_type_names_of(s.decl_scope()) {
            let kind = match sig_types.get(&name) {
                Some(KType::UserType { kind: UserTypeKind::TypeConstructor { param_names }, .. }) => {
                    UserTypeKind::TypeConstructor { param_names: param_names.clone() }
                }
                _ => UserTypeKind::Module,
            };
            minted.push((
                name.clone(),
                KType::UserType {
                    kind,
                    scope_id,
                    name: name.clone(),
                },
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
    sig: &crate::runtime::machine::model::values::Signature<'a>,
    src_scope: &Scope<'a>,
) -> Result<(), KError> {
    // Snapshot abstract-type names first so the helper's `data` borrow releases before
    // we acquire our own — honors the `types → functions → data` borrow order.
    let abstract_names: std::collections::HashSet<String> =
        abstract_type_names_of(sig.decl_scope()).into_iter().collect();
    let sig_data = sig.decl_scope().bindings().data();
    let src_data = src_scope.bindings().data();
    for name in sig_data.keys() {
        if abstract_names.contains(name.as_str()) {
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

/// Collect every name in `scope`'s `Bindings` that classifies as an abstract Type member.
/// Type-class LET aliases write `bindings.types` via `register_type`; other carriers that
/// classify as Type-tokens at use still land on `bindings.data`. Sweeping both maps keeps
/// the helper's answer robust to either binding home; names already in `types` are not
/// duplicated.
///
/// Goes through the [`Bindings`](crate::runtime::machine::core::Bindings) façade — no
/// raw `RefCell` reach-around. Drops `types_guard` before acquiring `data_guard` to
/// honor the `types → functions → data` borrow ordering.
pub(super) fn abstract_type_names_of<'a>(scope: &crate::runtime::machine::Scope<'a>) -> Vec<String> {
    let bindings = scope.bindings();
    let types_guard = bindings.types();
    let mut names: Vec<String> = types_guard.keys().cloned().collect();
    drop(types_guard);
    let types_set: std::collections::HashSet<String> = names.iter().cloned().collect();
    let data_guard = bindings.data();
    for k in data_guard.keys() {
        if is_abstract_type_name(k) && !types_set.contains(k) {
            names.push(k.clone());
        }
    }
    names
}

/// True iff `name` classifies as a Type token (first char uppercase + at least one
/// lowercase elsewhere). See [design/type-system.md](../../../design/type-system.md#token-classes--the-parser-level-foundation).
pub(super) fn is_abstract_type_name(name: &str) -> bool {
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
) -> Result<(&'a crate::runtime::machine::model::values::Module<'a>, &'a crate::runtime::machine::model::values::Signature<'a>), KError> {
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
    // Bare Type-token operands (`IntOrd :| OrderedSig`) ride the unified auto-wrap +
    // replay-park rails in [`KFunction::classify_for_pick`] — they sub-dispatch through
    // the `value_lookup`-TypeExprRef overload to a `Future(KModule)` / `Future(KSignature)`,
    // which then matches these slots strictly. No parallel Type-Type overload required.
    let module_ty = KType::AnyUserType { kind: UserTypeKind::Module };
    register_builtin(
        scope,
        ":|",
        sig(module_ty.clone(), vec![
            arg("m", module_ty.clone()),
            kw(":|"),
            arg("s", KType::Signature),
        ]),
        body_opaque,
    );
    register_builtin(
        scope,
        ":!",
        sig(module_ty.clone(), vec![
            arg("m", module_ty),
            kw(":!"),
            arg("s", KType::Signature),
        ]),
        body_transparent,
    );
}

#[cfg(test)]
mod tests {
    use crate::runtime::builtins::test_support::{parse_one, run, run_one, run_one_err, run_root_silent};
    use crate::runtime::machine::model::{KObject, KType};
    use crate::runtime::machine::{KErrorKind, RuntimeArena};
    use crate::runtime::machine::execute::Scheduler;
    use crate::parse::parse;

    #[test]
    fn opaque_ascription_returns_module() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "MODULE IntOrd = (LET compare = 0)\n\
             SIG OrderedSig = (VAL compare: Number)\n\
             LET IntOrdAbstract = (IntOrd :| OrderedSig)",
        );
        let data = scope.bindings().data();
        assert!(matches!(data.get("IntOrdAbstract"), Some(KObject::KModule(_, _))));
    }

    #[test]
    fn transparent_ascription_returns_module() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "MODULE IntOrd = (LET compare = 0)\n\
             SIG OrderedSig = (VAL compare: Number)\n\
             LET IntOrdView = (IntOrd :! OrderedSig)",
        );
        let data = scope.bindings().data();
        assert!(matches!(data.get("IntOrdView"), Some(KObject::KModule(_, _))));
    }

    #[test]
    fn ascription_missing_member_errors() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "MODULE Empty = (LET unrelated = 0)\n\
             SIG OrderedSig = (VAL compare: Number)",
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
             SIG OrderedSig = ((LET Type = Number) (VAL compare: Number))\n\
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
        let data = scope.bindings().data();
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
        use crate::runtime::machine::model::types::UserTypeKind;
        assert!(matches!(
            &a_t,
            Some(KType::UserType { kind: UserTypeKind::Module, .. })
        ));
        assert!(matches!(
            &b_t,
            Some(KType::UserType { kind: UserTypeKind::Module, .. })
        ));
        assert_ne!(a_t, b_t, "two opaque ascriptions must mint distinct module abstract types");
    }

    #[test]
    fn transparent_ascription_does_not_mint_module_types() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "MODULE IntOrd = (LET compare = 0)\n\
             SIG OrderedSig = (VAL compare: Number)\n\
             LET ViewMod = (IntOrd :! OrderedSig)",
        );
        let data = scope.bindings().data();
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
             SIG OrderedSig = (VAL compare: Number)\n\
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
             SIG OrderedSig = ((LET Type = Number) (VAL compare: Number))\n\
             LET IntOrdAbstract = (IntOrd :| OrderedSig)",
        );

        let data = scope.bindings().data();
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
        use crate::runtime::machine::model::types::UserTypeKind;
        match &minted {
            KType::UserType { kind: UserTypeKind::Module, name, .. } => assert_eq!(name, "Type"),
            other => panic!("minted abstract type must be UserType(Module), got {:?}", other),
        }
        assert_ne!(minted, KType::Number, "opaque IntOrdAbstract.Type must not equal Number");
        let compare = abstract_mod
            .child_scope().bindings().data()
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
    // call so the synchronous scope-aware elaboration in FN-def's parameter-list walk
    // sees the SIG binding (the same caveat documented on the elaborator-binding test
    // above).

    /// Test 1 — Functor returns a module. A FN with a sig-typed parameter whose body
    /// declares `MODULE Result = (LET inner = 1)` produces a `KObject::KModule` whose
    /// child scope carries `inner = 1`.
    #[test]
    fn functor_returns_a_module() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "SIG OrderedSig = (VAL compare: Number)\n\
             MODULE IntOrd = (LET compare = 7)",
        );
        run(scope, "LET int_ord_a = (IntOrd :! OrderedSig)");
        run(
            scope,
            "FN (MAKESET elem: OrderedSig) -> Module = (MODULE Result = (LET inner = 1))",
        );
        run(scope, "LET set_value = (MAKESET int_ord_a)");

        let data = scope.bindings().data();
        let m = match data.get("set_value") {
            Some(KObject::KModule(m, _)) => *m,
            other => panic!("set_value should be a module, got {:?}", other.map(|o| o.ktype())),
        };
        let inner = m.child_scope().bindings().data().get("inner").copied();
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
            "SIG OrderedSig = (VAL compare: Number)\n\
             MODULE IntOrd = (LET compare = 7)",
        );
        run(scope, "LET int_ord_a = (IntOrd :! OrderedSig)");
        run(
            scope,
            "FN (MAKESET elem: OrderedSig) -> Module = (MODULE Result = (LET sample = (elem.compare)))",
        );
        run(scope, "LET set_value = (MAKESET int_ord_a)");

        let data = scope.bindings().data();
        let m = match data.get("set_value") {
            Some(KObject::KModule(m, _)) => *m,
            other => panic!("set_value should be a module, got {:?}", other.map(|o| o.ktype())),
        };
        let sample = m.child_scope().bindings().data().get("sample").copied();
        assert!(matches!(sample, Some(KObject::Number(n)) if *n == 7.0));
    }

    /// Test 3 — Per-call generative semantics. Two functor invocations produce modules
    /// whose `scope_id` differs, since each call's body runs in a fresh per-call frame
    /// whose arena hands out a fresh scope address. The `Module::scope_id` is the
    /// identity carrier `KType::UserType { kind: Module, .. }` would mint after `:|` opaque ascription;
    /// asserting on the bare `scope_id`s themselves directly pins the per-call
    /// generativity property without depending on multi-statement-FN-body forward refs
    /// (which fold through `CONS` and don't share lexical bindings between statements).
    #[test]
    fn functor_application_is_generative() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "SIG OrderedSig = (VAL compare: Number)\n\
             MODULE IntOrd = (LET compare = 7)",
        );
        run(scope, "LET int_ord_a = (IntOrd :! OrderedSig)");
        run(
            scope,
            "FN (MAKESET elem: OrderedSig) -> Module = (MODULE Result = (LET inner = 1))",
        );
        run(scope, "LET set_one = (MAKESET (int_ord_a))");
        run(scope, "LET set_two = (MAKESET (int_ord_a))");

        let data = scope.bindings().data();
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
        // would seed two distinct `KType::UserType { kind: Module, scope_id, .. }` values; the
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
        use crate::runtime::machine::execute::Scheduler;
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "SIG OrderedSig = (VAL compare: Number)\n\
             MODULE IntOrd = (LET compare = 7)",
        );
        run(
            scope,
            "FN (MAKESET elem: OrderedSig) -> Module = (MODULE Result = (LET inner = 1))",
        );
        // Bind `IntOrd` (an unascribed module) under a lowercase identifier so the
        // auto-wrap pass triggers when the identifier appears in the SignatureBound
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
            "SIG OrderedSig = (VAL compare: Number)\n\
             SIG HashedSig = (VAL hash: Number)\n\
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

        let data = scope.bindings().data();
        let mo = match data.get("ord_set") { Some(KObject::KModule(m, _)) => *m, _ => panic!("ord_set not module") };
        let mh = match data.get("hash_set") { Some(KObject::KModule(m, _)) => *m, _ => panic!("hash_set not module") };
        let to = mo.child_scope().bindings().data().get("tag").copied();
        let th = mh.child_scope().bindings().data().get("tag").copied();
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
            "SIG OrderedSig = (VAL compare: Number)\n\
             MODULE IntOrd = (LET compare = 7)",
        );
        run(scope, "LET int_view = (IntOrd :! OrderedSig)");
        run(
            scope,
            "FN (MAKESET elem: OrderedSig) -> Module = (MODULE Result = (LET sample = (elem.compare)))",
        );
        run(scope, "LET set_value = (MAKESET int_view)");

        let data = scope.bindings().data();
        let m = match data.get("set_value") {
            Some(KObject::KModule(m, _)) => *m,
            other => panic!("set_value should be a module, got {:?}", other.map(|o| o.ktype())),
        };
        let sample = m.child_scope().bindings().data().get("sample").copied();
        assert!(matches!(sample, Some(KObject::Number(n)) if *n == 7.0));
    }

    /// Test 7 — bare Type-token argument auto-wraps into a value-lookup. A `LET`-bound
    /// Type-classified name (`IntOrdA`) passed as `MAKESET IntOrdA` should resolve to its
    /// bound `KModule` the same way the lowercase-identifier and parens-wrapped forms do.
    /// Pins the auto-wrap extension to Type-tokens via the `value_lookup`-TypeExprRef overload.
    #[test]
    fn functor_argument_bare_type_token_auto_wraps() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "SIG OrderedSig = (VAL compare: Number)\n\
             MODULE IntOrd = (LET compare = 7)",
        );
        run(scope, "LET IntOrdA = (IntOrd :! OrderedSig)");
        run(
            scope,
            "FN (MAKESET elem: OrderedSig) -> Module = \
             (MODULE Result = (LET sample = (elem.compare)))",
        );
        run(scope, "LET set_value = (MAKESET IntOrdA)");

        let data = scope.bindings().data();
        let m = match data.get("set_value") {
            Some(KObject::KModule(m, _)) => *m,
            other => panic!("set_value should be a module, got {:?}", other.map(|o| o.ktype())),
        };
        let sample = m.child_scope().bindings().data().get("sample").copied();
        assert!(matches!(sample, Some(KObject::Number(n)) if *n == 7.0));
    }

    /// Module-system stage 2 Workstream B: two opaque ascriptions of a module that
    /// satisfies a SIG declaring `LET Wrap = (TYPE_CONSTRUCTOR T)` mint distinct
    /// per-call `KType::UserType { kind: TypeConstructor, .. }` values under each
    /// resulting module's `type_members[Wrap]`. Mirror of
    /// `functor_application_is_generative` — pins the abstraction-barrier property
    /// for higher-kinded slots.
    #[test]
    fn opaque_ascription_mints_fresh_type_constructor_per_call() {
        use crate::runtime::machine::model::types::UserTypeKind;
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let src = "SIG MonadSig = ((LET Wrap = (TYPE_CONSTRUCTOR Type)))\n\
                   MODULE IntList = ((LET Wrap = Number))\n\
                   LET First = (IntList :| MonadSig)\n\
                   LET Second = (IntList :| MonadSig)";
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
        let data = scope.bindings().data();
        let a = match data.get("First") {
            Some(KObject::KModule(m, _)) => *m,
            _ => panic!("First should be a module"),
        };
        let b = match data.get("Second") {
            Some(KObject::KModule(m, _)) => *m,
            _ => panic!("Second should be a module"),
        };
        let a_wrap = a.type_members.borrow().get("Wrap").cloned();
        let b_wrap = b.type_members.borrow().get("Wrap").cloned();
        // Both wraps must be UserType(TypeConstructor) — the SIG slot kind, not
        // the default Module kind.
        assert!(matches!(
            &a_wrap,
            Some(KType::UserType { kind: UserTypeKind::TypeConstructor { .. }, .. })
        ));
        assert!(matches!(
            &b_wrap,
            Some(KType::UserType { kind: UserTypeKind::TypeConstructor { .. }, .. })
        ));
        // Per-call generativity: two opaque ascriptions get distinct scope_ids on the
        // minted slot, even though the SIG and source module are the same. The manual
        // `UserTypeKind::PartialEq` ignores `param_names`, so the equality test below
        // is gated on `(scope_id, name)` — exactly the abstraction-barrier property.
        match (&a_wrap, &b_wrap) {
            (
                Some(KType::UserType { scope_id: aid, .. }),
                Some(KType::UserType { scope_id: bid, .. }),
            ) => {
                assert_ne!(
                    aid, bid,
                    "two opaque ascriptions must mint TypeConstructor slots with distinct scope_id",
                );
            }
            _ => unreachable!("matched above"),
        }
        assert_ne!(
            a_wrap, b_wrap,
            "two opaque ascriptions must mint distinct TypeConstructor types",
        );
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
            "SIG OrderedSig = (VAL compare: Number)\n\
             MODULE IntOrd = (LET compare = 7)\n\
             LET IntOrdA = (IntOrd :| OrderedSig)",
        );
        let data = scope.bindings().data();
        assert!(matches!(data.get("IntOrdA"), Some(KObject::KModule(_, _))));
    }

    /// Miri audit-slate: pins the opaque-ascription re-bind path under tree borrows.
    /// `body_opaque` allocates a fresh child scope, mirrors the source module's bindings
    /// into it via `try_bulk_install_from` (which replays each entry through `try_apply`
    /// so a `KFunction` entry exercises the `functions`-map dual-write mirror as well as
    /// the plain `data` write), and builds the resulting `Module` over the captured
    /// scope. The captured-reference shape is the per-call analogue of the
    /// `module_child_scope_transmute_does_not_dangle` site, so the slate needs an
    /// end-to-end pin that the re-bind walk plus the held `&Module` survive subsequent
    /// arena churn under tree borrows.
    #[test]
    fn opaque_ascription_re_binds_do_not_alias_unsoundly() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        // The source module carries a plain `LET` plus a `LET = FN` so the
        // `try_bulk_install_from` walk hits the `KFunction → functions` dual-map mirror
        // (`LET <name> = (FN ...)` is the canonical shape for module-member functions per
        // `module_member_function_via_let_fn` in `module_def.rs`) as well as the plain
        // `data` write path. The SIG only requires `compare`; `helper` is a non-required
        // FN member that still rides through the re-bind walk.
        run(
            scope,
            "SIG OrderedSig = (VAL compare: Number)\n\
             MODULE IntOrd = ((LET compare = 7) (LET helper = (FN (HELP x: Number) -> Number = (x))))\n\
             LET Held = (IntOrd :| OrderedSig)",
        );
        // Extract the module pointer *before* further dispatches — `bindings().data()`
        // returns a `Ref<_>` and holding it across a `run` would block the RefCell
        // writes the new dispatches need.
        let held = {
            let data = scope.bindings().data();
            match data.get("Held") {
                Some(KObject::KModule(m, _)) => *m,
                other => panic!("Held should be a module, got {:?}", other.map(|o| o.ktype())),
            }
        };

        // Subsequent allocations and FN calls churn the run-root arena. The re-bound
        // child scope (and the `&Module` pointing at it) must keep both maps live
        // across that churn.
        run(scope, "FN (CHURNCALL) -> Number = (1)");
        for _ in 0..20 {
            run_one(scope, parse_one("CHURNCALL"));
        }
        // Re-ascribe a second time to allocate another re-bind scope; the original
        // `held` reference must still walk through to its own data/functions pair.
        run(scope, "LET Held2 = (IntOrd :| OrderedSig)");

        // Read both binding kinds back through the held module's child scope. The
        // `compare` slot tests the plain `data` mirror; the `helper` slot tests the
        // `KFunction → functions` mirror written by `try_apply`.
        let child = held.child_scope();
        let inner = child.bindings().data();
        assert!(
            matches!(inner.get("compare"), Some(KObject::Number(n)) if *n == 7.0),
            "held.child_scope().compare must still read 7.0 after subsequent churn",
        );
        assert!(
            matches!(inner.get("helper"), Some(KObject::KFunction(_, _))),
            "held.child_scope().helper must still resolve to a KFunction after churn",
        );
    }
}
